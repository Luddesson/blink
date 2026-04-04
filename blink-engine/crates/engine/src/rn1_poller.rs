//! RN1 wallet trade poller — REST-based detection of the tracked wallet's trades.
//!
//! Polls the **public** Polymarket Data API at
//! `https://data-api.polymarket.com/activity?user={RN1_WALLET}` to detect new
//! trades.  No authentication required.
//!
//! Each new trade is converted to an [`RN1Signal`] and forwarded to the paper
//! (or live) engine via the signal channel.

use std::collections::HashSet;
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::activity_log::{push as log_push, ActivityLog, EntryKind};
use crate::config::Config;
use crate::types::{OrderSide, RN1Signal};

/// How often to poll for new RN1 trades.
const POLL_INTERVAL: Duration = Duration::from_millis(1200);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(2000);
const ERROR_BACKOFF_MAX: Duration = Duration::from_secs(5);
const CIRCUIT_BREAKER_THRESHOLD: u32 = 10;
const CIRCUIT_BREAKER_COOLDOWN: Duration = Duration::from_secs(30);
const METRICS_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// Maximum trades to request per poll.
const POLL_LIMIT: u32 = 20;

/// Polymarket public data API.
const DATA_API: &str = "https://data-api.polymarket.com";

#[derive(Debug, Clone, Default)]
pub struct Rn1PollDiagnostics {
    pub consecutive_errors: u32,
    pub total_polls: u64,
    pub total_signals: u64,
    pub last_success_unix_ms: i64,
    pub last_error_at_unix_ms: i64,
    pub last_error: Option<String>,
    pub last_http_status: Option<u16>,
    pub last_content_type: Option<String>,
    pub last_body_preview: Option<String>,
}

pub type Rn1PollDiagnosticsHandle = Arc<Mutex<Rn1PollDiagnostics>>;

#[derive(Debug, Clone)]
struct PollFailure {
    message: String,
    http_status: Option<u16>,
    content_type: Option<String>,
    body_preview: Option<String>,
}

fn classify_reqwest_error(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_body() {
        "body"
    } else if e.is_decode() {
        "decode"
    } else if e.is_request() {
        "request"
    } else {
        "other"
    }
}

fn reqwest_source_chain(mut source: Option<&(dyn StdError + 'static)>) -> String {
    let mut parts: Vec<String> = Vec::new();
    while let Some(err) = source {
        parts.push(err.to_string());
        source = err.source();
        if parts.len() >= 4 {
            break;
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" source=[{}]", parts.join(" | "))
    }
}

// ─── Data API response types ─────────────────────────────────────────────────

/// A single activity entry from the Data API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ActivityEntry {
    /// Transaction hash — unique per trade.
    #[serde(default)]
    transaction_hash: Option<String>,
    /// Condition ID of the market.
    #[serde(default)]
    condition_id: Option<String>,
    /// Token ID (asset).
    #[serde(default)]
    asset: Option<String>,
    /// "BUY" or "SELL".
    #[serde(default)]
    side: Option<String>,
    /// Execution price (float as number, e.g. 0.52).
    #[serde(default)]
    price: Option<f64>,
    /// Token size (float as number).
    #[serde(default)]
    size: Option<f64>,
    /// USDC notional value.
    #[serde(default)]
    usdc_size: Option<f64>,
    /// Unix epoch seconds.
    #[serde(default)]
    timestamp: Option<i64>,
    /// Human-readable market title.
    #[serde(default)]
    title: Option<String>,
    /// Market slug.
    #[serde(default)]
    slug: Option<String>,
    /// Outcome label (e.g. "Yes", "No", team name).
    #[serde(default)]
    outcome: Option<String>,
    /// Entry type, e.g. "TRADE".
    #[serde(default, rename = "type")]
    entry_type: Option<String>,
}

// ─── Poller ──────────────────────────────────────────────────────────────────

