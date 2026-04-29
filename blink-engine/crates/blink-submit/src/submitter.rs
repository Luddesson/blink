//! The hot-path [`Submitter`].
//!
//! One instance is shared (wrapped in `Arc`) across the decision kernel's
//! submit workers. Workers call [`Submitter::submit`] with a validated
//! [`Intent`] plus the coid + intent_hash chosen by the driver loop.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use blink_h2::{H2Client, H2Error};
use blink_signer::SignerPool;
use blink_timestamps::Timestamp;
use blink_types::{Intent, StageTimestamps, TimeInForce};
use bytes::Bytes;
use serde::Deserialize;

use crate::auth::{build_poly_headers, AuthError, PolyAuthCreds};
use crate::encoder::{EncodedOrder, OrderEncoder, POLYMARKET_CTF_EXCHANGE};
use crate::stats::SubmitterStats;
use crate::templates::{compute_amounts_for_intent, OrderTemplate};
use crate::verdict::{SubmitVerdict, UnknownReason};

// ─── Config ──────────────────────────────────────────────────────────────

/// L2 credentials passed to [`Submitter::new`].
///
/// Kept as a separate struct (rather than baked into
/// [`SubmitterConfig`]) so operator code can hold credentials in a TEE /
/// vault-managed object without leaking them into the config struct's
/// `Debug` output.
#[derive(Clone)]
pub struct PolyAuth {
    pub api_key: String,
    pub api_secret_b64: String,
    pub passphrase: String,
}

/// Construction-time configuration.
#[derive(Clone, Debug)]
pub struct SubmitterConfig {
    /// HTTP authority (used for the `host` header). E.g.
    /// `"clob.polymarket.com"`.
    pub authority: String,
    /// POST path for order submission. Typically `"/order"`.
    pub post_path: String,
    /// Hard submit deadline (covers sign + HTTP round-trip).
    pub post_timeout: Duration,
    /// Proxy-wallet / funder address — appears as the `maker` on-chain.
    pub maker_address: [u8; 20],
    /// CTF Exchange contract address. Typically
    /// [`POLYMARKET_CTF_EXCHANGE`].
    pub exchange_address: [u8; 20],
}

impl SubmitterConfig {
    /// Defaults matching Polygon-mainnet Polymarket.
    pub fn polymarket_mainnet(maker_address: [u8; 20]) -> Self {
        Self {
            authority: "clob.polymarket.com".to_string(),
            post_path: "/order".to_string(),
            post_timeout: Duration::from_millis(300),
            maker_address,
            exchange_address: POLYMARKET_CTF_EXCHANGE,
        }
    }
}

// ─── Probe result ────────────────────────────────────────────────────────

/// Outcome of a probe query issued after an [`SubmitVerdict::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    /// CLOB confirmed an order exists for the given coid.
    Found {
        venue_order_id: String,
        status: String,
    },
    /// CLOB confirmed no order exists for the coid — safe to retry.
    NotFound,
    /// The blink-h2 client only exposes POST today; GET-by-coid is not
    /// plumbed through. Callers must fall back to a slower reconcile
    /// (journal replay + `GET /order/{id}` once we learn the id from
    /// fills). Documented explicitly so operators never mistake this
    /// for "no order exists".
    ///
    /// # Limitation
    /// The current `blink-h2::H2Client` only exposes `post()`. A probe
    /// endpoint needs either (a) a `get()` method on H2Client, or
    /// (b) a separate reconciliation path reading the `OrderFilled`
    /// on-chain log stream for the coid. Both are out of scope for
    /// p4-submit.
    Unsupported,
}

// ─── Submitter ───────────────────────────────────────────────────────────

/// Hot-path submit handle. Cheap to clone (everything is `Arc`-wrapped).
#[derive(Clone)]
pub struct Submitter {
    inner: Arc<Inner>,
}

struct Inner {
    cfg: SubmitterConfig,
    h2: Arc<H2Client>,
    signer: Arc<SignerPool>,
    encoder: OrderEncoder,
    signer_addr: [u8; 20],
    auth: Option<PolyAuthCreds>,
    stats: SubmitterStats,
}

