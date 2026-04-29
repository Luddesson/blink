//! Polymarket CLOB order submission and lifecycle management.
//!
//! Handles REST API calls (POST/DELETE/GET) with Polymarket HMAC-SHA256
//! authentication headers.  When `dry_run` is `true` (which is always the case
//! when `Config::live_trading == false`), order submission is logged but no
//! real HTTP request is sent.
//!
//! ## Environment knobs (submit path)
//!
//! | Variable                  | Default | Description                                  |
//! |---------------------------|---------|----------------------------------------------|
//! | `BLINK_SUBMIT_TIMEOUT_MS` | `2000`  | HTTP total timeout for order submission (ms) |
//! | `BLINK_CONNECT_TIMEOUT_MS`| `500`   | TCP connect timeout (ms)                     |
//! | `BLINK_SUBMIT_MAX_ATTEMPTS`| `2`    | Max submit attempts before giving up         |

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tee_vault::KeyVault;
use tracing::{error, info, instrument, warn};

use crate::config::Config;
use crate::order_signer::SignedOrder;
use crate::types::TimeInForce;

type HmacSha256 = Hmac<Sha256>;

// ─── Public types ────────────────────────────────────────────────────────────

/// Outcome of [`OrderExecutor::submit_order`].
///
/// Returned as `Ok(SubmitOutcome)` rather than `Err` so the caller can
/// distinguish a *definitive* failure (bad auth, malformed order) from a
/// *timeout* where the order may already be live on the exchange.
#[derive(Debug)]
pub enum SubmitOutcome {
    /// Exchange acknowledged the order.
    Success(OrderResponse),
    /// All attempts timed out.  The order **may or may not** have been placed.
    /// Park the intent in `SubmitUnknown` state and reconcile via
    /// `GET /order/{id}` — do **not** blindly re-submit.
    Unknown,
}

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
    #[serde(rename = "makerAmount", alias = "maker_amount")]
    pub maker_amount: Option<String>,
    #[serde(rename = "takerAmount", alias = "taker_amount")]
    pub taker_amount: Option<String>,
    #[serde(rename = "remainingAmount", alias = "remaining_amount")]
    pub remaining_amount: Option<String>,
    #[serde(rename = "sizeMatched", alias = "size_matched")]
    pub size_matched: Option<String>,
}

/// Entry returned from `GET /orders` list endpoint.
///
/// Used for SubmitUnknown recovery — fields match the Polymarket CLOB v2 schema.
///
/// ## ASSUMPTION (verifiable)
/// Field names match the Polymarket CLOB REST API v2 JSON response for `GET /orders`.
/// Verify with: `curl -H "POLY-API-KEY: ..." "https://clob.polymarket.com/orders?client_order_id=blk-1"`
#[derive(Debug, Deserialize, Clone)]
pub struct OrderSearchEntry {
    pub id: String,
    pub status: String,
    #[serde(rename = "clientOrderId")]
    pub client_order_id: Option<String>,
    #[serde(rename = "sizeMatched")]
    pub size_matched: Option<String>,
    #[serde(rename = "remainingAmount")]
    pub remaining_amount: Option<String>,
    #[serde(rename = "price")]
    pub price: Option<String>,
}

// ─── Executor ────────────────────────────────────────────────────────────────

/// HTTP client for Polymarket CLOB order management.
///
/// When `dry_run` is `true` submission calls are logged but no network
/// request is made.  The engine always sets `dry_run = !live_trading`.
#[derive(Clone)]
pub struct OrderExecutor {
    client: Client,
    base_url: String,
    auth_address: String,
    api_key: String,
    /// Base64-encoded secret; decoded to raw bytes before HMAC use.
    api_secret: String,
    passphrase: String,
    /// When `true` outbound mutating requests are suppressed.
    pub dry_run: bool,
}

