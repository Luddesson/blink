//! # MempoolSource — Polygon mempool tap (feature `mempool-tap`)
//!
//! Subscribes to the local Polygon node's `newPendingTransactions`
//! WebSocket stream, then for every pending tx hash fires an
//! `eth_getTransactionByHash` on the same connection. If the tx's `to`
//! address matches the configured CTF exchange contract, a
//! [`RawEvent`] tagged [`SourceKind::MempoolCtf`] is emitted.
//!
//! ## ⚠️ OBSERVE-ONLY INVARIANT ⚠️
//!
//! Every event emitted by this source carries `observe_only = true`.
//! Downstream — in particular the decision kernel and the submit stage —
//! **MUST NOT** turn a mempool observation into an on-chain submission
//! unless an independent legal sign-off has been recorded in
//! `docs/rebuild/R3_LEGAL_MEMO_STUB.md` and the operator has explicitly
//! flipped `BLINK_MEMPOOL_SUBMIT=true` **and** not passed
//! `--mempool-observe-only`. The current default is strict: mempool
//! events inform internal priors only; they never trigger orders.
//!
//! This invariant is enforced here by hard-coding `observe_only = true`
//! at construction time. Do not add an escape hatch in this file.
//!
//! See plan §3 Phase 2 (`p2-ingress`) and Phase 5 for the conditional
//! loosening that would require a separate, audited change.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use blink_rings::Producer;
use blink_timestamps::Timestamp;
use blink_types::{EventId, OnChainAnchor, RawEvent, SourceKind, wall_clock_ns};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::{Source, SourceCounters, ShutdownToken, try_push};

const MIN_BACKOFF: Duration = Duration::from_millis(25);
const MAX_BACKOFF: Duration = Duration::from_millis(2000);

/// Configuration for [`MempoolSource`].
#[derive(Debug, Clone)]
pub struct MempoolConfig {
    /// Polygon JSON-RPC WS URL (e.g. `ws://127.0.0.1:8546`).
    pub url: String,
    /// 20-byte CTF exchange contract address. Only txs whose `to` field
    /// lower-cases to this value emit events.
    pub ctf_address: [u8; 20],
}

/// The mempool source. All events emitted by this source have
/// [`RawEvent::observe_only`] set to `true` — see module docs.
pub struct MempoolSource {
    cfg: MempoolConfig,
    counters: Arc<SourceCounters>,
}

impl MempoolSource {
    /// Construct.
    pub fn new(cfg: MempoolConfig) -> Self {
        Self {
            cfg,
            counters: SourceCounters::new(),
        }
    }
}

impl Source for MempoolSource {
    fn kind(&self) -> SourceKind {
        SourceKind::MempoolCtf
    }
    fn stats_handle(&self) -> Arc<SourceCounters> {
        self.counters.clone()
    }

    fn run(self: Box<Self>, sink: Producer<RawEvent>, shutdown: ShutdownToken) {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let cfg = self.cfg.clone();
        let counters = self.counters.clone();
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                log::error!("mempool: cannot build tokio runtime: {e}");
                return;
            }
        };
        rt.block_on(run_loop(cfg, sink, counters, shutdown));
    }
}