impl Submitter {
    /// Build a submitter without venue auth. Only useful in tests /
    /// shadow runs where the POST target is a mock.
    pub fn new(cfg: SubmitterConfig, h2: Arc<H2Client>, signer: Arc<SignerPool>) -> Self {
        Self::with_auth(cfg, h2, signer, None)
    }

    /// Build a submitter with Polymarket L2 credentials.
    pub fn with_auth(
        cfg: SubmitterConfig,
        h2: Arc<H2Client>,
        signer: Arc<SignerPool>,
        auth: Option<PolyAuth>,
    ) -> Self {
        let encoder = OrderEncoder::new(cfg.maker_address, cfg.exchange_address);
        let signer_addr = signer.address(0);
        let maker_hex = format!("0x{}", hex_20(&cfg.maker_address));
        let auth = auth.map(|a| PolyAuthCreds {
            api_key: a.api_key,
            api_secret_b64: a.api_secret_b64,
            passphrase: a.passphrase,
            maker_address: maker_hex,
        });
        Self {
            inner: Arc::new(Inner {
                cfg,
                h2,
                signer,
                encoder,
                signer_addr,
                auth,
                stats: SubmitterStats::default(),
            }),
        }
    }

    /// Atomic counter snapshot.
    pub fn stats(&self) -> crate::stats::SubmitterStatsSnapshot {
        self.inner.stats.snapshot()
    }