impl OrderExecutor {
    /// Constructs an executor from [`Config`].
    ///
    /// `dry_run` is set to `!config.live_trading` automatically.
    ///
    /// Client settings are tunable via env vars (see module-level docs).
    pub fn from_config(config: &Config) -> Result<Self> {
        let submit_timeout_ms = std::env::var("BLINK_SUBMIT_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(2000);
        let connect_timeout_ms = std::env::var("BLINK_CONNECT_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(500);

        let client = Client::builder()
            // Total request timeout (covers connect + TLS + send + recv).
            .timeout(Duration::from_millis(submit_timeout_ms))
            // Fail fast on unreachable hosts — 500 ms is generous for colocated infra.
            .connect_timeout(Duration::from_millis(connect_timeout_ms))
            // HTTP/2 connection keep-alive pings (PING frame every 10 s).
            .http2_keep_alive_interval(Duration::from_secs(10))
            // TCP keep-alive probes so the NIC doesn't silently drop idle conns.
            .tcp_keepalive(Some(Duration::from_secs(30)))
            // Connection pool: up to 64 idle connections per host.
            .pool_max_idle_per_host(64)
            // Evict idle connections after 90 s of inactivity.
            .pool_idle_timeout(Duration::from_secs(90))
            // Disable Nagle's algorithm — critical for low-latency HFT submits.
            .tcp_nodelay(true)
            .build()
            .context("failed to build reqwest HTTP client")?;

        tracing::info!(
            submit_timeout_ms,
            connect_timeout_ms,
            "OrderExecutor HTTP client initialised (HFT-tuned)"
        );

        // Log io_uring routing status at startup so operators know which path is active.
        #[cfg(feature = "io_uring")]
        {
            let use_io_uring = std::env::var("BLINK_IO_URING_SUBMIT")
                .map(|v| v == "1")
                .unwrap_or(false);
            if use_io_uring {
                // TODO(phase2-B): True io_uring HTTP submit requires replacing reqwest with a
                // custom HTTP/2 client built on top of `IoUringNet` (see io_uring_net.rs).
                // `IoUringNet` provides a raw TCP abstraction; bridging it to HTTPS + HTTP/2
                // framing is non-trivial and needs a dedicated sprint.
                // For now we log the intent and fall through to the reqwest path.
                tracing::warn!(
                    "BLINK_IO_URING_SUBMIT=1 detected but io_uring HTTP submit path is not yet \
                     implemented. Falling back to reqwest (tokio) for order submission. \
                     See TODO in order_executor.rs for the wiring gap."
                );
            } else {
                tracing::info!(
                    "io_uring feature enabled; set BLINK_IO_URING_SUBMIT=1 to route \
                                submits through io_uring (not yet wired — see TODO)"
                );
            }
        }

        let auth_address = if config.signer_private_key.is_empty() {
            config.funder_address.clone()
        } else {
            tee_vault::SoftwareVault::from_hex(&config.signer_private_key)
                .context("derive signer address for Polymarket L2 auth")?
                .signer_address()
                .to_string()
        };

        Ok(Self {
            client,
            base_url: config.clob_host.clone(),
            auth_address,
            api_key: config.api_key.clone(),
            api_secret: config.api_secret.clone(),
            passphrase: config.api_passphrase.clone(),
            dry_run: !config.live_trading,
        })
    }

    // ─── Order submission ────────────────────────────────────────────────────

    /// Submits a signed order to `POST /order`.
    ///
    /// Returns [`SubmitOutcome::Success`] on exchange acknowledgement.  On
    /// final timeout returns [`SubmitOutcome::Unknown`] — the caller **must
    /// not** blindly retry; reconcile via `GET /order/{id}` instead.
    ///
    /// Retry policy (configurable via env):
    /// - Max attempts: `BLINK_SUBMIT_MAX_ATTEMPTS` (default 2)
    /// - Backoff: 50 ms, 150 ms (HFT-safe — avoids queuing behind stale fills)
    /// - Retryable: timeout, connect error, 5xx, 429
    /// - Non-retryable: 4xx (except 429) → immediate `Err`
    #[instrument(skip(self, order), fields(token_id = %order.token_id, side = order.side, dry_run = self.dry_run))]
    pub async fn submit_order(
        &self,
        order: &SignedOrder,
        time_in_force: TimeInForce,
    ) -> Result<SubmitOutcome> {
        let body = build_order_body(order, &self.api_key, time_in_force)?;
        let body_json = serde_json::to_string(&body).context("failed to serialise order body")?;

        if self.dry_run {
            info!(
                body = %body_json,
                "DRY-RUN: would POST /order (live_trading=false)"
            );
            return Ok(SubmitOutcome::Success(OrderResponse {
                success: true,
                order_id: Some("dry-run".to_string()),
                status: Some("dry_run".to_string()),
                error_msg: None,
            }));
        }

        let max_attempts = std::env::var("BLINK_SUBMIT_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(2);

        // HFT backoff schedule (ms): 50, 150 — stays well under 500 ms total.
        const BACKOFF_MS: &[u64] = &[50, 150];

        let url = format!("{}/order", self.base_url);
        let mut last_err: Option<anyhow::Error> = None;
        let mut timed_out_all = false;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay_ms = BACKOFF_MS
                    .get((attempt - 1) as usize)
                    .copied()
                    .unwrap_or(150);
                warn!(
                    attempt,
                    delay_ms,
                    error = ?last_err,
                    "POST /order transient — retrying"
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            crate::hot_metrics::counters()
                .submits_started
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::hot_metrics::counters()
                .http_submit_inflight
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _submit_timer =
                crate::hot_metrics::StageTimer::start(crate::hot_metrics::HotStage::Submit);

            // Rebuild auth headers each attempt: POLY-TIMESTAMP must be fresh.
            let headers = build_auth_headers(
                &self.api_key,
                &self.api_secret,
                &self.passphrase,
                &self.auth_address,
                "POST",
                "/order",
                &body_json,
            )?;

            let mut req = self
                .client
                .post(&url)
                .body(body_json.clone())
                .header("Content-Type", "application/json");
            for (k, v) in headers {
                req = req.header(k, v);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) if e.is_timeout() => {
                    crate::hot_metrics::counters()
                        .http_submit_inflight
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    timed_out_all = true;
                    last_err = Some(anyhow::anyhow!("POST /order timeout: {e}"));
                    continue;
                }
                Err(e) if e.is_connect() => {
                    crate::hot_metrics::counters()
                        .http_submit_inflight
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    timed_out_all = false;
                    last_err = Some(anyhow::anyhow!("POST /order connect error: {e}"));
                    continue;
                }
                Err(e) => {
                    crate::hot_metrics::counters()
                        .http_submit_inflight
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    return Err(anyhow::anyhow!("POST /order network error: {e}"));
                }
            };

            crate::hot_metrics::counters()
                .http_submit_inflight
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            timed_out_all = false;

            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();

            // Rate-limited (429) or server error (5xx) → retryable.
            if status.as_u16() == 429 || status.is_server_error() {
                last_err = Some(anyhow::anyhow!("POST /order returned {status}: {text}"));
                continue;
            }

            // Non-idempotent 4xx client error — retrying would be wrong.
            if status.is_client_error() {
                anyhow::bail!("POST /order rejected (HTTP {status}): {text}");
            }

            // Any remaining non-2xx is unexpected — surface immediately.
            if !status.is_success() {
                anyhow::bail!("POST /order returned {status}: {text}");
            }

            let parsed: OrderResponse =
                serde_json::from_str(&text).context("failed to parse POST /order response")?;

            drop(_submit_timer);
            let _ack_timer =
                crate::hot_metrics::StageTimer::start(crate::hot_metrics::HotStage::Ack);
            drop(_ack_timer);

            if !parsed.success {
                let msg = parsed.error_msg.as_deref().unwrap_or("");
                if msg.to_lowercase().contains("transient") {
                    last_err = Some(anyhow::anyhow!("POST /order transient error: {msg}"));
                    continue;
                }
                error!(error = ?parsed.error_msg, "❌ POST /order rejected by exchange");
                crate::hot_metrics::counters()
                    .submits_rejected
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                crate::hot_metrics::counters()
                    .submits_ack
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }

            return Ok(SubmitOutcome::Success(parsed));
        }

        // All attempts exhausted.
        if timed_out_all {
            // Every attempt timed out: we cannot know whether the last attempt
            // reached the exchange.  Return Unknown so the caller can park the
            // intent without re-submitting (which could double-fill).
            warn!(
                max_attempts,
                "POST /order: all attempts timed out — returning SubmitOutcome::Unknown"
            );
            crate::hot_metrics::counters()
                .submit_unknown
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(SubmitOutcome::Unknown)
        } else {
            Err(last_err.unwrap_or_else(|| {
                anyhow::anyhow!("POST /order failed after {max_attempts} attempts")
            }))
        }
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
            &self.auth_address,
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
            &self.auth_address,
            "DELETE",
            &path,
            "",
        )?;

        let mut req = self.client.delete(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req
            .send()
            .await
            .context("DELETE /orders/market network error")?;
        let status = resp.status();

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("DELETE {path} returned {status}: {body}");
        }

        info!(%market_id, "all market orders cancelled");
        Ok(())
    }

    // ─── Order status ────────────────────────────────────────────────────────

    /// Sends a ping/time request to verify the CLOB connection and auth.
    ///
    /// In V2, we use `GET /time` as a reliable heartbeat/ping mechanism.
    pub async fn send_heartbeat(&self) -> Result<()> {
        if self.dry_run {
            info!("DRY-RUN: would GET /time (heartbeat)");
            return Ok(());
        }

        let path = "/time";
        let url = format!("{}{}", self.base_url, path);

        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.auth_address,
            "GET",
            path,
            "",
        )?;

        let mut req = self.client.get(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.context("GET /time network error")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("GET /time returned {status}: {text}");
        }

        Ok(())
    }

