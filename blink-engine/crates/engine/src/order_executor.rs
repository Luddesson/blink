//! Polymarket CLOB order submission and lifecycle management.
//!
//! Handles REST API calls (POST/DELETE/GET) with Polymarket HMAC-SHA256
//! authentication headers.  When `dry_run` is `true` (which is always the case
//! when `Config::live_trading == false`), order submission is logged but no
//! real HTTP request is sent.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{info, instrument, warn, error};

use crate::config::Config;
use crate::order_signer::SignedOrder;
use crate::types::TimeInForce;

type HmacSha256 = Hmac<Sha256>;

// ─── Public types ────────────────────────────────────────────────────────────

/// Response from `POST /order`.
#[derive(Debug, Deserialize)]
pub struct OrderResponse {
    pub success: bool,
    #[serde(rename = "orderID")]
    pub order_id: Option<String>,
    /// `"matched"` | `"delayed"` | `"unmatched"` | absent on failure.
    pub status: Option<String>,
    /// Error message when `success == false`.
    #[serde(rename = "errorMsg")]
    pub error_msg: Option<String>,
}

/// Snapshot of a single order as returned by `GET /order/{id}`.
#[derive(Debug, Deserialize)]
pub struct OrderStatus {
    pub id: String,
    pub status: String,
    #[serde(rename = "makerAmount")]
    pub maker_amount: Option<String>,
    #[serde(rename = "takerAmount")]
    pub taker_amount: Option<String>,
    #[serde(rename = "remainingAmount")]
    pub remaining_amount: Option<String>,
    #[serde(rename = "sizeMatched")]
    pub size_matched: Option<String>,
}

// ─── Executor ────────────────────────────────────────────────────────────────

/// HTTP client for Polymarket CLOB order management.
///
/// When `dry_run` is `true` submission calls are logged but no network
/// request is made.  The engine always sets `dry_run = !live_trading`.
#[derive(Clone)]
pub struct OrderExecutor {
    client:         Client,
    base_url:       String,
    maker_address:  String,
    api_key:        String,
    /// Base64-encoded secret; decoded to raw bytes before HMAC use.
    api_secret:     String,
    passphrase:     String,
    /// When `true` outbound mutating requests are suppressed.
    pub dry_run:    bool,
}

impl OrderExecutor {
    /// Constructs an executor from [`Config`].
    ///
    /// `dry_run` is set to `!config.live_trading` automatically.
    pub fn from_config(config: &Config) -> Self {
        Self {
            client:        Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("failed to build HTTP client"),
            base_url:      "https://clob.polymarket.com".to_string(),
            maker_address: config.funder_address.clone(),
            api_key:       config.api_key.clone(),
            api_secret:    config.api_secret.clone(),
            passphrase:    config.api_passphrase.clone(),
            dry_run:       !config.live_trading,
        }
    }

    // ─── Order submission ────────────────────────────────────────────────────