    /// Submit one intent.
    ///
    /// * `coid` — the 16-byte client_order_id (caller derives via
    ///   [`crate::derive_client_order_id`]).
    /// * `salt` — EIP-712 salt, typically `u128::from_be_bytes(intent_hash[0..16])`
    ///   so replays of the same intent produce byte-identical signed
    ///   payloads and the venue's dedup layer catches the retry.
    /// * `stamps` — filled in through `tsc_sign`, `tsc_submit`, `tsc_ack`.
    ///
    /// Returns synchronously on terminal verdicts; never retries.
    pub async fn submit(
        &self,
        intent: &Intent,
        coid: [u8; 16],
        salt: u128,
        stamps: &mut StageTimestamps,
    ) -> SubmitVerdict {
        let inner = &self.inner;
        inner.stats.submits_total.fetch_add(1, Ordering::Relaxed);

        let tif = tif_str(intent.tif);

        // ─── 1. Encode ────────────────────────────────────────────────
        let t_enc0 = Timestamp::now();
        let encoded = match inner
            .encoder
            .encode(intent, inner.signer_addr, salt, &coid, tif)
        {
            Ok(e) => e,
            Err(e) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::PreflightError(format!("encode: {e}")),
                };
            }
        };
        let t_enc1 = Timestamp::now();
        inner
            .stats
            .encode_time_ns_total
            .fetch_add(t_enc1.elapsed_ns_since(t_enc0), Ordering::Relaxed);

        // ─── 2. Sign ──────────────────────────────────────────────────
        stamps.tsc_risk = t_enc1; // analogue of "tsc_signer_in"
        let t_sig0 = Timestamp::now();
        let sig_rs = inner.signer.sign(&encoded.digest);
        let mut sig65 = sig_rs.to_bytes65();
        // Legacy uses Ethereum-style v = 27 + rid; blink-signer returns
        // raw rid (0/1). Add 27 at the edge.
        // WIRE FORMAT REF: engine/src/order_signer.rs:168
        if sig65[64] < 27 {
            sig65[64] = sig65[64].wrapping_add(27);
        }
        let t_sig1 = Timestamp::now();
        stamps.tsc_sign = t_sig1;
        inner
            .stats
            .signer_time_ns_total
            .fetch_add(t_sig1.elapsed_ns_since(t_sig0), Ordering::Relaxed);

        // ─── 3. Build signed body ─────────────────────────────────────
        let body = match inner.encoder.build_wire_body_signed(
            intent,
            &encoded,
            inner.signer_addr,
            &sig65,
            &coid,
            tif,
        ) {
            Ok(b) => b,
            Err(e) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::PreflightError(format!("body: {e}")),
                };
            }
        };

        // ─── 4. Auth headers ──────────────────────────────────────────
        let headers_owned = match self.build_headers(&body) {
            Ok(h) => h,
            Err(e) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::PreflightError(format!("auth: {e}")),
                };
            }
        };
        let header_refs: Vec<(&str, &str)> = headers_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // ─── 5. POST under hard timeout ───────────────────────────────
        let t_tx0 = Timestamp::now();
        stamps.tsc_submit = t_tx0;
        let post_fut = inner
            .h2
            .post(&inner.cfg.post_path, &header_refs, body.clone());
        let h2_out = tokio::time::timeout(inner.cfg.post_timeout, post_fut).await;
        let t_tx1 = Timestamp::now();
        inner
            .stats
            .h2_time_ns_total
            .fetch_add(t_tx1.elapsed_ns_since(t_tx0), Ordering::Relaxed);

        let resp = match h2_out {
            Ok(Ok(r)) => r,
            Ok(Err(H2Error::Timeout)) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::Timeout,
                };
            }
            Ok(Err(e)) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::H2Error(e.to_string()),
                };
            }
            Err(_) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::Timeout,
                };
            }
        };

        stamps.tsc_ack = t_tx1;
        let snapshot = *stamps;
        let v = classify_response(&resp.body, resp.status, coid, snapshot);
        record_verdict(&inner.stats, &v);
        v
    }

    /// Submit one intent using a pre-computed [`OrderTemplate`] — the hot
    /// path after warmup. Saves the ABI-encode + 10-word Keccak absorb of
    /// the constant order fields versus [`Submitter::submit`].
    ///
    /// The caller is responsible for passing a template whose
    /// `(market_id, side_bit)` matches the intent; a debug-only assertion
    /// catches mismatches. In release, a mismatched template produces a
    /// digest the CLOB will reject, so misuse is observable.
    pub async fn submit_templated(
        &self,
        template: &OrderTemplate,
        intent: &Intent,
        coid: [u8; 16],
        salt: u128,
        stamps: &mut StageTimestamps,
    ) -> SubmitVerdict {
        let inner = &self.inner;
        inner.stats.submits_total.fetch_add(1, Ordering::Relaxed);

        let tif = tif_str(intent.tif);

        // ─── 1. Amounts + template digest ────────────────────────────
        let t_enc0 = Timestamp::now();
        let (maker_amount, taker_amount, side_bit) = match compute_amounts_for_intent(intent) {
            Ok(x) => x,
            Err(e) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::PreflightError(format!("amounts: {e}")),
                };
            }
        };
        debug_assert_eq!(
            template.side_bit, side_bit,
            "OrderTemplate side_bit={} but intent side_bit={}",
            template.side_bit, side_bit
        );
        debug_assert_eq!(
            template.market_id, intent.market_id,
            "OrderTemplate market_id mismatch"
        );

        let digest = template.digest(salt, maker_amount, taker_amount);
        let t_enc1 = Timestamp::now();
        inner
            .stats
            .encode_time_ns_total
            .fetch_add(t_enc1.elapsed_ns_since(t_enc0), Ordering::Relaxed);

        // ─── 2. Sign ──────────────────────────────────────────────────
        stamps.tsc_risk = t_enc1;
        let t_sig0 = Timestamp::now();
        let sig_rs = inner.signer.sign(&digest);
        let mut sig65 = sig_rs.to_bytes65();
        if sig65[64] < 27 {
            sig65[64] = sig65[64].wrapping_add(27);
        }
        let t_sig1 = Timestamp::now();
        stamps.tsc_sign = t_sig1;
        inner
            .stats
            .signer_time_ns_total
            .fetch_add(t_sig1.elapsed_ns_since(t_sig0), Ordering::Relaxed);

        // ─── 3. Build signed body ─────────────────────────────────────
        // Reconstruct an EncodedOrder shell so we can reuse
        // `build_wire_body_signed` (which is pure JSON; no keccak). The
        // `struct_hash` field is not observed by body construction.
        let encoded_shell = EncodedOrder {
            struct_hash: [0u8; 32],
            digest,
            wire_body_unsigned: bytes::Bytes::new(),
            maker_amount,
            taker_amount,
            salt,
        };
        let body = match inner.encoder.build_wire_body_signed(
            intent,
            &encoded_shell,
            inner.signer_addr,
            &sig65,
            &coid,
            tif,
        ) {
            Ok(b) => b,
            Err(e) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::PreflightError(format!("body: {e}")),
                };
            }
        };

        // ─── 4. Auth headers ──────────────────────────────────────────
        let headers_owned = match self.build_headers(&body) {
            Ok(h) => h,
            Err(e) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::PreflightError(format!("auth: {e}")),
                };
            }
        };
        let header_refs: Vec<(&str, &str)> = headers_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // ─── 5. POST under hard timeout ───────────────────────────────
        let t_tx0 = Timestamp::now();
        stamps.tsc_submit = t_tx0;
        let post_fut = inner
            .h2
            .post(&inner.cfg.post_path, &header_refs, body.clone());
        let h2_out = tokio::time::timeout(inner.cfg.post_timeout, post_fut).await;
        let t_tx1 = Timestamp::now();
        inner
            .stats
            .h2_time_ns_total
            .fetch_add(t_tx1.elapsed_ns_since(t_tx0), Ordering::Relaxed);

        let resp = match h2_out {
            Ok(Ok(r)) => r,
            Ok(Err(H2Error::Timeout)) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::Timeout,
                };
            }
            Ok(Err(e)) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::H2Error(e.to_string()),
                };
            }
            Err(_) => {
                inner.stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
                return SubmitVerdict::Unknown {
                    client_order_id: coid,
                    reason: UnknownReason::Timeout,
                };
            }
        };

        stamps.tsc_ack = t_tx1;
        let snapshot = *stamps;
        let v = classify_response(&resp.body, resp.status, coid, snapshot);
        record_verdict(&inner.stats, &v);
        v
    }

    /// Probe the CLOB for an order matching `coid`.
    ///
    /// Currently returns [`ProbeResult::Unsupported`] because
    /// `blink-h2::H2Client` only exposes `post()`. The shape is preserved
    /// so callers can be wired today and the probe implementation can be
    /// dropped in when the GET path lands.
    pub async fn probe_client_order_id(&self, _coid: &[u8; 16]) -> ProbeResult {
        ProbeResult::Unsupported
    }

    // ─── Internals ───────────────────────────────────────────────────

    fn build_headers(
        &self,
        body: &Bytes,
    ) -> Result<Vec<(String, String)>, String> {
        let inner = &self.inner;
        if let Some(creds) = inner.auth.as_ref() {
            let body_str = std::str::from_utf8(body).map_err(|e| e.to_string())?;
            let hs = build_poly_headers(creds, "POST", &inner.cfg.post_path, body_str)
                .map_err(|e: AuthError| e.to_string())?;
            Ok(hs.into_iter().collect())
        } else {
            // No auth configured (shadow / mock target). Still set
            // content-type so the server parses the body as JSON.
            Ok(vec![(
                "content-type".to_string(),
                "application/json".to_string(),
            )])
        }
    }

}