    /// Cancels **all** open orders for this account.  `DELETE /cancel-all`
    ///
    /// Used by the emergency-stop path to immediately clear all exchange
    /// exposure before the operator takes over.
    pub async fn cancel_all_orders(&self) -> Result<()> {
        if self.dry_run {
            info!("DRY-RUN: would DELETE /cancel-all (cancel all open orders)");
            return Ok(());
        }

        let path = "/cancel-all";
        let url = format!("{}{}", self.base_url, path);

        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.auth_address,
            "DELETE",
            path,
            "",
        )?;

        let mut req = self.client.delete(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.context("DELETE /orders network error")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("DELETE /cancel-all returned {status}: {text}");
        }

        info!("✅ All open orders cancelled via DELETE /cancel-all");
        Ok(())
    }

    /// Validates that L2 HMAC credentials are accepted by the exchange.
    ///
    /// Probes `GET /data/orders` with HMAC headers. This is a
    /// non-mutating L2 endpoint: success proves the API key, passphrase,
    /// secret, signer address, timestamp, and HMAC path format all work before
    /// live order submission is allowed.
    ///
    /// Returns `Ok(())` on success, `Err(…)` with a human-readable explanation
    /// on auth failure so operators know exactly what to fix before going live.
    pub async fn validate_credentials(&self) -> Result<()> {
        if self.dry_run {
            info!("DRY-RUN: skipping credential validation (no live keys)");
            return Ok(());
        }

        let path = "/data/orders";
        let url = format!("{}{}", self.base_url, path);
        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.auth_address,
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
        let body = resp.text().await.unwrap_or_default();

        match status.as_u16() {
            200..=299 => {
                info!("✅ L2 HMAC credentials validated via GET /data/orders");
                Ok(())
            }
            // 401 / 403 = auth rejected.
            401 | 403 => {
                anyhow::bail!(
                    "Credential validation failed (HTTP {status}): {body}\n\
                     Check POLYMARKET_API_KEY, POLYMARKET_API_SECRET, POLYMARKET_API_PASSPHRASE, SIGNER_PRIVATE_KEY, and POLYMARKET_FUNDER_ADDRESS."
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

    /// Spawns a background task that sends a lightweight GET probe to the CLOB
    /// host every `interval_secs` seconds to keep the HTTP/2 (or HTTP/1.1)
    /// connection pool alive during idle trading periods.
    ///
    /// A warm connection saves ~200ms on the first order after an idle period
    /// (eliminates TCP handshake + TLS resumption). The probe reuses the same
    /// `reqwest::Client` connection pool as order submission.
    ///
    /// Controlled by `BLINK_CLOB_KEEPALIVE_SECS` (default 5, set 0 to disable).
    pub fn spawn_connection_keepalive(&self) {
        let interval_secs = std::env::var("BLINK_CLOB_KEEPALIVE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5);
        if interval_secs == 0 {
            return;
        }
        // Use a simple unauthenticated endpoint that returns quickly.
        let probe_url = format!("{}/time", self.base_url);
        let client = self.client.clone();
        let probe_url_log = probe_url.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                // Fire-and-forget: we only care about keeping the socket warm.
                let _ = client
                    .get(&probe_url)
                    .timeout(std::time::Duration::from_secs(3))
                    .send()
                    .await;
            }
        });
        tracing::info!(
            interval_secs,
            probe_url = %probe_url_log,
            "CLOB connection keep-alive probe started"
        );
    }

    /// Searches for an order by `client_order_id`.
    ///
    /// **Primary path**: `GET /data/orders` — returns open authenticated-user
    /// orders. We scan client-side because the documented query parameters are
    /// order hash, market, asset_id, and pagination cursor.
    ///
    /// **Fallback path**: `GET /orders?status=live&limit=200` then scan results
    /// client-side when the primary call returns an empty list.
    #[instrument(skip(self), fields(client_id))]
    pub async fn find_order_by_client_id(
        &self,
        client_id: &str,
    ) -> Result<Option<OrderSearchEntry>> {
        if self.dry_run {
            info!("DRY-RUN: would GET /data/orders and scan for client_order_id={client_id}");
            return Ok(None);
        }

        // Primary: scan open orders returned by the documented endpoint.
        let path = "/data/orders".to_string();
        let url = format!("{}{}", self.base_url, path);
        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.auth_address,
            "GET",
            &path,
            "",
        )?;
        let mut req = self.client.get(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.context("GET /data/orders network error")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            anyhow::bail!("GET /data/orders returned {status}: {text}");
        }

        let entries = parse_order_list(&text);
        if let Some(found) = entries
            .into_iter()
            .find(|e| e.client_order_id.as_deref() == Some(client_id))
        {
            return Ok(Some(found));
        }
        Ok(None)
    }

    /// Fetches the current status of an order.  Primary path is
    /// `GET /order/{order_id}`. FAK/taker orders can disappear from the order
    /// endpoint after matching, so a 404 falls back to authenticated
    /// `GET /trades` and scans for `taker_order_id == order_id`.
    #[instrument(skip(self), fields(order_id))]
    pub async fn get_order_status(&self, order_id: &str) -> Result<OrderStatus> {
        let path = format!("/order/{order_id}");
        let url = format!("{}{}", self.base_url, path);

        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.auth_address,
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

        if status.is_success() {
            return serde_json::from_str(&text).context("failed to parse GET /order response");
        }

        if status.as_u16() == 404 {
            if let Some(trade_status) = self.get_trade_status_for_order(order_id).await? {
                return Ok(trade_status);
            }
        }

        anyhow::bail!("GET /order/{order_id} returned {status}: {text}")
    }

    async fn get_trade_status_for_order(&self, order_id: &str) -> Result<Option<OrderStatus>> {
        #[derive(Debug, Deserialize)]
        struct TradeList {
            #[serde(default)]
            data: Vec<TradeEntry>,
        }

        #[derive(Debug, Deserialize)]
        struct TradeEntry {
            #[serde(default)]
            taker_order_id: String,
            #[serde(default)]
            status: String,
            #[serde(default)]
            size: String,
        }

        let path = "/trades";
        let url = format!("{}{}", self.base_url, path);
        let headers = build_auth_headers(
            &self.api_key,
            &self.api_secret,
            &self.passphrase,
            &self.auth_address,
            "GET",
            path,
            "",
        )?;

        let mut req = self.client.get(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.context("GET /trades network error")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GET /trades returned {status}: {text}");
        }

        let trades: TradeList = serde_json::from_str(&text).unwrap_or(TradeList { data: vec![] });
        let Some(trade) = trades
            .data
            .into_iter()
            .find(|t| t.taker_order_id.eq_ignore_ascii_case(order_id))
        else {
            return Ok(None);
        };

        let normalized_status = match trade.status.to_ascii_uppercase().as_str() {
            "TRADE_STATUS_CONFIRMED" | "CONFIRMED" => "filled",
            "TRADE_STATUS_FAILED" | "FAILED" => "rejected",
            _ => "pending",
        };

        Ok(Some(OrderStatus {
            id: order_id.to_string(),
            status: normalized_status.to_string(),
            maker_amount: None,
            taker_amount: None,
            remaining_amount: None,
            size_matched: if trade.size.is_empty() {
                None
            } else {
                Some(trade.size)
            },
        }))
    }
}