async fn run_loop(
    cfg: MempoolConfig,
    mut sink: Producer<RawEvent>,
    counters: Arc<SourceCounters>,
    shutdown: ShutdownToken,
) {
    let mut backoff = MIN_BACKOFF;
    let mut next_req_id: u64 = 100;
    while !shutdown.is_cancelled() {
        match tokio_tungstenite::connect_async(&cfg.url).await {
            Ok((mut ws, _)) => {
                backoff = MIN_BACKOFF;
                let sub = serde_json::json!({
                    "jsonrpc":"2.0","id":1,"method":"eth_subscribe",
                    "params":["newPendingTransactions"]
                });
                if ws.send(Message::Text(sub.to_string().into())).await.is_err() {
                    counters.reconnects.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => {
                            let _ = ws.close(None).await;
                            return;
                        }
                        msg = ws.next() => match msg {
                            Some(Ok(Message::Text(t))) => {
                                if let Some(hash) = extract_pending_hash(t.as_bytes()) {
                                    // Fire eth_getTransactionByHash on the same conn.
                                    next_req_id += 1;
                                    let req = serde_json::json!({
                                        "jsonrpc":"2.0",
                                        "id": next_req_id,
                                        "method":"eth_getTransactionByHash",
                                        "params":[format!("0x{}", hex_encode(&hash))]
                                    });
                                    let _ = ws.send(Message::Text(req.to_string().into())).await;
                                } else if let Some(ev) = decode_tx_detail(t.as_bytes(), &cfg.ctf_address) {
                                    try_push(&mut sink, &counters, ev);
                                }
                            }
                            Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                log::warn!("mempool: frame error: {e}");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
            Err(e) => log::warn!("mempool: connect failed: {e}"),
        }
        counters.reconnects.fetch_add(1, Ordering::Relaxed);
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.cancelled() => return,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Extract a pending-tx hash from an `eth_subscription` notification.
pub fn extract_pending_hash(bytes: &[u8]) -> Option<[u8; 32]> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let params = v.get("params")?;
    let result = params.get("result")?.as_str()?;
    parse_hex_bytes::<32>(result)
}

/// Decode an `eth_getTransactionByHash` response. If the tx's `to`
/// matches `ctf_address`, return a `RawEvent::MempoolCtfTx`-equivalent.
pub fn decode_tx_detail(bytes: &[u8], ctf_address: &[u8; 20]) -> Option<RawEvent> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    // Must be a response, not a subscription notification.
    let result = v.get("result")?;
    if result.is_null() {
        return None;
    }
    let to_str = result.get("to")?.as_str()?;
    let to = parse_hex_bytes::<20>(to_str)?;
    if !eq_ci_bytes(&to, ctf_address) {
        return None;
    }
    let from_str = result.get("from").and_then(|v| v.as_str()).unwrap_or("0x");
    let maker = parse_hex_bytes::<20>(from_str).unwrap_or([0u8; 20]);
    let hash_str = result.get("hash")?.as_str()?;
    let tx_hash = parse_hex_bytes::<32>(hash_str)?;
    let data_hex = result.get("input").and_then(|v| v.as_str()).unwrap_or("0x");
    let data_bytes = decode_hex(data_hex).unwrap_or_default();

    // Payload layout in `extra`: [20-byte maker][data].
    let mut payload = Vec::with_capacity(20 + data_bytes.len());
    payload.extend_from_slice(&maker);
    payload.extend_from_slice(&data_bytes);

    Some(RawEvent {
        event_id: EventId::fetch_next(),
        source: SourceKind::MempoolCtf,
        source_seq: u64::MAX,
        anchor: Some(OnChainAnchor {
            tx_hash,
            log_index: u32::MAX,
        }),
        token_id: String::new(),
        market_id: None,
        side: None,
        price: None,
        size: None,
        tsc_in: Timestamp::now(),
        wall_ns: wall_clock_ns(),
        extra: Some(payload.into_boxed_slice()),
        // INVARIANT: see module docs — never flip in this file.
        observe_only: true,
        maker_wallet: Some(maker),
    })
}

fn eq_ci_bytes(a: &[u8; 20], b: &[u8; 20]) -> bool {
    a == b
}

fn parse_hex_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for i in 0..N {
        out[i] = u8::from_str_radix(&stripped[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(stripped.len() / 2);
    for i in 0..stripped.len() / 2 {
        out.push(u8::from_str_radix(&stripped[i * 2..i * 2 + 2], 16).ok()?);
    }
    Some(out)
}

fn hex_encode(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for &v in b {
        out.push(HEX[(v >> 4) as usize] as char);
        out.push(HEX[(v & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    const CTF: [u8; 20] = [0x4b, 0xfb, 0x41, 0xd5, 0xb3, 0x57, 0x0d, 0xef, 0xd0, 0x3c, 0x39, 0xa9, 0xa4, 0xd8, 0xde, 0x6b, 0xd8, 0xb8, 0x98, 0x2e];

    fn hex_addr(a: &[u8; 20]) -> String {
        format!("0x{}", hex_encode(a))
    }

    #[test]
    fn extracts_pending_hash() {
        let notif = r#"{"jsonrpc":"2.0","method":"eth_subscription","params":{"subscription":"0x1","result":"0xabababababababababababababababababababababababababababababababab"}}"#;
        let h = extract_pending_hash(notif.as_bytes()).unwrap();
        assert_eq!(h[0], 0xab);
        assert_eq!(h[31], 0xab);
    }

    #[test]
    fn decode_tx_detail_emits_observe_only_event_when_to_matches() {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let tx_hash = "0x".to_string() + &"11".repeat(32);
        let from = "0x".to_string() + &"22".repeat(20);
        let resp = format!(
            r#"{{"jsonrpc":"2.0","id":101,"result":{{"hash":"{tx_hash}","from":"{from}","to":"{}","input":"0xdeadbeef"}}}}"#,
            hex_addr(&CTF)
        );
        let ev = decode_tx_detail(resp.as_bytes(), &CTF).expect("matches ctf");
        assert_eq!(ev.source, SourceKind::MempoolCtf);
        assert!(ev.observe_only, "MUST be observe_only");
        let extra = ev.extra.as_ref().unwrap();
        assert_eq!(&extra[0..20], &[0x22u8; 20]);
        assert_eq!(&extra[20..], &[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn decode_tx_detail_rejects_non_ctf_to() {
        let other = [0x99u8; 20];
        let tx_hash = "0x".to_string() + &"11".repeat(32);
        let resp = format!(
            r#"{{"jsonrpc":"2.0","id":101,"result":{{"hash":"{tx_hash}","from":"0x0000000000000000000000000000000000000001","to":"{}","input":"0x"}}}}"#,
            hex_addr(&other)
        );
        assert!(decode_tx_detail(resp.as_bytes(), &CTF).is_none());
    }

    #[test]
    fn decode_tx_detail_rejects_null_result() {
        let resp = r#"{"jsonrpc":"2.0","id":101,"result":null}"#;
        assert!(decode_tx_detail(resp.as_bytes(), &CTF).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mempool_end_to_end_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("ws://{addr}/");
        let tx_hash_hex = "0x".to_string() + &"ef".repeat(32);
        let from_hex = "0x".to_string() + &"33".repeat(20);
        let to_hex = hex_addr(&CTF);

        let tx_hash_hex_c = tx_hash_hex.clone();
        let from_hex_c = from_hex.clone();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            // Consume subscribe.
            let _ = ws.next().await;
            // Subscribe ack.
            let _ = ws
                .send(Message::Text(
                    r#"{"jsonrpc":"2.0","id":1,"result":"0xsub"}"#.into(),
                ))
                .await;
            // Pending notification.
            let notif = format!(
                r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0xsub","result":"{tx_hash_hex_c}"}}}}"#
            );
            let _ = ws.send(Message::Text(notif.into())).await;
            // Expect eth_getTransactionByHash call.
            let req = ws.next().await.and_then(|r| r.ok());
            assert!(req.is_some(), "expected getTransactionByHash call");
            // Respond with a matching tx.
            let tx_resp = format!(
                r#"{{"jsonrpc":"2.0","id":101,"result":{{"hash":"{tx_hash_hex_c}","from":"{from_hex_c}","to":"{to_hex}","input":"0xdeadbeef"}}}}"#
            );
            let _ = ws.send(Message::Text(tx_resp.into())).await;
            let _ = ws.close(None).await;
        });

        let (prod, mut cons) = blink_rings::bounded::<RawEvent>(16);
        let src = MempoolSource::new(MempoolConfig {
            url,
            ctf_address: CTF,
        });
        let shutdown = ShutdownToken::new();
        let shutdown2 = shutdown.clone();
        let handle = std::thread::spawn(move || {
            Box::new(src).run(prod, shutdown2);
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = None;
        while got.is_none() && std::time::Instant::now() < deadline {
            if let Some(ev) = cons.pop() {
                got = Some(ev);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        shutdown.cancel();
        let _ = server.await;
        let _ = handle.join();

        let ev = got.expect("expected one MempoolCtf event");
        assert_eq!(ev.source, SourceKind::MempoolCtf);
        assert!(ev.observe_only);
    }
}