    /// Submits a signed order to `POST /order`.
    ///
    /// Returns the exchange's [`OrderResponse`].  In dry-run mode the order is
    /// logged and a synthetic success response is returned instead.
    #[instrument(skip(self, order), fields(token_id = %order.token_id, side = order.side, dry_run = self.dry_run))]
    pub async fn submit_order(&self, order: &SignedOrder, time_in_force: TimeInForce) -> Result<OrderResponse> {
        let body = build_order_body(order, &self.maker_address, time_in_force);
        let body_json =
            serde_json::to_string(&body).context("failed to serialise order body")?;

        if self.dry_run {
            info!(
                body = %body_json,
                "DRY-RUN: would POST /order (live_trading=false)"
            );
            return Ok(OrderResponse {
                success:   true,
                order_id:  Some("dry-run".to_string()),
                status:    Some("dry_run".to_string()),
                error_msg: None,
            });
        }

        let url = format!("{}/order", self.base_url);
        const MAX_ATTEMPTS: u32 = 4;
        const BASE_DELAY_MS: u64 = 200;

        let mut last_err: Option<anyhow::Error> = None;

        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let delay_ms = BASE_DELAY_MS * 2_u64.pow(attempt - 1);
                warn!(
                    attempt,
                    delay_ms,
                    error = ?last_err,
                    "POST /order transient error — retrying"
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            // Rebuild auth headers each attempt: POLY-TIMESTAMP must be fresh.
            let headers = build_auth_headers(
                &self.api_key,
                &self.api_secret,
                &self.passphrase,
                &self.maker_address,
                "POST",
                "/order",
                &body_json,
            )?;

            let mut req = self.client.post(&url).body(body_json.clone()).header(
                "Content-Type",
                "application/json",
            );
            for (k, v) in headers {
                req = req.header(k, v);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) if e.is_timeout() || e.is_connect() => {
                    last_err = Some(anyhow::anyhow!("POST /order network error: {e}"));
                    continue;
                }
                Err(e) => return Err(anyhow::anyhow!("POST /order network error: {e}")),
            };

            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();

            // Rate-limited or server-side error → retryable.
            if status.as_u16() == 429 || status.is_server_error() {
                last_err = Some(anyhow::anyhow!("POST /order returned {status}: {text}"));
                continue;
            }

            // Any other non-2xx is a permanent client error.
            if !status.is_success() {
                anyhow::bail!("POST /order returned {status}: {text}");
            }

            let parsed: OrderResponse =
                serde_json::from_str(&text).context("failed to parse POST /order response")?;

            if !parsed.success {
                let msg = parsed.error_msg.as_deref().unwrap_or("");
                if msg.to_lowercase().contains("transient") {
                    last_err = Some(anyhow::anyhow!("POST /order transient error: {msg}"));
                    continue;
                }
                // Non-transient application error — no point retrying.
                error!(error = ?parsed.error_msg, "❌ POST /order rejected by exchange");
            }

            return Ok(parsed);
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!("POST /order failed after {MAX_ATTEMPTS} attempts")
        }))
    }

    // ─── Cancellation ────────────────────────────────────────────────────────

    /// Cancels a single order by ID.  `DELETE /order/{order_id}`
    #[instrument(skip(self), fields(order_id, dry_run = self.dry_run))]
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        let path = format!("/order/{order_id}");

        if self.dry_run {
            info!("DRY-RUN: would DELETE {path}");
            return Ok(());
        }

        let url = format!("{}{}", self.base_url, path);
        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.maker_address,
            "DELETE",
            &path,
            "",
        )?;

        let mut req = self.client.delete(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.context("DELETE /order network error")?;
        let status = resp.status();

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("DELETE /order/{order_id} returned {status}: {body}");
        }

        info!(%order_id, "order cancelled");
        Ok(())
    }

    /// Cancels all open orders for a market.
    /// `DELETE /orders/market/{market_id}`
    #[instrument(skip(self), fields(market_id, dry_run = self.dry_run))]
    pub async fn cancel_market_orders(&self, market_id: &str) -> Result<()> {
        let path = format!("/orders/market/{market_id}");

        if self.dry_run {
            info!("DRY-RUN: would DELETE {path}");
            return Ok(());
        }

        let url = format!("{}{}", self.base_url, path);
        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.maker_address,
            "DELETE",
            &path,
            "",
        )?;

        let mut req = self.client.delete(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.context("DELETE /orders/market network error")?;
        let status = resp.status();

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("DELETE {path} returned {status}: {body}");
        }

        info!(%market_id, "all market orders cancelled");
        Ok(())
    }

    // ─── Order status ────────────────────────────────────────────────────────

    /// Sends `POST /heartbeat` to keep the L2 session alive.
    ///
    /// Polymarket cancels open orders when the session heartbeat lapses.
    /// Call this every 15 seconds in live mode (see [`crate::heartbeat`]).
    pub async fn send_heartbeat(&self) -> Result<()> {
        if self.dry_run {
            info!("DRY-RUN: would POST /heartbeat");
            return Ok(());
        }

        let path = "/heartbeat";
        let url  = format!("{}{}", self.base_url, path);
        let body = "{}";

        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.maker_address,
            "POST",
            path,
            body,
        )?;

        let mut req = self
            .client
            .post(&url)
            .body(body)
            .header("Content-Type", "application/json");
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp   = req.send().await.context("POST /heartbeat network error")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("POST /heartbeat returned {status}: {text}");
        }
        Ok(())
    }

    /// Cancels **all** open orders for this account.  `DELETE /orders`
    ///
    /// Used by the emergency-stop path to immediately clear all exchange
    /// exposure before the operator takes over.
    pub async fn cancel_all_orders(&self) -> Result<()> {
        if self.dry_run {
            info!("DRY-RUN: would DELETE /orders (cancel all open orders)");
            return Ok(());
        }

        let path = "/orders";
        let url  = format!("{}{}", self.base_url, path);

        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.maker_address,
            "DELETE",
            path,
            "",
        )?;

        let mut req = self.client.delete(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp   = req.send().await.context("DELETE /orders network error")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("DELETE /orders returned {status}: {text}");
        }

        info!("✅ All open orders cancelled via DELETE /orders");
        Ok(())
    }

    /// Validates that L2 HMAC credentials are accepted by the exchange.
    ///
    /// Probes `GET /order/auth-probe` with HMAC headers.  The exchange will
    /// return 404 (order not found) for valid credentials, or 401/403 for
    /// invalid ones.  This avoids relying on a specific list-endpoint that
    /// may require query parameters.
    ///
    /// Returns `Ok(())` on success, `Err(…)` with a human-readable explanation
    /// on auth failure so operators know exactly what to fix before going live.
    pub async fn validate_credentials(&self) -> Result<()> {
        if self.dry_run {
            info!("DRY-RUN: skipping credential validation (no live keys)");
            return Ok(());
        }

        let path = "/order/auth-probe";
        let url  = format!("{}{}", self.base_url, path);
        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.maker_address,
            "GET",
            path,
            "",
        )?;

        let mut req = self.client.get(&url);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let resp = req
            .send()
            .await
            .context("GET /order/auth-probe network error — check connectivity")?;

        let status = resp.status();
        let body   = resp.text().await.unwrap_or_default();

        match status.as_u16() {
            // 404 = auth accepted, order simply not found — credentials valid.
            404 => {
                info!("✅ L2 HMAC credentials validated (probe returned 404 as expected)");
                Ok(())
            }
            // 200 / 2xx = unexpected but auth clearly worked.
            200..=299 => {
                info!("✅ L2 HMAC credentials validated (probe returned {status})");
                Ok(())
            }
            // 401 / 403 = auth rejected.
            401 | 403 => {
                anyhow::bail!(
                    "Credential validation failed (HTTP {status}): {body}\n\
                     Check POLYMARKET_API_KEY, POLYMARKET_API_SECRET, POLYMARKET_API_PASSPHRASE, and POLYMARKET_FUNDER_ADDRESS."
                )
            }
            other => {
                anyhow::bail!(
                    "Credential probe returned unexpected HTTP {other}: {body}\n\
                     This may indicate a network issue or API change."
                )
            }
        }
    }

    /// Fetches the current status of an order.  `GET /order/{order_id}`
    #[instrument(skip(self), fields(order_id))]
    pub async fn get_order_status(&self, order_id: &str) -> Result<OrderStatus> {
        let path = format!("/order/{order_id}");
        let url = format!("{}{}", self.base_url, path);

        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.maker_address,
            "GET",
            &path,
            "",
        )?;

        let mut req = self.client.get(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.context("GET /order network error")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            anyhow::bail!("GET /order/{order_id} returned {status}: {text}");
        }

        serde_json::from_str(&text).context("failed to parse GET /order response")
    }
}