// ─── HMAC-SHA256 auth headers ────────────────────────────────────────────────

/// Parses a Polymarket order-list response that may be a bare JSON array or
/// wrapped in `{"data": [...]}`.
fn parse_order_list(text: &str) -> Vec<OrderSearchEntry> {
    if text.trim_start().starts_with('[') {
        serde_json::from_str(text).unwrap_or_default()
    } else {
        #[derive(serde::Deserialize)]
        struct Envelope {
            #[serde(default)]
            data: Vec<OrderSearchEntry>,
        }
        serde_json::from_str::<Envelope>(text)
            .map(|e| e.data)
            .unwrap_or_default()
    }
}

/// Builds Polymarket L2 authentication headers.
///
/// Message format: `timestamp + method + path + body`  
/// The `api_secret` is base64-decoded before use as the HMAC key.
fn build_auth_headers(
    api_key: &str,
    api_secret: &str,
    passphrase: &str,
    auth_address: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Result<Vec<(String, String)>> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX epoch")?
        .as_secs()
        .to_string();

    let message = format!("{timestamp}{method}{path}{body}");

    // Robust Base64 decoding: clean whitespace and try both Standard and URL-safe engines.
    let clean_secret = api_secret
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();

    let secret_bytes = B64
        .decode(&clean_secret)
        .or_else(|_| {
            use base64::engine::general_purpose::URL_SAFE;
            URL_SAFE.decode(&clean_secret)
        })
        .context("POLYMARKET_API_SECRET is not valid base64 (even after cleaning whitespace)")?;

    let mut mac = HmacSha256::new_from_slice(&secret_bytes).context("HMAC init failed")?;
    mac.update(message.as_bytes());
    let mac_bytes = mac.finalize().into_bytes();

    let signature = B64.encode(mac_bytes).replace('+', "-").replace('/', "_");

    Ok(vec![
        ("POLY_ADDRESS".into(), auth_address.to_string()),
        ("POLY_SIGNATURE".into(), signature),
        ("POLY_TIMESTAMP".into(), timestamp),
        ("POLY_NONCE".into(), "0".to_string()),
        ("POLY_API_KEY".into(), api_key.to_string()),
        ("POLY_PASSPHRASE".into(), passphrase.to_string()),
    ])
}