// ─── Pure classification helper ─────────────────────────────────────────

/// Classify a received `(status, body)` into a [`SubmitVerdict`]. Pure —
/// no stats / clocks / allocations beyond what the body requires.
fn classify_response(
    body: &[u8],
    status: u16,
    coid: [u8; 16],
    stamps_snapshot: StageTimestamps,
) -> SubmitVerdict {
    // Transient / server-error → Unknown, caller may retry.
    if status == 429 || (500..600).contains(&status) {
        return SubmitVerdict::Unknown {
            client_order_id: coid,
            reason: UnknownReason::Transient {
                status,
                body: String::from_utf8_lossy(body).to_string(),
            },
        };
    }

    // 4xx (non-429) → rejected by venue. 409 or dedup-marker body → replay.
    if (400..500).contains(&status) {
        if status == 409 || looks_like_dedup(body) {
            return SubmitVerdict::DuplicateDedup {
                client_order_id: coid,
            };
        }
        return SubmitVerdict::RejectedByVenue {
            reason_code: status,
            reason_text: String::from_utf8_lossy(body).to_string(),
            client_order_id: coid,
        };
    }

    // 2xx — parse the envelope.
    let parsed: OrderResponse = match serde_json::from_slice(body) {
        Ok(p) => p,
        Err(e) => {
            return SubmitVerdict::Unknown {
                client_order_id: coid,
                reason: UnknownReason::Parse(e.to_string()),
            };
        }
    };

    if parsed.success {
        if parsed
            .status
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("dedup"))
            .unwrap_or(false)
        {
            return SubmitVerdict::DuplicateDedup {
                client_order_id: coid,
            };
        }
        let oid = parsed.order_id.unwrap_or_default();
        SubmitVerdict::Accepted {
            venue_order_id: oid,
            client_order_id: coid,
            stamps: stamps_snapshot,
        }
    } else {
        let reason_text = parsed.error_msg.unwrap_or_default();
        let low = reason_text.to_lowercase();
        if low.contains("duplicate") || low.contains("already") {
            return SubmitVerdict::DuplicateDedup {
                client_order_id: coid,
            };
        }
        SubmitVerdict::RejectedByVenue {
            reason_code: 0,
            reason_text,
            client_order_id: coid,
        }
    }
}