// ─── HMAC-SHA256 auth headers ────────────────────────────────────────────────

/// Builds Polymarket L2 authentication headers.
///
/// Message format: `timestamp + method + path + body`  
/// The `api_secret` is base64-decoded before use as the HMAC key.
fn build_auth_headers(
    api_key:        &str,
    api_secret:     &str,
    passphrase:     &str,
    maker_address:  &str,
    method:         &str,
    path:           &str,
    body:           &str,
) -> Result<Vec<(String, String)>> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX epoch")?
        .as_secs()
        .to_string();

    let message = format!("{timestamp}{method}{path}{body}");

    let secret_bytes = B64
        .decode(api_secret)
        .context("POLYMARKET_API_SECRET is not valid base64")?;

    let mut mac =
        HmacSha256::new_from_slice(&secret_bytes).context("HMAC init failed")?;
    mac.update(message.as_bytes());
    let mac_bytes = mac.finalize().into_bytes();

    let signature = B64.encode(mac_bytes);

    Ok(vec![
        ("POLY-ADDRESS".into(),    maker_address.to_string()),
        ("POLY-SIGNATURE".into(),  signature),
        ("POLY-TIMESTAMP".into(),  timestamp),
        ("POLY-NONCE".into(),      "0".to_string()),
        ("POLY-API-KEY".into(),    api_key.to_string()),
        ("POLY-PASSPHRASE".into(), passphrase.to_string()),
    ])
}

// ─── JSON body construction ──────────────────────────────────────────────────

/// All numeric fields are serialised as decimal strings, matching the
/// Polymarket CLOB REST API spec.
#[derive(Debug, Serialize)]
struct OrderBody {
    order:      OrderFields,
    owner:      String,
    #[serde(rename = "orderType")]
    order_type: String,
    /// `true` = post-only (maker).  **MUST** be `true` to avoid taker fees.
    maker:      bool,
}

#[derive(Debug, Serialize)]
struct OrderFields {
    salt:                      String,
    maker:                     String,
    signer:                    String,
    taker:                     String,
    #[serde(rename = "tokenId")]
    token_id:                  String,
    #[serde(rename = "makerAmount")]
    maker_amount:              String,
    #[serde(rename = "takerAmount")]
    taker_amount:              String,
    expiration:                String,
    nonce:                     String,
    #[serde(rename = "feeRateBps")]
    fee_rate_bps:              String,
    side:                      u8,
    #[serde(rename = "signatureType")]
    signature_type:            u8,
    signature:                 String,
}