/// Polls the public Polymarket Data API for the RN1 wallet's trades.
#[allow(unused_assignments)]
pub async fn run_rn1_poller(
    config: Arc<Config>,
    signal_tx: crossbeam_channel::Sender<RN1Signal>,
    activity: Option<ActivityLog>,
    diagnostics: Rn1PollDiagnosticsHandle,
) {
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .http1_only()
        .pool_max_idle_per_host(0) // fresh TCP conn each poll — avoids Cloudflare RST on stale sockets
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent("blink-engine-rn1-poller/1.0")
        .build()
        .expect("reqwest client");

    let mut seen_hashes: HashSet<String> = HashSet::with_capacity(256);
    let mut consecutive_errors: u32 = 0;
    let mut total_signals: u64 = 0;
    let mut total_polls: u64 = 0;
    let mut total_entries_seen: u64 = 0;
    let mut idle_cycles: u32 = 0;
    let mut last_ts: i64 = 0; // track newest timestamp to seed on first poll
    let mut next_interval = POLL_INTERVAL;
    let mut metrics_last_logged = Instant::now();

    let rn1_short = if config.rn1_wallet.len() >= 10 {
        &config.rn1_wallet[..10]
    } else {
        &config.rn1_wallet
    };

    info!(rn1_wallet = %config.rn1_wallet, "RN1 poller started (data-api, no auth) — every {}ms", POLL_INTERVAL.as_millis());
    if let Some(ref log) = activity {
        log_push(
            log,
            EntryKind::Engine,
            format!(
                "RN1 poller started — tracking {rn1_short}… every {}ms (public API)",
                POLL_INTERVAL.as_millis()
            ),
        );
    }

    loop {
        let poll_start = Instant::now();
        total_polls += 1;
        {
            let mut d = diagnostics.lock().unwrap();
            d.total_polls = total_polls;
        }

        match poll_activity_with_retry(&client, &config.rn1_wallet).await {
            Ok(entries) => {
                consecutive_errors = 0;
                total_entries_seen += entries.len() as u64;
                let mut new_signals_this_cycle = 0_u64;

                // On first successful poll, seed seen_hashes with existing trades
                // so we don't fire signals for old trades.
                let is_first = seen_hashes.is_empty() && last_ts == 0;

                for entry in &entries {
                    let hash = match &entry.transaction_hash {
                        Some(h) => h.clone(),
                        None => continue,
                    };

                    // Skip non-trade entries.
                    if entry.entry_type.as_deref() != Some("TRADE") {
                        continue;
                    }

                    // Track timestamp high-water mark.
                    if let Some(ts) = entry.timestamp {
                        if ts > last_ts {
                            last_ts = ts;
                        }
                    }

                    if seen_hashes.contains(&hash) {
                        continue;
                    }
                    seen_hashes.insert(hash.clone());

                    // On first poll, just seed — don't emit signals for old trades.
                    if is_first {
                        continue;
                    }

                    let side_str = entry.side.as_deref().unwrap_or("BUY");
                    let side = match side_str.to_uppercase().as_str() {
                        "BUY" => OrderSide::Buy,
                        "SELL" => OrderSide::Sell,
                        _ => {
                            debug!(side = side_str, "unknown trade side — skipping");
                            continue;
                        }
                    };

                    let token_id = entry.asset.as_deref().unwrap_or("unknown").to_string();
                    let price = entry.price.unwrap_or(0.0);
                    let size = entry.size.unwrap_or(0.0);
                    let title = entry.title.as_deref().unwrap_or("?");
                    let outcome = entry.outcome.as_deref().unwrap_or("?");

                    let signal = RN1Signal {
                        token_id: token_id.clone(),
                        market_title: entry.title.clone(),
                        market_outcome: entry.outcome.clone(),
                        side,
                        price: (price * 1000.0) as u64,
                        size: (size * 1000.0) as u64,
                        order_id: hash.clone(),
                        detected_at: Instant::now(),
                    };

                    total_signals += 1;
                    new_signals_this_cycle += 1;
                    info!(
                        tx = %hash,
                        title = %title,
                        outcome = %outcome,
                        side = %side,
                        price,
                        size,
                        "🚨 RN1 trade #{total_signals} detected"
                    );
                    if let Some(ref log) = activity {
                        log_push(log, EntryKind::Engine,
                            format!("🚨 RN1 {side} {outcome} @ {price:.2} ×{size:.0} — {title} (#{total_signals})"));
                    }

                    if let Err(e) = signal_tx.try_send(signal) {
                        warn!(error = %e, "signal channel full — RN1 signal dropped");
                    }
                }

                if is_first && !entries.is_empty() {
                    info!(
                        seeded = seen_hashes.len(),
                        "RN1 poller seeded — watching for new trades"
                    );
                    if let Some(ref log) = activity {
                        log_push(
                            log,
                            EntryKind::Engine,
                            format!(
                                "RN1 poller seeded ({} existing trades) — watching for new",
                                seen_hashes.len()
                            ),
                        );
                    }
                }

                debug!(
                    entries = entries.len(),
                    seen = seen_hashes.len(),
                    signals = total_signals,
                    ms = poll_start.elapsed().as_millis(),
                    "poll cycle"
                );

                if new_signals_this_cycle == 0 {
                    idle_cycles = idle_cycles.saturating_add(1);
                    if idle_cycles >= 5 {
                        next_interval = IDLE_POLL_INTERVAL;
                    } else {
                        next_interval = POLL_INTERVAL;
                    }
                } else {
                    idle_cycles = 0;
                    next_interval = POLL_INTERVAL;
                }
                {
                    let mut d = diagnostics.lock().unwrap();
                    d.consecutive_errors = 0;
                    d.total_signals = total_signals;
                    d.last_success_unix_ms = Utc::now().timestamp_millis();
                    d.last_error = None;
                    d.last_http_status = None;
                    d.last_content_type = None;
                    d.last_body_preview = None;
                }
            }
            Err(err) => {
                consecutive_errors += 1;
                {
                    let mut d = diagnostics.lock().unwrap();
                    d.consecutive_errors = consecutive_errors;
                    d.last_error_at_unix_ms = Utc::now().timestamp_millis();
                    d.last_error = Some(err.message.clone());
                    d.last_http_status = err.http_status;
                    d.last_content_type = err.content_type.clone();
                    d.last_body_preview = err.body_preview.clone();
                }
                if consecutive_errors <= 3 || consecutive_errors % 60 == 0 {
                    error!(error = %err.message, n = consecutive_errors, "RN1 poll failed");
                    if let Some(ref log) = activity {
                        log_push(
                            log,
                            EntryKind::Warn,
                            format!("RN1 poll error (#{consecutive_errors}): {}", err.message),
                        );
                    }
                }

                if consecutive_errors >= CIRCUIT_BREAKER_THRESHOLD {
                    warn!(
                        errors = consecutive_errors,
                        cooldown_secs = CIRCUIT_BREAKER_COOLDOWN.as_secs(),
                        "RN1 poller circuit-breaker cooldown activated"
                    );
                    if let Some(ref log) = activity {
                        log_push(
                            log,
                            EntryKind::Warn,
                            format!(
                                "RN1 poller cooldown: {} consecutive errors, sleeping {}s",
                                consecutive_errors,
                                CIRCUIT_BREAKER_COOLDOWN.as_secs()
                            ),
                        );
                    }
                    tokio::time::sleep(CIRCUIT_BREAKER_COOLDOWN).await;
                    consecutive_errors = 0;
                    // Reset to normal interval after cooldown; `continue` skips
                    // the bottom-of-loop sleep so next iteration uses this value.
                    next_interval = POLL_INTERVAL; // read after `continue` at loop bottom
                    continue;
                }

                let backoff_ms = (POLL_INTERVAL.as_millis() as u64)
                    .saturating_mul(1_u64 << consecutive_errors.min(4))
                    .min(ERROR_BACKOFF_MAX.as_millis() as u64);
                next_interval = Duration::from_millis(backoff_ms);
            }
        }

        if metrics_last_logged.elapsed() >= METRICS_LOG_INTERVAL {
            info!(
                polls = total_polls,
                entries_seen = total_entries_seen,
                signals = total_signals,
                idle_cycles,
                interval_ms = next_interval.as_millis(),
                "RN1 poller metrics"
            );
            metrics_last_logged = Instant::now();
        }

        let elapsed = poll_start.elapsed();
        if elapsed < next_interval {
            tokio::time::sleep(next_interval - elapsed).await;
        }
    }
}

