//! Smart Money Signal Module
//!
//! Polls the Polymarket Data API for top-trader activity and emits
//! [`SmartMoneySignal`] events when significant trades are detected.
//!
//! Configuration (environment variables):
//! - `SMART_MONEY_ENABLED`       — set to `true` to activate (default: false)
//! - `SMART_MONEY_TOP_N`         — number of top wallets to track (default: 20)
//! - `SMART_MONEY_MIN_TRADE_USD` — minimum trade size to emit a signal (default: 500)
//! - `SMART_MONEY_POLL_MS`       — poll interval in milliseconds (default: 5000)
//!
//! Signals are forwarded on the same `tokio::sync::mpsc::Sender<RN1Signal>`
//! used by the RN1 poller, so the paper/live engine consumes them transparently.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::Sender;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, error, info, warn};

use crate::activity_log::{ActivityLog, EntryKind, push as log_push};
use crate::types::{OrderSide, RN1Signal};

// ─── Constants ───────────────────────────────────────────────────────────────

const DATA_API: &str = "https://data-api.polymarket.com";
const DEFAULT_TOP_N: usize = 20;
const DEFAULT_MIN_TRADE_USD: f64 = 500.0;
const DEFAULT_POLL_MS: u64 = 5_000;

// ─── Public Types ─────────────────────────────────────────────────────────────

/// A signal emitted when a tracked smart-money wallet places a significant trade.
#[derive(Debug, Clone)]
pub struct SmartMoneySignal {
    /// Wallet address of the smart-money trader.
    pub wallet: String,
    /// Token ID traded.
    pub token_id: String,
    /// Optional market title.
    pub market_title: Option<String>,
    /// Optional outcome label.
    pub market_outcome: Option<String>,
    /// Trade direction.
    pub side: OrderSide,
    /// Trade price (0.0–1.0).
    pub price: f64,
    /// USD notional value of the trade.
    pub size_usd: f64,
    /// Rank of the wallet in the leaderboard (1 = top).
    pub wallet_rank: usize,
}

/// Diagnostics exposed via the web server.
#[derive(Debug, Clone, Default)]
pub struct SmartMoneyDiagnostics {
    pub total_polls: u64,
    pub total_signals: u64,
    pub tracked_wallets: usize,
    pub consecutive_errors: u32,
    pub last_error: Option<String>,
}

pub type SmartMoneyDiagHandle = Arc<Mutex<SmartMoneyDiagnostics>>;

// ─── Internal API Types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LeaderboardEntry {
    #[serde(rename = "proxyWallet")]
    proxy_wallet: Option<String>,
    name: Option<String>,
    #[serde(rename = "pnl")]
    _pnl: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ActivityRecord {
    #[serde(rename = "id")]
    id: Option<String>,
    #[serde(rename = "asset")]
    token_id: Option<String>,
    side: Option<String>,
    price: Option<serde_json::Value>,
    size: Option<serde_json::Value>,
    #[serde(rename = "usdcSize")]
    usdc_size: Option<serde_json::Value>,
    title: Option<String>,
    outcome: Option<String>,
}

// ─── Runner ──────────────────────────────────────────────────────────────────