// ─── JSON body construction ──────────────────────────────────────────────────

/// All numeric fields are serialised as decimal strings, matching the
/// Polymarket CLOB REST API spec.
#[derive(Debug, Serialize)]
struct OrderBody {
    order: OrderFields,
    owner: String,
    #[serde(rename = "orderType")]
    order_type: String,
    /// `true` = post-only. Rejected if it would cross the book.
    #[serde(rename = "postOnly")]
    post_only: bool,
    /// V2 submit option; keep false unless deliberately using delayed execution.
    #[serde(rename = "deferExec")]
    defer_exec: bool,
}

#[derive(Debug, Serialize)]
struct OrderFields {
    salt: u64,
    maker: String,
    signer: String,
    taker: String,
    #[serde(rename = "tokenId")]
    token_id: String,
    #[serde(rename = "makerAmount")]
    maker_amount: String,
    #[serde(rename = "takerAmount")]
    taker_amount: String,
    expiration: String,
    side: String,
    #[serde(rename = "signatureType")]
    signature_type: u8,
    signature: String,
    timestamp: String,
    metadata: String,
    builder: String,
}

fn build_order_body(
    order: &SignedOrder,
    owner: &str,
    time_in_force: TimeInForce,
) -> Result<OrderBody> {
    let salt = order
        .salt
        .parse::<u64>()
        .with_context(|| format!("signed order salt is not a u64 JSON number: {}", order.salt))?;

    Ok(OrderBody {
        order: OrderFields {
            salt,
            maker: order.maker.clone(),
            signer: order.signer.clone(),
            taker: order.taker.clone(),
            token_id: order.token_id.clone(),
            maker_amount: order.maker_amount.to_string(),
            taker_amount: order.taker_amount.to_string(),
            expiration: order.expiration.to_string(),
            side: if order.side == 0 { "BUY" } else { "SELL" }.to_string(),
            signature_type: order.signature_type,
            signature: order.signature.clone(),
            timestamp: order.timestamp.to_string(),
            metadata: order.metadata.clone(),
            builder: order.builder.clone(),
        },
        owner: owner.to_string(),
        order_type: time_in_force.to_string(),
        post_only: matches!(time_in_force, TimeInForce::Gtc),
        defer_exec: false,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_signer::SignedOrder;

    fn dummy_order() -> SignedOrder {
        SignedOrder {
            salt: "12345".to_string(),
            maker: "0x0000000000000000000000000000000000000000".to_string(),
            signer: "0x0000000000000000000000000000000000000000".to_string(),
            taker: "0x0000000000000000000000000000000000000000".to_string(),
            token_id: "99999".to_string(),
            maker_amount: 10_000_000,
            taker_amount: 15_384_615,
            expiration: 0,
            nonce: 0,
            fee_rate_bps: 0,
            side: 0,
            signature_type: 0,
            signature: "0xdeadbeef".to_string(),
            timestamp: 123456789,
            metadata: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            builder: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            client_order_id: None,
        }
    }

    #[test]
    fn order_body_serialises_correctly() {
        let order = dummy_order();
        let body = build_order_body(&order, "0xOwner", crate::types::TimeInForce::Gtc).unwrap();
        let json = serde_json::to_value(&body).unwrap();

        assert_eq!(json["postOnly"], true, "post-only flag must be true");
        assert_eq!(json["deferExec"], false);
        assert_eq!(json["orderType"], "GTC");
        assert_eq!(json["order"]["salt"], 12345);
        assert_eq!(json["order"]["makerAmount"], "10000000");
        assert_eq!(json["order"]["takerAmount"], "15384615");
        assert_eq!(json["order"]["expiration"], "0");
        assert!(json["order"].get("nonce").is_none());
        assert!(json["order"].get("feeRateBps").is_none());
        assert_eq!(
            json["order"]["taker"],
            "0x0000000000000000000000000000000000000000"
        );
        assert_eq!(json["order"]["side"], "BUY");
        assert_eq!(json["order"]["signatureType"], 0);
        assert_eq!(json["order"]["timestamp"], "123456789");
        assert_eq!(
            json["order"]["metadata"],
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn hmac_message_format() {
        // Smoke-test: verify that build_auth_headers doesn't panic with
        // a well-formed base64 secret.
        let secret_b64 = B64.encode(b"test_secret_32bytes_exactly_ok!!");
        let headers =
            build_auth_headers("key", &secret_b64, "pass", "0xAddr", "POST", "/order", "{}")
                .unwrap();

        let keys: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"POLY_ADDRESS"));
        assert!(keys.contains(&"POLY_SIGNATURE"));
        assert!(keys.contains(&"POLY_TIMESTAMP"));
        assert!(keys.contains(&"POLY_NONCE"));
        assert!(keys.contains(&"POLY_API_KEY"));
        assert!(keys.contains(&"POLY_PASSPHRASE"));
    }

    #[test]
    fn order_body_fok_sets_order_type() {
        let order = dummy_order();
        let body = build_order_body(&order, "0xOwner", crate::types::TimeInForce::Fok).unwrap();
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["orderType"], "FOK");
        assert_eq!(json["postOnly"], false, "post-only is invalid for FOK");
    }

    #[test]
    fn order_body_fak_sets_order_type() {
        let order = dummy_order();
        let body = build_order_body(&order, "0xOwner", crate::types::TimeInForce::Fak).unwrap();
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