fn record_verdict(stats: &SubmitterStats, v: &SubmitVerdict) {
    match v {
        SubmitVerdict::Accepted { .. } => {
            stats.submits_accepted.fetch_add(1, Ordering::Relaxed);
        }
        SubmitVerdict::RejectedByVenue { .. } => {
            stats.submits_rejected.fetch_add(1, Ordering::Relaxed);
        }
        SubmitVerdict::DuplicateDedup { .. } => {
            stats.submits_dedup.fetch_add(1, Ordering::Relaxed);
        }
        SubmitVerdict::Unknown { .. } => {
            stats.submits_unknown.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ─── Small helpers ───────────────────────────────────────────────────────

// WIRE FORMAT REF: engine/src/order_executor.rs:832-884 (OrderResponse)
#[derive(Debug, Deserialize)]
struct OrderResponse {
    success: bool,
    #[serde(rename = "orderID", alias = "orderId")]
    order_id: Option<String>,
    status: Option<String>,
    #[serde(rename = "errorMsg")]
    error_msg: Option<String>,
}

fn tif_str(tif: TimeInForce) -> &'static str {
    match tif {
        TimeInForce::Gtc => "GTC",
        TimeInForce::Fok => "FOK",
        TimeInForce::Fak => "FAK",
    }
}

fn hex_20(a: &[u8; 20]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(40);
    for b in a {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn looks_like_dedup(body: &[u8]) -> bool {
    let s = String::from_utf8_lossy(body).to_lowercase();
    s.contains("duplicate") || s.contains("already exists") || s.contains("dedup")
}

// Suppress the "unused field" lint for fields we hold for future
// observability / debugging. Treating them as `pub(crate)` would be
// wrong — they must stay module-private.
#[allow(dead_code)]
impl Inner {
    fn _touch(&self) -> (&H2Client, &SignerPool, &OrderEncoder, [u8; 20]) {
        (&self.h2, &self.signer, &self.encoder, self.signer_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamps() -> StageTimestamps {
        StageTimestamps::UNSET
    }

    #[test]
    fn classify_200_success_is_accepted() {
        let coid = [0x11; 16];
        let body = br#"{"success":true,"orderID":"abc123","status":"matched"}"#;
        let v = classify_response(body, 200, coid, stamps());
        match v {
            SubmitVerdict::Accepted {
                venue_order_id,
                client_order_id,
                ..
            } => {
                assert_eq!(venue_order_id, "abc123");
                assert_eq!(client_order_id, coid);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classify_200_failure_is_rejected() {
        let coid = [0x11; 16];
        let body = br#"{"success":false,"errorMsg":"not enough liquidity"}"#;
        assert!(matches!(
            classify_response(body, 200, coid, stamps()),
            SubmitVerdict::RejectedByVenue { reason_code: 0, .. }
        ));
    }

    #[test]
    fn classify_200_failure_duplicate_is_dedup() {
        let coid = [0x11; 16];
        let body = br#"{"success":false,"errorMsg":"duplicate order id"}"#;
        assert!(matches!(
            classify_response(body, 200, coid, stamps()),
            SubmitVerdict::DuplicateDedup { .. }
        ));
    }

    #[test]
    fn classify_409_is_dedup() {
        assert!(matches!(
            classify_response(b"duplicate coid", 409, [0x11; 16], stamps()),
            SubmitVerdict::DuplicateDedup { .. }
        ));
    }

    #[test]
    fn classify_400_dedup_body_is_dedup() {
        // Some CLOBs return 400 with "already exists" body; treat as dedup.
        assert!(matches!(
            classify_response(b"order already exists", 400, [0x11; 16], stamps()),
            SubmitVerdict::DuplicateDedup { .. }
        ));
    }

    #[test]
    fn classify_400_plain_is_rejected() {
        assert!(matches!(
            classify_response(b"bad price", 400, [0x11; 16], stamps()),
            SubmitVerdict::RejectedByVenue { reason_code: 400, .. }
        ));
    }

    #[test]
    fn classify_429_is_unknown_transient() {
        assert!(matches!(
            classify_response(b"slow", 429, [0x11; 16], stamps()),
            SubmitVerdict::Unknown {
                reason: UnknownReason::Transient { status: 429, .. },
                ..
            }
        ));
    }

    #[test]
    fn classify_500_is_unknown_transient() {
        assert!(matches!(
            classify_response(b"oops", 503, [0x11; 16], stamps()),
            SubmitVerdict::Unknown {
                reason: UnknownReason::Transient { status: 503, .. },
                ..
            }
        ));
    }

    #[test]
    fn classify_bad_json_is_unknown_parse() {
        assert!(matches!(
            classify_response(b"not json", 200, [0x11; 16], stamps()),
            SubmitVerdict::Unknown {
                reason: UnknownReason::Parse(_),
                ..
            }
        ));
    }

    #[test]
    fn classify_200_dedup_status_field() {
        let body = br#"{"success":true,"status":"dedup"}"#;
        assert!(matches!(
            classify_response(body, 200, [0x11; 16], stamps()),
            SubmitVerdict::DuplicateDedup { .. }
        ));
    }

    #[test]
    fn classify_200_order_id_alias() {
        // Some CLOB responses use camelCase `orderId` instead of `orderID`.
        let body = br#"{"success":true,"orderId":"xyz"}"#;
        match classify_response(body, 200, [0x11; 16], stamps()) {
            SubmitVerdict::Accepted { venue_order_id, .. } => {
                assert_eq!(venue_order_id, "xyz")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn record_verdict_increments() {
        let s = SubmitterStats::default();
        let coid = [0; 16];
        record_verdict(
            &s,
            &SubmitVerdict::Accepted {
                venue_order_id: "x".into(),
                client_order_id: coid,
                stamps: stamps(),
            },
        );
        record_verdict(
            &s,
            &SubmitVerdict::RejectedByVenue {
                reason_code: 400,
                reason_text: "".into(),
                client_order_id: coid,
            },
        );
        record_verdict(
            &s,
            &SubmitVerdict::DuplicateDedup {
                client_order_id: coid,
            },
        );
        record_verdict(
            &s,
            &SubmitVerdict::Unknown {
                client_order_id: coid,
                reason: UnknownReason::Timeout,
            },
        );
        let snap = s.snapshot();
        assert_eq!(snap.submits_accepted, 1);
        assert_eq!(snap.submits_rejected, 1);
        assert_eq!(snap.submits_dedup, 1);
        assert_eq!(snap.submits_unknown, 1);
    }
}