// ─── Single poll with retry ──────────────────────────────────────────────────

const POLL_RETRIES: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_millis(500);

async fn poll_activity_with_retry(
    client: &Client,
    wallet: &str,
) -> std::result::Result<Vec<ActivityEntry>, PollFailure> {
    let mut last_err = None;
    for attempt in 0..=POLL_RETRIES {
        match poll_activity(client, wallet).await {
            Ok(entries) => return Ok(entries),
            Err(e) => {
                last_err = Some(e);
                if attempt < POLL_RETRIES {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }
    Err(last_err.unwrap())
}

// ─── Single poll ─────────────────────────────────────────────────────────────

async fn poll_activity(
    client: &Client,
    wallet: &str,
) -> std::result::Result<Vec<ActivityEntry>, PollFailure> {
    fn preview_body(body: &str) -> String {
        body.chars()
            .take(180)
            .collect::<String>()
            .replace('\n', " ")
            .replace('\r', " ")
    }

    let url = format!("{DATA_API}/activity?user={wallet}&limit={POLL_LIMIT}");

    let resp = client
        .get(&url)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| PollFailure {
            message: format!(
                "data-api request failed kind={} err={}{}",
                classify_reqwest_error(&e),
                e,
                reqwest_source_chain(e.source())
            ),
            http_status: None,
            content_type: None,
            body_preview: None,
        })?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(PollFailure {
            message: format!("data-api /activity returned {}", status.as_u16()),
            http_status: Some(status.as_u16()),
            content_type,
            body_preview: Some(preview_body(&body)),
        });
    }

    // Data API usually returns a bare JSON array, but under upstream changes or
    // edge responses it can arrive wrapped. Parse leniently so the poller keeps running.
    let body = resp.text().await.map_err(|e| PollFailure {
        message: format!("failed to read /activity response body: {e}"),
        http_status: Some(status.as_u16()),
        content_type: content_type.clone(),
        body_preview: None,
    })?;

    let value: Value = serde_json::from_str(&body).map_err(|e| PollFailure {
        message: format!("failed to parse /activity response as JSON: {e}"),
        http_status: Some(status.as_u16()),
        content_type: content_type.clone(),
        body_preview: Some(preview_body(&body)),
    })?;

    let entries = if value.is_array() {
        serde_json::from_value::<Vec<ActivityEntry>>(value).map_err(|e| PollFailure {
            message: format!("failed to decode /activity array payload: {e}"),
            http_status: Some(status.as_u16()),
            content_type: content_type.clone(),
            body_preview: Some(preview_body(&body)),
        })?
    } else if let Some(arr) = value.get("data").and_then(|v| v.as_array()) {
        serde_json::from_value::<Vec<ActivityEntry>>(Value::Array(arr.clone())).map_err(|e| {
            PollFailure {
                message: format!("failed to decode /activity data[] payload: {e}"),
                http_status: Some(status.as_u16()),
                content_type: content_type.clone(),
                body_preview: Some(preview_body(&body)),
            }
        })?
    } else if let Some(arr) = value.get("activity").and_then(|v| v.as_array()) {
        serde_json::from_value::<Vec<ActivityEntry>>(Value::Array(arr.clone())).map_err(|e| {
            PollFailure {
                message: format!("failed to decode /activity activity[] payload: {e}"),
                http_status: Some(status.as_u16()),
                content_type: content_type.clone(),
                body_preview: Some(preview_body(&body)),
            }
        })?
    } else {
        return Err(PollFailure {
            message: "unexpected /activity JSON shape".to_string(),
            http_status: Some(status.as_u16()),
            content_type: content_type.clone(),
            body_preview: Some(preview_body(&body)),
        });
    };

    Ok(entries)
}