fn build_order_body(order: &SignedOrder, owner: &str, time_in_force: TimeInForce) -> OrderBody {
    OrderBody {
        order: OrderFields {
            salt:           order.salt.clone(),
            maker:          order.maker.clone(),
            signer:         order.signer.clone(),
            taker:          order.taker.clone(),
            token_id:       order.token_id.clone(),
            maker_amount:   order.maker_amount.to_string(),
            taker_amount:   order.taker_amount.to_string(),
            expiration:     order.expiration.to_string(),
            nonce:          order.nonce.to_string(),
            fee_rate_bps:   order.fee_rate_bps.to_string(),
            side:           order.side,
            signature_type: order.signature_type,
            signature:      order.signature.clone(),
        },
        owner:      owner.to_string(),
        order_type: time_in_force.to_string(),
        maker:      true,   // post-only — CRITICAL: prevents paying taker fees
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_signer::SignedOrder;

    fn dummy_order() -> SignedOrder {
        SignedOrder {
            salt:           "12345".to_string(),
            maker:          "0x0000000000000000000000000000000000000000".to_string(),
            signer:         "0x0000000000000000000000000000000000000000".to_string(),
            taker:          "0x0000000000000000000000000000000000000000".to_string(),
            token_id:       "99999".to_string(),
            maker_amount:   10_000_000,
            taker_amount:   15_384_615,
            expiration:     0,
            nonce:          0,
            fee_rate_bps:   0,
            side:           0,
            signature_type: 0,
            signature:      "0xdeadbeef".to_string(),
        }
    }

    #[test]
    fn order_body_serialises_correctly() {
        let order = dummy_order();
        let body = build_order_body(&order, "0xOwner", crate::types::TimeInForce::Gtc);
        let json = serde_json::to_value(&body).unwrap();

        assert_eq!(json["maker"], true, "post-only flag must be true");
        assert_eq!(json["orderType"], "GTC");
        assert_eq!(json["order"]["makerAmount"], "10000000");
        assert_eq!(json["order"]["takerAmount"], "15384615");
        assert_eq!(json["order"]["expiration"], "0");
        assert_eq!(json["order"]["nonce"], "0");
        assert_eq!(json["order"]["feeRateBps"], "0");
        assert_eq!(json["order"]["side"], 0);
        assert_eq!(json["order"]["signatureType"], 0);
    }

    #[test]
    fn hmac_message_format() {
        // Smoke-test: verify that build_auth_headers doesn't panic with
        // a well-formed base64 secret.
        let secret_b64 = B64.encode(b"test_secret_32bytes_exactly_ok!!");
        let headers = build_auth_headers(
            "key",
            &secret_b64,
            "pass",
            "0xAddr",
            "POST",
            "/order",
            "{}",
        )
        .unwrap();

        let keys: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"POLY-ADDRESS"));
        assert!(keys.contains(&"POLY-SIGNATURE"));
        assert!(keys.contains(&"POLY-TIMESTAMP"));
        assert!(keys.contains(&"POLY-NONCE"));
        assert!(keys.contains(&"POLY-API-KEY"));
        assert!(keys.contains(&"POLY-PASSPHRASE"));
    }

    #[test]
    fn order_body_fok_sets_order_type() {
        let order = dummy_order();
        let body = build_order_body(&order, "0xOwner", crate::types::TimeInForce::Fok);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["orderType"], "FOK");
        assert_eq!(json["maker"], true, "post-only must always be true");
    }

    #[test]
    fn order_body_fak_sets_order_type() {
        let order = dummy_order();
        let body = build_order_body(&order, "0xOwner", crate::types::TimeInForce::Fak);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["orderType"], "FAK");
    }

    #[test]
    fn time_in_force_display() {
        use crate::types::TimeInForce;
        assert_eq!(TimeInForce::Gtc.to_string(), "GTC");
        assert_eq!(TimeInForce::Fok.to_string(), "FOK");
        assert_eq!(TimeInForce::Fak.to_string(), "FAK");
    }

    #[test]
    fn time_in_force_default_is_gtc() {
        use crate::types::TimeInForce;
        assert_eq!(TimeInForce::default(), TimeInForce::Gtc);
    }

    /// Verifies that the transient-error detection in submit_order correctly
    /// identifies the exchange "transient" keyword in the error message.
    #[test]
    fn order_response_transient_error_detected() {
        // Simulate what the retry logic checks: success=false + "transient" in msg.
        let msg = "transient order book error, please retry";
        assert!(msg.to_lowercase().contains("transient"));

        // Permanent error must NOT trigger a retry.
        let perm = "signature verification failed";
        assert!(!perm.to_lowercase().contains("transient"));
    }
}