/// Spawns the smart-money polling loop.
///
/// Fetches the Polymarket leaderboard every `SMART_MONEY_POLL_MS` ms,
/// then polls the activity feed for each tracked wallet. Significant trades
/// are converted to [`RN1Signal`]s and forwarded on `signal_tx`.
pub async fn run_smart_money(
    signal_tx: Sender<RN1Signal>,
    activity: ActivityLog,
    diag: SmartMoneyDiagHandle,
    top_n: usize,
    min_trade_usd: f64,
    poll_ms: u64,
) {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("smart-money: failed to build HTTP client");

    // seen_ids: prevent re-emitting the same trade across polls
    let mut seen_ids: HashSet<String> = HashSet::new();
    // wallet list is refreshed every 60 polls to track leaderboard changes
    let mut tracked_wallets: Vec<(usize, String)> = Vec::new();
    let mut refresh_counter: u32 = 0;

    info!(top_n, min_trade_usd, poll_ms, "Smart money poller starting");
    log_push(&activity, EntryKind::Engine, format!(
        "SmartMoney: tracking top {top_n} wallets, min ${min_trade_usd:.0}/trade"
    ));

    loop {
        // Refresh leaderboard every 60 cycles (~5 minutes at default rate)
        if refresh_counter % 60 == 0 {
            match fetch_leaderboard(&client, top_n).await {
                Ok(wallets) => {
                    let count = wallets.len();
                    tracked_wallets = wallets;
                    let mut d = diag.lock().unwrap();
                    d.tracked_wallets = count;
                    debug!(count, "Smart money: refreshed leaderboard");
                }
                Err(e) => {
                    warn!(error = %e, "Smart money: failed to refresh leaderboard");
                    let mut d = diag.lock().unwrap();
                    d.consecutive_errors += 1;
                    d.last_error = Some(e.to_string());
                }
            }
        }
        refresh_counter = refresh_counter.wrapping_add(1);

        // Poll each wallet's recent activity
        for (rank, wallet) in &tracked_wallets {
            match fetch_wallet_activity(&client, wallet).await {
                Ok(trades) => {
                    for trade in trades {
                        let Some(ref id) = trade.id else { continue };
                        if seen_ids.contains(id) { continue; }

                        let size_usd = extract_f64(&trade.usdc_size)
                            .or_else(|| {
                                let p = extract_f64(&trade.price)?;
                                let s = extract_f64(&trade.size)?;
                                Some(p * s)
                            })
                            .unwrap_or(0.0);

                        if size_usd < min_trade_usd { continue; }

                        let Some(ref token_id) = trade.token_id else { continue };
                        let side = match trade.side.as_deref() {
                            Some("BUY") | Some("buy") => OrderSide::Buy,
                            Some("SELL") | Some("sell") => OrderSide::Sell,
                            _ => continue,
                        };
                        let price = extract_f64(&trade.price).unwrap_or(0.0);

                        seen_ids.insert(id.clone());
                        // Keep seen_ids bounded
                        if seen_ids.len() > 50_000 {
                            seen_ids.clear();
                        }

                        let signal = RN1Signal {
                            token_id: token_id.clone(),
                            market_title: trade.title.clone(),
                            market_outcome: trade.outcome.clone(),
                            side,
                            price: (price * 1000.0) as u64,
                            size: (size_usd * 1000.0) as u64,
                            order_id: id.clone(),
                            detected_at: std::time::Instant::now(),
                        };

                        let sm_sig = SmartMoneySignal {
                            wallet: wallet.clone(),
                            token_id: token_id.clone(),
                            market_title: trade.title,
                            market_outcome: trade.outcome,
                            side,
                            price,
                            size_usd,
                            wallet_rank: *rank,
                        };

                        info!(
                            wallet = %wallet,
                            rank   = rank,
                            token  = %token_id,
                            side   = ?side,
                            price  = price,
                            usd    = size_usd,
                            "SmartMoney signal"
                        );
                        log_push(&activity, EntryKind::Signal, format!(
                            "SmartMoney #{rank} {side:?} ${size_usd:.0} on {}",
                            sm_sig.market_title.as_deref().unwrap_or(token_id)
                        ));

                        {
                            let mut d = diag.lock().unwrap();
                            d.total_signals += 1;
                            d.consecutive_errors = 0;
                        }

                        if signal_tx.send(signal).is_err() {
                            error!("SmartMoney: signal channel closed — shutting down poller");
                            return;
                        }
                    }
                }
                Err(e) => {
                    debug!(wallet = %wallet, error = %e, "SmartMoney: failed to fetch wallet activity");
                }
            }
        }

        {
            let mut d = diag.lock().unwrap();
            d.total_polls += 1;
        }

        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
    }
}

// ─── Config Helper ────────────────────────────────────────────────────────────

/// Smart money configuration derived from environment variables.
#[derive(Debug, Clone)]
pub struct SmartMoneyConfig {
    pub enabled: bool,
    pub top_n: usize,
    pub min_trade_usd: f64,
    pub poll_ms: u64,
}

impl SmartMoneyConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("SMART_MONEY_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let top_n = std::env::var("SMART_MONEY_TOP_N")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_TOP_N);
        let min_trade_usd = std::env::var("SMART_MONEY_MIN_TRADE_USD")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MIN_TRADE_USD);
        let poll_ms = std::env::var("SMART_MONEY_POLL_MS")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_POLL_MS);
        Self { enabled, top_n, min_trade_usd, poll_ms }
    }
}

// ─── API Helpers ─────────────────────────────────────────────────────────────

async fn fetch_leaderboard(client: &Client, limit: usize) -> anyhow::Result<Vec<(usize, String)>> {
    let url = format!("{DATA_API}/leaderboard?window=weekly&limit={limit}");
    let resp: serde_json::Value = client.get(&url)
        .send().await?
        .json().await?;

    let entries = resp.as_array()
        .or_else(|| resp["data"].as_array())
        .cloned()
        .unwrap_or_default();

    let wallets: Vec<(usize, String)> = entries.iter().enumerate()
        .filter_map(|(i, e)| {
            let wallet = e["proxyWallet"].as_str()
                .or_else(|| e["address"].as_str())
                .or_else(|| e["user"].as_str())?
                .to_lowercase();
            Some((i + 1, wallet))
        })
        .collect();

    Ok(wallets)
}

async fn fetch_wallet_activity(client: &Client, wallet: &str) -> anyhow::Result<Vec<ActivityRecord>> {
    let url = format!("{DATA_API}/activity?user={wallet}&limit=10");
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;

    let arr = resp.as_array()
        .or_else(|| resp["data"].as_array())
        .cloned()
        .unwrap_or_default();

    let records: Vec<ActivityRecord> = arr.iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();

    Ok(records)
}

fn extract_f64(v: &Option<serde_json::Value>) -> Option<f64> {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}
