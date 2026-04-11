//! Paper trading engine.
//!
//! Receives [`RN1Signal`]s, sizes the mirror order, simulates a 3-second fill
//! window (drift failsafe), and records virtual fills in [`PaperPortfolio`].
//! When an [`ActivityLog`] is provided (TUI mode), events are pushed there
//! instead of (or in addition to) stdout.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use chrono::Datelike;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::activity_log::{ActivityLog, EntryKind, push as log_push};
use crate::clickhouse_logger::{ClosedTradeFull, EquitySnapshot, WarehouseEvent};
use crate::exit_strategy::{ExitAction, ExitConfig, evaluate_exits};
use crate::latency_tracker::LatencyStats;
use crate::order_book::{OrderBook, OrderBookStore};
use crate::paper_portfolio::{DRIFT_THRESHOLD, PaperPortfolio, STARTING_BALANCE_USDC, polymarket_taker_fee};
use crate::risk_manager::{RiskConfig, RiskManager};
use crate::types::{OrderSide, RN1Signal, format_price};

// ─── PaperEngine ─────────────────────────────────────────────────────────────

/// 4C: Build a category-specific ExitConfig patch.
///
/// Checks env vars like `EXIT_SPORTS_STOP_LOSS_PCT`, `EXIT_CRYPTO_MAX_HOLD_SECS`,
/// etc.  Falls back to the base config for any field without an override.
/// Valid category prefixes: SPORTS, POLITICS, CRYPTO, GEO, OTHER.
fn patched_exit_config_for_category(base: &ExitConfig, market_title: Option<&str>) -> ExitConfig {
    let (category, _) = crate::paper_portfolio::detect_fee_category(market_title.unwrap_or(""));
    let prefix = match category {
        "sports"      => "SPORTS",
        "politics"    => "POLITICS",
        "crypto"      => "CRYPTO",
        "geopolitics" => "GEO",
        _             => "OTHER",
    };
    let mut cfg = base.clone();
    if let Ok(v) = std::env::var(format!("EXIT_{prefix}_STOP_LOSS_PCT")) {
        if let Ok(pct) = v.parse::<f64>() { cfg.stop_loss_pct = pct.clamp(1.0, 99.0); }
    }
    if let Ok(v) = std::env::var(format!("EXIT_{prefix}_TRAILING_ACTIVATE_PCT")) {
        if let Ok(pct) = v.parse::<f64>() { cfg.trailing_stop_activate_pct = pct.clamp(1.0, 99.0); }
    }
    if let Ok(v) = std::env::var(format!("EXIT_{prefix}_TRAILING_DROP_PCT")) {
        if let Ok(pct) = v.parse::<f64>() { cfg.trailing_stop_drop_pct = pct.clamp(1.0, 99.0); }
    }
    if let Ok(v) = std::env::var(format!("EXIT_{prefix}_MAX_HOLD_SECS")) {
        if let Ok(s) = v.parse::<u64>() { cfg.max_hold_secs = s; }
    }
    if let Ok(v) = std::env::var(format!("EXIT_{prefix}_EVENT_AWARE_SECS")) {
        if let Ok(s) = v.parse::<u64>() { cfg.event_aware_exit_secs = s; }
    }
    cfg
}

/// Paper trading engine — simulates order placement without touching real funds.
pub struct PaperEngine {
    pub portfolio: Arc<Mutex<PaperPortfolio>>,
    book_store:    Arc<OrderBookStore>,
    /// Optional activity log for TUI display. `None` → log to stdout only.
    activity:      Option<ActivityLog>,
    /// Risk manager — shared with TUI for runtime config editing.
    pub risk:      Arc<std::sync::Mutex<RiskManager>>,
    /// Active fill-window snapshot for the TUI failsafe visualizer.
    pub fill_window:  Arc<std::sync::Mutex<Option<FillWindowSnapshot>>>,
    /// Detection-to-fill latency samples for the TUI histogram.
    pub fill_latency: Arc<std::sync::Mutex<LatencyStats>>,
    signal_queue: Arc<Mutex<BinaryHeap<PrioritySignal>>>,
    volatility_state: Arc<std::sync::Mutex<VolatilityState>>,
    rejection_analytics: Arc<Mutex<RejectionAnalytics>>,
    shadow_comparator: Arc<Mutex<ShadowComparator>>,
    experiments: Arc<std::sync::Mutex<ExperimentSwitches>>,
    twin_health: Arc<Mutex<TwinHealth>>,
    metadata_client: Client,
    rn1_wallet: String,
    signal_meta_cache: Arc<Mutex<HashMap<String, CachedSignalMeta>>>,
    seen_order_ids: Arc<Mutex<HashSet<String>>>,
    equity_tick: std::sync::atomic::AtomicU64,
    /// Shared subscription list — new token_ids are added here on fill so the WS client
    /// subscribes and `get_market_price()` stays live for the position's lifetime.
    market_subscriptions: Arc<std::sync::Mutex<Vec<String>>>,
    ws_force_reconnect: Arc<std::sync::atomic::AtomicBool>,
    /// Per-token drift-abort cooldown. After a drift abort, the token is blocked
    /// for DRIFT_ABORT_COOLDOWN_SECS seconds to prevent cascading redundant aborts.
    drift_abort_cooldown: Arc<std::sync::Mutex<HashMap<String, Instant>>>,
    /// Optional Bullpen discovery store for conviction boost lookups.
    pub discovery_store: Option<Arc<tokio::sync::RwLock<crate::bullpen_discovery::DiscoveryStore>>>,
    /// Optional Bullpen convergence store for whale convergence sizing boost.
    pub convergence_store: Option<Arc<tokio::sync::RwLock<crate::bullpen_smart_money::ConvergenceStore>>>,
    /// Per-"token_id:side" fill timestamps for soft deduplication window (2C).
    recent_fills: Arc<std::sync::Mutex<HashMap<String, Instant>>>,
    /// Session-start NAV for intraday drawdown gating (3B): (nav, ordinal_day).
    session_start_nav: Arc<std::sync::Mutex<Option<(f64, u32)>>>,
    /// Optional warehouse sender — emits equity snapshots and closed trades to ClickHouse.
    warehouse_tx: Option<crossbeam_channel::Sender<WarehouseEvent>>,
}

/// Snapshot of the currently active fill window, if any.
#[derive(Debug, Clone)]
pub struct FillWindowSnapshot {
    pub token_id:      String,
    pub side:          OrderSide,
    pub entry_price:   f64,
    pub current_price: Option<f64>,
    pub drift_pct:     Option<f64>,
    pub elapsed:       Duration,
    pub countdown:     Duration,
}

#[derive(Debug, Clone)]
struct PrioritySignal {
    edge_score: f64,
    queued_at: Instant,
    signal: RN1Signal,
}

impl Eq for PrioritySignal {}
impl PartialEq for PrioritySignal {
    fn eq(&self, other: &Self) -> bool {
        self.edge_score == other.edge_score
    }
}
impl Ord for PrioritySignal {
    fn cmp(&self, other: &Self) -> Ordering {
        self.edge_score
            .partial_cmp(&other.edge_score)
            .unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for PrioritySignal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
struct VolatilityState {
    samples: VecDeque<f64>,
    max_samples: usize,
}

#[derive(Debug, Clone)]
struct CachedSignalMeta {
    market_title: Option<String>,
    market_outcome: Option<String>,
    event_start_time: Option<i64>,
    event_end_time: Option<i64>,
    cached_at: Instant,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TradeMetaEntry {
    #[serde(default, alias = "transactionHash")]
    transaction_hash: Option<String>,
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
}

impl VolatilityState {
    fn new(max_samples: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(max_samples),
            max_samples: max_samples.max(16),
        }
    }

    fn push(&mut self, p: f64) {
        if self.samples.len() >= self.max_samples {
            self.samples.pop_front();
        }
        self.samples.push_back(p);
    }

    fn volatility_bps(&self) -> f64 {
        if self.samples.len() < 3 {
            return 0.0;
        }
        let mean = self.samples.iter().sum::<f64>() / self.samples.len() as f64;
        if mean <= 0.0 {
            return 0.0;
        }
        let var = self
            .samples
            .iter()
            .map(|v| {
                let d = *v - mean;
                d * d
            })
            .sum::<f64>()
            / self.samples.len() as f64;
        (var.sqrt() / mean) * 10_000.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RejectionAnalytics {
    pub schema_version: u32,
    pub reasons: HashMap<String, Vec<i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RejectionTrendPoint {
    pub hour_utc_epoch: i64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionSummary {
    pub trades: usize,
    pub fill_rate_pct: f64,
    pub reject_rate_pct: f64,
    pub avg_slippage_bps: f64,
    pub avg_queue_delay_ms: f64,
    pub shadow_realism_gap_bps: f64,
    pub tags: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowFillObservation {
    pub token_id: String,
    pub order_id: String,
    pub side: OrderSide,
    pub expected_price: f64,
    pub paper_fill_price: f64,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShadowComparator {
    pub observations: Vec<ShadowFillObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExperimentSwitches {
    pub schema_version: u32,
    pub sizing_variant_b: bool,
    pub autoclaim_variant_b: bool,
    pub drift_variant_b: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExperimentMetrics {
    pub variant_a_fills: usize,
    pub variant_b_fills: usize,
    pub variant_a_realized_pnl: f64,
    pub variant_b_realized_pnl: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TwinHealth {
    pub abort_rate: f64,
    pub close_rate: f64,
    pub open_positions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmState {
    pub schema_version: u32,
    pub saved_at_ms: i64,
    pub market_subscriptions: Vec<String>,
    pub order_books: Vec<(String, OrderBook)>,
    pub portfolio_path: String,
    pub rejection_analytics: RejectionAnalytics,
    pub comparator: ShadowComparator,
    pub experiments: ExperimentSwitches,
    pub checksum: u64,
}

impl FillWindowSnapshot {
    fn new(token_id: String, side: OrderSide, entry_price: f64, countdown: Duration) -> Self {
        Self {
            token_id,
            side,
            entry_price,
            current_price: None,
            drift_pct: None,
            elapsed: Duration::from_secs(0),
            countdown,
        }
    }
}

impl PaperEngine {
    /// Creates a new engine with a fresh `$100 USDC` virtual portfolio.
    ///
    /// Pass `Some(log)` to feed a TUI activity panel; pass `None` for plain
    /// terminal output.
    pub fn new(book_store: Arc<OrderBookStore>, activity: Option<ActivityLog>, market_subscriptions: Arc<std::sync::Mutex<Vec<String>>>, ws_force_reconnect: Arc<std::sync::atomic::AtomicBool>, warehouse_tx: Option<crossbeam_channel::Sender<WarehouseEvent>>) -> Self {
        if activity.is_none() {
            // Only print the text banner when not in TUI mode.
            println!();
            println!("╔════════════════════════════════════════════════════════════╗");
            println!("║         📄  BLINK PAPER TRADING MODE ACTIVE               ║");
            println!("║  Starting balance:  ${:<10.2}  (virtual USDC)           ║", STARTING_BALANCE_USDC);
            println!("║  Sizing:            2% of RN1 notional, max 10% of NAV   ║");
            println!("║  Fill window:       3 s — aborts if price drifts >1.5%   ║");
            println!("║  NO REAL ORDERS WILL BE PLACED                            ║");
            println!("╚════════════════════════════════════════════════════════════╝");
            println!();
        }
        if let Some(ref log) = activity {
            log_push(log, EntryKind::Engine,
                format!("Paper trading started — balance ${:.2} USDC", STARTING_BALANCE_USDC));
        }
        Self {
            portfolio: Arc::new(Mutex::new(PaperPortfolio::new())),
            book_store,
            activity,
            risk: Arc::new(std::sync::Mutex::new(RiskManager::new(RiskConfig::from_env()))),
            fill_window: Arc::new(std::sync::Mutex::new(None)),
            fill_latency: Arc::new(std::sync::Mutex::new(LatencyStats::new(1_000))),
            signal_queue: Arc::new(Mutex::new(BinaryHeap::new())),
            volatility_state: Arc::new(std::sync::Mutex::new(VolatilityState::new(120))),
            rejection_analytics: Arc::new(Mutex::new(RejectionAnalytics { schema_version: 1, reasons: HashMap::new() })),
            shadow_comparator: Arc::new(Mutex::new(ShadowComparator::default())),
            experiments: Arc::new(std::sync::Mutex::new(ExperimentSwitches {
                schema_version: 1,
                sizing_variant_b: env_flag("EXPERIMENT_SIZING_B"),
                autoclaim_variant_b: env_flag("EXPERIMENT_AUTOCLAIM_B"),
                drift_variant_b: env_flag("EXPERIMENT_DRIFT_B"),
            })),
            twin_health: Arc::new(Mutex::new(TwinHealth::default())),
            metadata_client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest client"),
            rn1_wallet: std::env::var("RN1_WALLET").unwrap_or_default(),
            signal_meta_cache: Arc::new(Mutex::new(HashMap::new())),
            seen_order_ids: Arc::new(Mutex::new(HashSet::with_capacity(512))),
            equity_tick: std::sync::atomic::AtomicU64::new(0),
            market_subscriptions,
            ws_force_reconnect,
            drift_abort_cooldown: Arc::new(std::sync::Mutex::new(HashMap::new())),
            discovery_store: None,
            convergence_store: None,
            recent_fills: Arc::new(std::sync::Mutex::new(HashMap::new())),
            session_start_nav: Arc::new(std::sync::Mutex::new(None)),
            warehouse_tx,
        }
    }

    pub fn twin_health_handle(&self) -> Arc<Mutex<TwinHealth>> {
        Arc::clone(&self.twin_health)
    }

    pub fn rejection_analytics_handle(&self) -> Arc<Mutex<RejectionAnalytics>> {
        Arc::clone(&self.rejection_analytics)
    }

    pub fn risk_status(&self) -> String {
        self.risk.lock().unwrap().status_line()
    }

    /// Returns the current global volatility in basis points (coefficient of variation × 10 000).
    pub fn vol_bps(&self) -> f64 {
        self.volatility_state.lock().unwrap().volatility_bps()
    }

    pub async fn load_portfolio_if_present(&self, path: &str) -> std::io::Result<bool> {
        if !std::path::Path::new(path).exists() {
            return Ok(false);
        }
        let loaded = PaperPortfolio::load_from_path(path)?;
        let mut p = self.portfolio.lock().await;
        *p = loaded;
        Ok(true)
    }

    pub async fn load_rejections_if_present(&self, path: &str) -> std::io::Result<bool> {
        if !std::path::Path::new(path).exists() {
            return Ok(false);
        }
        let data = std::fs::read_to_string(path)?;
        let mut parsed: RejectionAnalytics = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        if parsed.schema_version == 0 {
            parsed.schema_version = 1;
        }
        *self.rejection_analytics.lock().await = parsed;
        Ok(true)
    }

    pub async fn save_rejections(&self, path: &str) -> std::io::Result<()> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let state = self.rejection_analytics.lock().await.clone();
        atomic_write_with_backup(path, &state)
    }

    pub async fn save_portfolio(&self, path: &str) -> std::io::Result<()> {
        let p = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.portfolio.lock(),
        ).await {
            Ok(guard) => guard,
            Err(_) => {
                tracing::warn!("save_portfolio: lock timeout (2s) — skipping save");
                return Ok(());
            }
        };
        let mut tmp = p.save_to_path(path);
        if tmp.is_ok() {
            let data = std::fs::read_to_string(path)?;
            atomic_write_with_backup(path, &serde_json::from_str::<serde_json::Value>(&data)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?)?;
            tmp = Ok(());
        }
        tmp
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// Process one RN1 signal end-to-end (async — runs fill simulation).
    pub async fn handle_signal(&self, signal: RN1Signal) {
        // 5B: Live tick recording — write every RN1 signal to CSV for backtesting.
        {
            let tick_path = std::env::var("TICK_RECORD_PATH")
                .unwrap_or_else(|_| "logs\\ticks.csv".to_string());
            if tick_path != "off" && tick_path != "false" {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .append(true).create(true).open(&tick_path)
                {
                    let ts_ms = chrono::Utc::now().timestamp_millis();
                    let _ = writeln!(f, "{},{},{},{},{},{}",
                        ts_ms, signal.token_id, signal.side,
                        signal.price, signal.size, signal.source_wallet);
                }
            }
        }

        // ── Order-ID dedup: skip if we've already processed this transaction ──
        {
            let mut seen = self.seen_order_ids.lock().await;
            if !seen.insert(signal.order_id.clone()) {
                warn!(order_id = %signal.order_id, "⏭️  Duplicate order_id — skipping");
                return;
            }
        }

        // ── Per-token dedup: skip if we already hold a position on this token ──
        {
            let mut p = self.portfolio.lock().await;
            if p.positions.iter().any(|pos| pos.token_id == signal.token_id) {
                warn!(token_id = %signal.token_id, "⏭️  Already holding position — skipping");
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("already_holding").await;
                return;
            }
            // ── Per-match concentration limit: max 2 positions on same event ──
            let max_per_match: usize = std::env::var("MAX_POSITIONS_PER_MATCH")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(2);
            if let Some(title) = signal.market_title.as_deref() {
                let match_key = title.split(':').next().unwrap_or(title).trim().to_lowercase();
                if match_key.len() > 5 {
                    let same_match = p.positions.iter().filter(|pos| {
                        pos.market_title.as_deref()
                            .map(|t| t.split(':').next().unwrap_or(t).trim().to_lowercase() == match_key)
                            .unwrap_or(false)
                    }).count();
                    if same_match >= max_per_match {
                        warn!(
                            match_name = %match_key,
                            count = same_match,
                            max = max_per_match,
                            "⏭️  Match concentration limit — skipping"
                        );
                        p.skipped_orders += 1;
                        drop(p);
                        self.record_rejection("match_concentration").await;
                        return;
                    }
                }
            }
        }

        // Compute vol_bps once — reused for gating (1B) and adaptive sizing (2A).
        let vol_bps = self.volatility_state.lock().unwrap().volatility_bps();

        // 2C: Soft dedup window — skip if we filled this (token, side) recently.
        {
            let dedup_window_ms = std::env::var("DEDUP_WINDOW_MS")
                .ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(30_000);
            let dedup_key = format!("{}:{}", signal.token_id, signal.side);
            let blocked = {
                let mut rf = self.recent_fills.lock().unwrap();
                rf.retain(|_, t: &mut Instant| t.elapsed().as_millis() < dedup_window_ms as u128);
                rf.contains_key(&dedup_key)
            };
            if blocked {
                warn!(token_id = %signal.token_id, "⏭️  Soft dedup window — recently filled this token/side");
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("dedup_window").await;
                return;
            }
        }

        // 1B: Volatility gate — skip when the signal environment is too noisy.
        {
            let vol_gate_threshold: f64 = std::env::var("VOL_GATE_THRESHOLD_BPS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(800.0);
            if vol_bps > vol_gate_threshold {
                warn!(
                    vol_bps, threshold = vol_gate_threshold,
                    "⏭️  Signal gated — volatility too high"
                );
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("vol_gate").await;
                return;
            }
        }

        let now = Instant::now();

        {
            let mut q = self.signal_queue.lock().await;
            let edge = self.compute_edge_score(&signal);
            q.push(PrioritySignal {
                edge_score: edge,
                queued_at: now,
                signal,
            });
        }
        let Some(prio_signal) = self.signal_queue.lock().await.pop() else {
            return;
        };
        let mut signal = prio_signal.signal;
        self.enrich_signal_metadata(&mut signal).await;
        let queue_delay_ms = prio_signal.queued_at.elapsed().as_millis() as u64;

        // ── 6-hour event horizon rule ─────────────────────────────────────
        // Skip signals where the event starts more than 6 hours from now.
        // We only skip when we actually know the start time — unknown timing passes through.
        const MAX_EVENT_HORIZON_SECS: i64 = 6 * 3600;
        if let Some(start_ts) = signal.event_start_time {
            let now_secs = chrono::Utc::now().timestamp();
            if start_ts > now_secs + MAX_EVENT_HORIZON_SECS {
                let hours_away = (start_ts - now_secs + 1799) / 3600; // round up
                warn!(
                    token_id   = %signal.token_id,
                    hours_away = hours_away,
                    "⏭️  Event too far away — skipping (>{} h horizon)",
                    MAX_EVENT_HORIZON_SECS / 3600,
                );
                if let Some(ref log) = self.activity {
                    log_push(log, EntryKind::Skip, format!(
                        "Skipped — event {}h away (max 6h horizon)", hours_away
                    ));
                }
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("event_too_far").await;
                return;
            }
        }

        // Convert scaled integers back to f64 for human-readable logic.
        let mut entry_price      = signal.price as f64 / 1_000.0;
        let rn1_shares       = signal.size  as f64 / 1_000.0;
        let rn1_notional_usd = rn1_shares * entry_price;
        let realism_mode = std::env::var("PAPER_REALISM_MODE")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let adverse_fill_bps = std::env::var("PAPER_ADVERSE_FILL_BPS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(10.0)
            .clamp(0.0, 500.0);
        if realism_mode {
            let h = adverse_fill_bps / 10_000.0;
            entry_price = match signal.side {
                OrderSide::Buy => entry_price * (1.0 + h),
                OrderSide::Sell => entry_price * (1.0 - h),
            }
            .clamp(0.0, 1.0);
        }

        // ── Min-notional filter (skip micro-signals before sizing) ──────────
        let min_notional: f64 = std::env::var("MIN_SIGNAL_NOTIONAL_USD")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(10.0);
        if rn1_notional_usd < min_notional {
            let mut p = self.portfolio.lock().await;
            p.skipped_orders += 1;
            drop(p);
            warn!(
                rn1_notional_usd = %format!("${:.2}", rn1_notional_usd),
                min = %format!("${:.2}", min_notional),
                "⏭️  Signal skipped — RN1 notional below minimum"
            );
            self.record_rejection("min_notional").await;
            return;
        }

        // ── Extreme price filter: skip very low or very high odds ──────────
        if entry_price < 0.10 || entry_price > 0.95 {
            let mut p = self.portfolio.lock().await;
            p.skipped_orders += 1;
            drop(p);
            warn!(
                price = %format!("{:.3}", entry_price),
                "⏭️  Signal skipped — extreme price (no edge)"
            );
            self.record_rejection("extreme_price").await;
            return;
        }

        // ── Market category filter: block esports + fee-aware gating ────
        let title_str = signal.market_title.as_deref().unwrap_or("");
        {
            let title_lower = title_str.to_lowercase();
            let blocked_keywords = ["esports", "lol:", "cs2:", "cs:go", "dota", "valorant",
                "league of legends", "counter-strike", "overwatch", "bo3)", "bo5)",
                "lec ", "lck ", "lpl ", "vct "];
            if blocked_keywords.iter().any(|kw| title_lower.contains(kw)) {
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                warn!(
                    title = %title_str,
                    "⏭️  Signal skipped — blocked market category (esports)"
                );
                self.record_rejection("blocked_category").await;
                return;
            }
        }

        // ── Fee category detection & cash-aware preference ────────────────
        let (fee_category, fee_rate) = crate::paper_portfolio::detect_fee_category(title_str);
        {
            let p = self.portfolio.lock().await;
            let cash_pct = p.cash_usdc / p.nav();
            drop(p);
            // When cash is tight (<50% NAV), only take low-fee markets
            if cash_pct < 0.50 && fee_rate > 0.04 {
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                warn!(
                    category = %fee_category,
                    fee_rate = %format!("{:.1}%", fee_rate * 100.0),
                    cash_pct = %format!("{:.0}%", cash_pct * 100.0),
                    "⏭️  Signal skipped — high-fee category while cash is low"
                );
                self.record_rejection("high_fee_low_cash").await;
                return;
            }
        }

        // ── Fee-to-edge filter: skip if fee would eat >60% of expected gain ──
        {
            let est_shares = rn1_notional_usd * 0.05 / entry_price; // estimated shares at 5% multiplier
            let est_fee = est_shares * fee_rate * entry_price * (1.0 - entry_price);
            let est_edge = rn1_notional_usd * 0.05 * 0.05; // assume ~5% price movement
            if est_fee > est_edge * 0.60 && fee_rate > 0.0 {
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                warn!(
                    fee = %format!("${:.3}", est_fee),
                    edge = %format!("${:.3}", est_edge),
                    category = %fee_category,
                    "⏭️  Signal skipped — fee exceeds 60% of estimated edge"
                );
                self.record_rejection("fee_exceeds_edge").await;
                return;
            }
        }

        // 3A: Per-market exposure limit — cap total invested in one market.
        // 3B: Intraday drawdown gating — throttle or pause on session loss.
        let drawdown_halving: bool;
        {
            let max_market_pct = std::env::var("MAX_EXPOSURE_PER_MARKET_PCT")
                .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(15.0) / 100.0;
            let max_intraday_dd: f64 = std::env::var("MAX_INTRADAY_DRAWDOWN_PCT")
                .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(5.0);
            let pause_dd = max_intraday_dd + 3.0;

            let mut p = self.portfolio.lock().await;
            let nav = p.nav();

            // 3A: per-market check
            if let Some(title) = signal.market_title.as_deref() {
                let market_invested: f64 = p.positions.iter()
                    .filter(|pos| pos.market_title.as_deref() == Some(title))
                    .map(|pos| pos.usdc_spent)
                    .sum();
                let market_limit = nav * max_market_pct;
                if market_invested >= market_limit {
                    warn!(
                        market = %title,
                        invested = %format!("${:.2}", market_invested),
                        limit   = %format!("${:.2}", market_limit),
                        "⏭️  Per-market exposure limit — skipping"
                    );
                    p.skipped_orders += 1;
                    drop(p);
                    self.record_rejection("market_concentration").await;
                    return;
                }
            }

            // 3B: intraday drawdown
            let day_of_year = chrono::Utc::now().ordinal();
            let session_nav = {
                let mut ssn = self.session_start_nav.lock().unwrap();
                match *ssn {
                    None => { *ssn = Some((nav, day_of_year)); nav }
                    Some((_, d)) if d != day_of_year => { *ssn = Some((nav, day_of_year)); nav }
                    Some((s, _)) => s,
                }
            };
            let drawdown_pct = if session_nav > 0.0 {
                (session_nav - nav) / session_nav * 100.0
            } else {
                0.0
            };
            if drawdown_pct >= pause_dd {
                warn!(
                    drawdown_pct = %format!("{:.1}%", drawdown_pct),
                    "🛑 Intraday drawdown pause — trading halted"
                );
                if let Some(ref log) = self.activity {
                    log_push(log, EntryKind::Warn, format!(
                        "INTRADAY PAUSE — drawdown {:.1}%", drawdown_pct
                    ));
                }
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("intraday_drawdown_pause").await;
                return;
            }
            drawdown_halving = drawdown_pct >= max_intraday_dd;
            if drawdown_halving {
                info!(
                    drawdown_pct = %format!("{:.1}%", drawdown_pct),
                    "⚠️  Intraday drawdown warning — will halve position size"
                );
            }
        } // portfolio lock released

        // ── Conviction-based sizing ──────────────────────────────────────
        // Compute a dynamic multiplier using FilterConfig bonuses based on
        // RN1 bet size, market category, sport, and liquidity.
        // Discovery boost + convergence boost added from Bullpen data when available.
        let conviction_mult = {
            let filter_cfg = crate::types::FilterConfig::from_env();
            let base = crate::exit_strategy::conviction_multiplier(
                rn1_notional_usd,
                fee_category,
                None, // sport tag — enriched below if available
                0.0,  // liquidity — not yet fetched at this point
                &filter_cfg,
            );
            let discovery_boost = if let Some(ref store) = self.discovery_store {
                let s = store.read().await;
                s.conviction_boost(&signal.token_id)
            } else {
                0.0
            };
            let convergence_boost = if let Some(ref store) = self.convergence_store {
                let s = store.read().await;
                s.convergence_boost(&signal.token_id)
            } else {
                0.0
            };
            Some(base + discovery_boost + convergence_boost)
        };

        // ── Sizing (brief lock, no await) ─────────────────────────────
        let (size_usdc, current_nav) = {
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            // Keep paper marks alive even when WS mark prices are stale/missing.
            // RN1 signal price becomes a fallback mark update for this token.
            p.update_price(&signal.token_id, entry_price);
            let size = p.calculate_size_usdc_with_conviction(rn1_notional_usd, conviction_mult);
            let nav  = p.nav();
            (size, nav)
        };

        info!(
            token_id         = %signal.token_id,
            side             = %signal.side,
            rn1_price        = %format_price(signal.price),
            rn1_notional_usd = %format!("${:.2}", rn1_notional_usd),
            our_size         = ?size_usdc.map(|s| format!("${:.2}", s)),
            nav              = %format!("${:.2}", current_nav),
            "📡 RN1 signal received"
        );
        if let Some(ref log) = self.activity {
            log_push(log, EntryKind::Signal, format!(
                "RN1 {} @{:.3}  notional=${:.2}  our size={}",
                signal.side,
                entry_price,
                rn1_notional_usd,
                size_usdc.map(|s| format!("${:.2}", s)).unwrap_or_else(|| "–".into()),
            ));
        }

        // ── Mirror RN1 exit: SELL signal → close our matching BUY position ──
        // RN1 exiting a position is the primary signal to exit. We close the
        // matching open BUY at current price rather than trying to open a short.
        if signal.side == OrderSide::Sell {
            let mut p = self.portfolio.lock().await;
            if let Some(idx) = p.positions.iter().position(|pos| {
                pos.token_id == signal.token_id && pos.side == OrderSide::Buy
            }) {
                let pos = p.positions.remove(idx);
                let exit_price = pos.current_price.clamp(0.001, 0.999);
                let exit_fee = polymarket_taker_fee(pos.shares, exit_price);
                let pnl = (exit_price - pos.entry_price) * pos.shares - exit_fee;
                p.total_fees_paid_usdc += exit_fee;
                p.cash_usdc += pos.usdc_spent + pnl;
                let dur = pos.opened_at.elapsed().as_secs();
                p.closed_trades.push(crate::paper_portfolio::ClosedTrade {
                    token_id: pos.token_id.clone(),
                    market_title: pos.market_title.clone(),
                    side: pos.side,
                    entry_price: pos.entry_price,
                    exit_price,
                    shares: pos.shares,
                    realized_pnl: pnl,
                    fees_paid_usdc: pos.entry_fee_paid_usdc + exit_fee,
                    reason: "rn1_mirror_exit".to_string(),
                    opened_at_wall: pos.opened_at_wall,
                    closed_at_wall: chrono::Local::now(),
                    duration_secs: dur,
                    scorecard: crate::paper_portfolio::ExecutionScorecard {
                        slippage_bps: pos.entry_slippage_bps,
                        queue_delay_ms: pos.queue_delay_ms,
                        outcome_tags: vec!["rn1_mirror_exit".to_string()],
                    },
                    event_start_time: pos.event_start_time,
                    event_end_time: pos.event_end_time,
                });
                self.risk.lock().unwrap().record_close(pnl);
                if let Some(ref log) = self.activity {
                    log_push(log, EntryKind::Fill, format!(
                        "RN1 EXIT: closed BUY @{:.3} (entry={:.3})  pnl={:+.3}  dur={}s",
                        exit_price, pos.entry_price, pnl, dur,
                    ));
                }
                info!(token_id = %pos.token_id, exit_price, pnl, "🔴 RN1 mirror-exit: closed BUY position");
            } else {
                // RN1 selling a token we don't hold — skip rather than open a short.
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("sell_no_position").await;
            }
            return;
        }

        let size_usdc = match size_usdc {
            Some(s) => s,
            None => {
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                warn!(
                    rn1_notional_usd = %format!("${:.2}", rn1_notional_usd),
                    cash             = %format!("${:.2}", p.cash_usdc),
                    "⏭️  Signal skipped — size below minimum or no cash"
                );
                if let Some(ref log) = self.activity {
                    log_push(log, EntryKind::Skip, format!(
                        "Skipped — notional=${:.2}  cash=${:.2}", rn1_notional_usd, p.cash_usdc
                    ));
                }
                self.record_rejection("size_or_cash").await;
                return;
            }
        };

        // Dynamic token concentration cap + sizing decay.
        let mut size_usdc = size_usdc;
        {
            let p = self.portfolio.lock().await;
            let token_invested: f64 = p.positions
                .iter()
                .filter(|pos| pos.token_id == signal.token_id)
                .map(|pos| pos.usdc_spent)
                .sum();
            let token_cap_pct = std::env::var("PAPER_TOKEN_MAX_EXPOSURE_PCT")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.20)
                .clamp(0.05, 1.0);
            let token_cap_usdc = current_nav * token_cap_pct;
            if token_invested >= token_cap_usdc {
                if let Some(ref log) = self.activity {
                    log_push(log, EntryKind::Skip, format!(
                        "Skipped — token concentration cap hit token={} invested={:.2} cap={:.2}",
                        &signal.token_id, token_invested, token_cap_usdc
                    ));
                }
                self.record_rejection("token_concentration_cap").await;
                let mut pm = self.portfolio.lock().await;
                pm.skipped_orders += 1;
                return;
            }
            let remaining_cap = (token_cap_usdc - token_invested).max(0.0);
            if size_usdc > remaining_cap {
                size_usdc = remaining_cap;
            }
            let concentration_ratio = if token_cap_usdc > 0.0 { token_invested / token_cap_usdc } else { 0.0 };
            let sizing_decay = if concentration_ratio > 0.80 {
                0.5
            } else if concentration_ratio > 0.60 {
                0.7
            } else {
                1.0
            };
            size_usdc *= sizing_decay;
        }

        // Twin-based throttle (shared health signal).
        {
            let th = self.twin_health.lock().await;
            let throttle = if th.abort_rate > 0.70 && th.close_rate < 0.05 {
                0.4
            } else if th.abort_rate > 0.50 {
                0.7
            } else {
                1.0
            };
            drop(th);
            size_usdc *= throttle;
        }

        // 2A: Adaptive sizing by volatility regime — shrink orders when signal environment is noisy.
        {
            let adaptive = std::env::var("ADAPTIVE_SIZING")
                .ok().map(|v| v != "false" && v != "0").unwrap_or(true);
            if adaptive && vol_bps > 0.0 {
                let vol_discount = 1.0 - (vol_bps.clamp(0.0, 2000.0) / 4000.0);
                size_usdc *= vol_discount;
                if vol_discount < 0.99 {
                    info!(
                        vol_bps, vol_discount = %format!("{:.2}", vol_discount),
                        "📉 Adaptive sizing: vol discount applied"
                    );
                }
            }
        }

        // 3B: Intraday drawdown warning — halve size when session loss exceeds warning threshold.
        if drawdown_halving {
            size_usdc *= 0.5;
        }

        // Price-confidence sizing: reduce position size for mid-range prices where
        // outcome uncertainty is highest. Prices near 0.1 or 0.9 get full size;
        // prices near 0.5 are capped at CONF_DISCOUNT of full size.
        // Configurable via PAPER_CONFIDENCE_DISCOUNT (0.0 = no reduction, 0.5 = max 50% reduction at 0.5).
        {
            let discount = std::env::var("PAPER_CONFIDENCE_DISCOUNT")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.35)
                .clamp(0.0, 0.75);
            // uncertainty peaks at 0.5 (= 1.0) and is 0.0 at 0.0 or 1.0
            let uncertainty = 1.0 - (2.0 * (entry_price - 0.5).abs()).clamp(0.0, 1.0);
            let conf_mult = 1.0 - discount * uncertainty;
            size_usdc *= conf_mult;
        }

        // 2D: Aggression ramp near event start — boost size when start is imminent.
        if let Some(event_start) = signal.event_start_time {
            let secs_until_start = event_start - chrono::Utc::now().timestamp();
            let ramp_window: i64 = std::env::var("AGGRESSION_RAMP_SECS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(300);
            if secs_until_start > 0 && secs_until_start <= ramp_window {
                let ramp_mult: f64 = std::env::var("AGGRESSION_RAMP_MULT")
                    .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.3).clamp(1.0, 3.0);
                size_usdc *= ramp_mult;
                info!(
                    token_id = %signal.token_id,
                    secs_until_start,
                    ramp_mult,
                    "🚀 Aggression ramp — event start imminent, boosting size"
                );
            }
        }

        // 1A: Secondary wallet weight — scale size by the wallet's configured weight.
        if signal.wallet_weight != 1.0 {
            size_usdc *= signal.wallet_weight.clamp(0.0, 2.0);
        }

        let min_trade_usdc = std::env::var("PAPER_MIN_TRADE_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(2.0)
            .max(1.0);
        if size_usdc < min_trade_usdc {
            let mut p = self.portfolio.lock().await;
            p.skipped_orders += 1;
            self.record_rejection("size_after_throttle").await;
            return;
        }

        // Pre-trade liquidity guard.
        let (possibly_downsized, liq_status) = self.check_liquidity_guard(&signal.token_id, signal.side, size_usdc);
        let size_usdc = match possibly_downsized {
            Some(s) => s,
            None => {
                if let Some(ref log) = self.activity {
                    log_push(log, EntryKind::Skip, "Skipped — liquidity guard reject".to_string());
                }
                self.record_rejection("liquidity_reject").await;
                return;
            }
        };
        if liq_status == "downsized" {
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("Liquidity guard downsized to ${:.2}", size_usdc));
            }
            self.record_rejection("liquidity_downsize").await;
        }

        // 1C: Order book imbalance filter — avoid buying into heavy sell pressure.
        {
            let imbalance_gate = std::env::var("IMBALANCE_GATE")
                .ok().map(|v| v != "false" && v != "0").unwrap_or(true);
            if imbalance_gate {
                if let Some(book) = self.book_store.get_book_snapshot(&signal.token_id) {
                    let bid_depth: f64 = book.bids.values().map(|&s| s as f64).sum::<f64>() / 1_000.0;
                    let ask_depth: f64 = book.asks.values().map(|&s| s as f64).sum::<f64>() / 1_000.0;
                    let total = bid_depth + ask_depth;
                    if total > 0.0 {
                        let imbalance = (bid_depth - ask_depth) / total; // −1 (sell-heavy) to +1 (buy-heavy)
                        let blocked = match signal.side {
                            OrderSide::Buy  => imbalance < -0.15,
                            OrderSide::Sell => imbalance > 0.15,
                        };
                        if blocked {
                            warn!(
                                token_id = %signal.token_id,
                                imbalance = %format!("{:.3}", imbalance),
                                "⏭️  Imbalance gate — adverse book pressure"
                            );
                            let mut p = self.portfolio.lock().await;
                            p.skipped_orders += 1;
                            drop(p);
                            self.record_rejection("imbalance_gate").await;
                            return;
                        }
                    }
                }
            }
        }

        // 1D: Signal confidence composite score gate.
        {
            let floor = std::env::var("ALPHA_CONFIDENCE_FLOOR")
                .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0).clamp(0.0, 1.0);
            if floor > 0.0 {
                let recency_ms = signal.detected_at.elapsed().as_millis() as f64;
                let confidence = self.compute_signal_confidence(
                    &signal.token_id, signal.side, entry_price, size_usdc, recency_ms,
                );
                if confidence < floor {
                    warn!(
                        token_id   = %signal.token_id,
                        confidence = %format!("{:.3}", confidence),
                        floor      = %format!("{:.3}", floor),
                        "⏭️  Signal skipped — confidence below floor"
                    );
                    let mut p = self.portfolio.lock().await;
                    p.skipped_orders += 1;
                    drop(p);
                    self.record_rejection("low_confidence").await;
                    return;
                }
            }
        }

        // 3D: Dynamic concurrent position cap — scales with win rate and cash.
        {
            let p = self.portfolio.lock().await;
            let n_positions = p.positions.len();
            let win_trades  = p.closed_trades.iter().filter(|t| t.realized_pnl > 0.0).count();
            let total_closed = p.closed_trades.len().max(1);
            let cash_pct     = p.cash_usdc / p.nav().max(1.0);
            drop(p);

            let base: f64 = std::env::var("DYNAMIC_POS_CAP_BASE")
                .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(5.0).clamp(1.0, 50.0);
            let min_cap: usize = std::env::var("DYNAMIC_POS_CAP_MIN")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(1);
            let max_cap: usize = std::env::var("DYNAMIC_POS_CAP_MAX")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(10);

            let win_rate    = win_trades as f64 / total_closed as f64;
            let win_factor  = (0.5 + win_rate).clamp(0.5, 1.5);
            let cash_factor = (cash_pct / 0.5).clamp(0.5, 1.5);
            let dynamic_cap = ((base * win_factor * cash_factor) as usize).clamp(min_cap, max_cap);

            if n_positions >= dynamic_cap {
                warn!(
                    n_positions,
                    dynamic_cap,
                    win_rate   = %format!("{:.2}", win_rate),
                    cash_pct   = %format!("{:.0}%", cash_pct * 100.0),
                    "⏭️  Dynamic position cap reached"
                );
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("dynamic_pos_cap").await;
                return;
            }
        }

        // ── Risk check ────────────────────────────────────────────────────
        let position_count = {let p = self.portfolio.lock().await; p.positions.len()};
        if let Err(violation) = self.risk.lock().unwrap().check_pre_order(
            size_usdc, position_count,
            current_nav, STARTING_BALANCE_USDC,
        ) {
            warn!("🛑 Risk check blocked paper order: {violation}");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("RISK BLOCKED: {violation}"));
            }
            self.record_rejection("risk_blocked").await;
            return;
        }

        // ── Fill window (adaptive, no lock held during sleep) ──────────────

        // Per-token drift-abort cooldown: skip tokens that recently fired a drift
        // abort to prevent cascading redundant aborts on the same volatile event.
        let cooldown_secs = std::env::var("DRIFT_ABORT_COOLDOWN_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);
        {
            let mut cooldowns = self.drift_abort_cooldown.lock().unwrap();
            // Evict expired entries
            cooldowns.retain(|_, t| t.elapsed().as_secs() < cooldown_secs);
            if cooldowns.contains_key(&signal.token_id) {
                let mut p = self.portfolio.lock().await;
                p.skipped_orders += 1;
                drop(p);
                self.record_rejection("drift_cooldown").await;
                return;
            }
        }

        let filled = self
            .check_fill_window(&signal.token_id, entry_price, signal.side)
            .await;

        if !filled {
            // Record this token in the abort cooldown map
            self.drift_abort_cooldown
                .lock()
                .unwrap()
                .insert(signal.token_id.clone(), Instant::now());
            let mut p = self.portfolio.lock().await;
            p.aborted_orders += 1;
            warn!(
                token_id = %signal.token_id,
                "🛑 Paper order ABORTED — price drift exceeded 1.5% during fill window"
            );
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Abort, format!(
                    "ABORTED — price moved >1.5% during 3s fill window  token={:.12}…",
                    &signal.token_id
                ));
            }
            self.record_rejection("drift_abort").await;
            return;
        }

        // 2B: Depth-weighted VWAP slippage — walk the order book to compute realistic fill price.
        let (slippage_bps, vwap_price) = self.compute_vwap_slippage(&signal.token_id, signal.side, size_usdc);
        if vwap_price > 0.0 {
            entry_price = vwap_price.clamp(0.001, 0.999);
        }
        let variant = if self.experiments.lock().unwrap().sizing_variant_b { "B" } else { "A" };

        // ── Record virtual fill ───────────────────────────────────────
        self.fill_latency
            .lock()
            .unwrap()
            .record(signal.detected_at.elapsed());
        let pos_id = {
            let mut p = self.portfolio.lock().await;
            p.open_position_with_meta(
                signal.token_id.clone(),
                signal.market_title.clone(),
                signal.market_outcome.clone(),
                signal.side,
                entry_price,
                size_usdc,
                signal.order_id.clone(),
                slippage_bps,
                queue_delay_ms,
                variant,
                signal.event_start_time,
                signal.event_end_time,
            )
        };
        // Record fill in risk manager for VaR tracking (does not affect daily P&L).
        self.risk.lock().unwrap().record_fill(size_usdc);

        // 2C: Mark this (token, side) as recently filled to prevent tranche re-entry.
        {
            let dedup_key = format!("{}:{}", signal.token_id, signal.side);
            self.recent_fills.lock().unwrap().insert(dedup_key, Instant::now());
        }

        // Ensure this token is subscribed in the WS feed so get_market_price() stays live.
        {
            let mut subs = self.market_subscriptions.lock().unwrap();
            if !subs.contains(&signal.token_id) {
                subs.push(signal.token_id.clone());
                self.ws_force_reconnect.store(true, std::sync::atomic::Ordering::Relaxed);
                tracing::info!(token_id = %signal.token_id, "📡 Added token to WS subscriptions");
            }
        }

        self.shadow_comparator.lock().await.observations.push(ShadowFillObservation {
            token_id: signal.token_id.clone(),
            order_id: signal.order_id.clone(),
            side: signal.side,
            expected_price: self.get_market_price(&signal.token_id).unwrap_or(entry_price),
            paper_fill_price: entry_price,
            timestamp_ms: Utc::now().timestamp_millis(),
        });

        let shares = size_usdc / entry_price;
        {
            let p = self.portfolio.lock().await;
            info!(
                pos_id         = pos_id,
                token_id       = %signal.token_id,
                side           = %signal.side,
                entry_price    = %format!("{:.3}", entry_price),
                shares         = %format!("{:.4}", shares),
                usdc_spent     = %format!("${:.2}", size_usdc),
                cash_remaining = %format!("${:.2}", p.cash_usdc),
                nav            = %format!("${:.2}", p.nav()),
                fee_cat        = %fee_category,
                fee_rate       = %format!("{:.1}%", fee_rate * 100.0),
                "✅ Paper order FILLED"
            );
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Fill, format!(
                    "FILLED #{} {} @{:.3}  {:.4} shares  ${:.2} spent  cash=${:.2}  NAV=${:.2}",
                    pos_id, signal.side, entry_price, shares,
                    size_usdc, p.cash_usdc, p.nav()
                ));
            }
        }

        // Only print text dashboard when not in TUI mode.
        if self.activity.is_none() {
            self.print_dashboard().await;
        }

        // Immediate save on position open — don't wait for the 10s timer.
        {
            let state_path = std::env::var("PAPER_STATE_PATH")
                .unwrap_or_else(|_| "logs\\paper_portfolio_state.json".to_string());
            let _ = self.save_portfolio(&state_path).await;
        }
    }

    pub async fn backfill_position_metadata(&self) -> usize {
        let targets: Vec<(usize, String)> = {
            let p = self.portfolio.lock().await;
            p.positions
                .iter()
                .enumerate()
                .filter_map(|(idx, pos)| {
                    let title_ok = pos.market_title.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                    let outcome_ok = pos.market_outcome.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                    if title_ok && outcome_ok {
                        None
                    } else {
                        Some((idx, pos.token_id.clone()))
                    }
                })
                .collect()
        };

        let mut resolved: Vec<(usize, Option<String>, Option<String>)> = Vec::with_capacity(targets.len());
        for (idx, token_id) in targets {
            if let Some((title, outcome)) = self.lookup_signal_metadata(&token_id, None).await {
                resolved.push((idx, title, outcome));
            }
        }

        let mut updated = 0usize;
        let mut p = self.portfolio.lock().await;
        for (idx, title, outcome) in resolved {
            let Some(pos) = p.positions.get_mut(idx) else {
                continue;
            };
            if pos.market_title.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) && title.is_some() {
                pos.market_title = title;
                updated += 1;
            }
            if pos.market_outcome.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) && outcome.is_some() {
                pos.market_outcome = outcome;
            }
        }
        updated
    }

    /// Print the P&L dashboard (refreshes all current prices first).
    pub async fn print_dashboard(&self) {
        // Gather current prices outside the lock to avoid deadlock.
        let token_prices = {
            let p      = self.portfolio.lock().await;
            let tokens: Vec<String> = p.positions.iter().map(|pos| pos.token_id.clone()).collect();
            tokens
                .into_iter()
                .map(|t| {
                    let price = self.get_market_price(&t);
                    (t, price)
                })
                .collect::<Vec<_>>()
        };

        // Apply price updates.
        {
            let mut p = self.portfolio.lock().await;
            for (token_id, price) in &token_prices {
                if let Some(pr) = price {
                    p.update_price(token_id, *pr);
                }
            }
        }

        let p         = self.portfolio.lock().await;
        let nav       = p.nav();
        let nav_delta = nav - STARTING_BALANCE_USDC;
        let nav_pct   = nav_delta / STARTING_BALANCE_USDC * 100.0;

        println!();
        println!("╔════════════════════════════════════════════════════════════╗");
        println!("║            📄  BLINK PAPER TRADING DASHBOARD              ║");
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║  Cash:             ${:<10.2} USDC                        ║", p.cash_usdc);
        println!("║  Invested:         ${:<10.2} USDC                        ║", p.total_invested());
        println!("║  Unrealized P&L:   {:>+10.4} USDC                        ║", p.unrealized_pnl());
        println!("║  Realized P&L:     {:>+10.4} USDC                        ║", p.realized_pnl());
        println!("║  ─────────────────────────────────────────────────────    ║");
        println!("║  NAV:              ${:<8.2} ({:>+.2}%)                    ║", nav, nav_pct);
        println!("╠════════════════════════════════════════════════════════════╣");
        println!(
            "║  Signals: {:>3}  │  Filled: {:>3}  │  Aborted: {:>3}  │  Skipped: {:>3}  ║",
            p.total_signals, p.filled_orders, p.aborted_orders, p.skipped_orders
        );

        if !p.positions.is_empty() {
            println!("╠════════════════════════════════════════════════════════════╣");
            println!("║  OPEN POSITIONS                                            ║");
            for pos in &p.positions {
                let age_s   = pos.opened_at.elapsed().as_secs();
                let upnl    = pos.unrealized_pnl();
                let upnl_pc = pos.unrealized_pnl_pct();
                // Truncate token_id for display (first 12 chars + "…")
                let tid_short = if pos.token_id.len() > 14 {
                    format!("{}…", &pos.token_id[..13])
                } else {
                    pos.token_id.clone()
                };
                println!(
                    "║  #{:<3} {} {} @{:.3} → {:.3} | {:>6.2}sh | {:>+.3}$ ({:>+.1}%) | {:>4}s  ║",
                    pos.id, pos.side, tid_short,
                    pos.entry_price, pos.current_price,
                    pos.shares, upnl, upnl_pc, age_s,
                );
            }
        } else {
            println!("║  No open positions.                                        ║");
        }

        if !p.closed_trades.is_empty() {
            println!("╠════════════════════════════════════════════════════════════╣");
            println!("║  CLOSED TRADES (last 5)                                    ║");
            for trade in p.closed_trades.iter().rev().take(5) {
                println!(
                    "║  {} @{:.3}→{:.3} | {:>+.3}$ ({})                     ║",
                    trade.side, trade.entry_price, trade.exit_price,
                    trade.realized_pnl, trade.reason,
                );
            }
        }

        println!("╚════════════════════════════════════════════════════════════╝");
        println!();
    }

    // ── Private helpers ───────────────────────────────────────────────────

    /// Simulate fill window with optional drift checking.
    ///
    /// Defaults to **immediate fill** (0 ms) for ultra-low-latency paper mode.
    /// Set `PAPER_FILL_WINDOW_MS > 0` to re-enable timed drift checks.
    async fn check_fill_window(
        &self,
        token_id:    &str,
        entry_price: f64,
        side:        OrderSide,
    ) -> bool {
        let realism_mode = std::env::var("PAPER_REALISM_MODE")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let base_countdown_ms = std::env::var("PAPER_FILL_WINDOW_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let effective_countdown_ms = if realism_mode {
            base_countdown_ms.max(1200)
        } else {
            base_countdown_ms
        };
        let base_check_interval_ms = std::env::var("PAPER_FILL_CHECK_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(100)
            .max(1);
        let (countdown_ms, check_interval_ms) =
            self.adaptive_fill_policy(token_id, effective_countdown_ms, base_check_interval_ms, entry_price);
        let countdown = Duration::from_millis(countdown_ms);

        if countdown_ms == 0 {
            self.fill_window.lock().unwrap().take();
            return true;
        }

        let started_at = Instant::now();
        self.fill_window.lock().unwrap().replace(FillWindowSnapshot::new(
            token_id.to_string(),
            side,
            entry_price,
            countdown,
        ));

        let checks = (countdown_ms / check_interval_ms).max(1);
        for check in 0..checks {
            sleep(Duration::from_millis(check_interval_ms)).await;

            let elapsed = started_at.elapsed();
            if let Some(current) = self.get_market_price(token_id) {
                let drift = (current - entry_price).abs() / entry_price;
                let drift_pct = drift * 100.0;
                self.volatility_state.lock().unwrap().push(current);
                self.fill_window.lock().unwrap().replace(FillWindowSnapshot {
                    token_id: token_id.to_string(),
                    side,
                    entry_price,
                    current_price: Some(current),
                    drift_pct: Some(drift_pct),
                    elapsed,
                    countdown,
                });
                if drift > DRIFT_THRESHOLD {
                    warn!(
                        check        = check,
                        entry_price  = %format!("{:.3}", entry_price),
                        current      = %format!("{:.3}", current),
                        drift_pct    = %format!("{:.2}%", drift * 100.0),
                        "🚨 Fill window abort: price drifted"
                    );
                    self.fill_window.lock().unwrap().take();
                    return false;
                }
            } else {
                self.fill_window.lock().unwrap().replace(FillWindowSnapshot {
                    token_id: token_id.to_string(),
                    side,
                    entry_price,
                    current_price: None,
                    drift_pct: None,
                    elapsed,
                    countdown,
                });
            }
        }
        self.fill_window.lock().unwrap().take();
        true
    }

    fn adaptive_fill_policy(
        &self,
        _token_id: &str,
        base_window_ms: u64,
        base_check_ms: u64,
        reference_price: f64,
    ) -> (u64, u64) {
        let vol_bps = self.volatility_state.lock().unwrap().volatility_bps();
        let drift_mult = if let Some(mid) = self.get_market_price(_token_id) {
            ((mid - reference_price).abs() / reference_price).clamp(0.0, 0.05)
        } else {
            0.0
        };
        let mut window = base_window_ms as f64;
        let mut check = base_check_ms as f64;
        if vol_bps > 120.0 || drift_mult > 0.01 {
            window *= 0.6;
            check *= 0.7;
        } else if vol_bps < 25.0 && drift_mult < 0.0025 {
            window *= 1.4;
            check *= 1.3;
        }
        (window.max(0.0) as u64, check.max(1.0) as u64)
    }

    fn compute_edge_score(&self, signal: &RN1Signal) -> f64 {
        let entry_price = signal.price as f64 / 1_000.0;
        let shares = signal.size as f64 / 1_000.0;
        let notional = shares * entry_price;
        let recency_ms = signal.detected_at.elapsed().as_millis() as f64;
        let mut spread_bps = 0.0;
        let mut depth = 0.0;
        if let Some(book) = self.book_store.get_book_snapshot(&signal.token_id) {
            spread_bps = book.spread_bps().unwrap_or(0) as f64;
            depth = match signal.side {
                OrderSide::Buy => book.asks.iter().next().map(|(_, s)| *s as f64 / 1_000.0).unwrap_or(0.0),
                OrderSide::Sell => book.bids.iter().next_back().map(|(_, s)| *s as f64 / 1_000.0).unwrap_or(0.0),
            };
        }
        (notional * 0.45) + (depth * 0.30) + ((500.0 - spread_bps).max(0.0) * 0.15) + ((5_000.0 - recency_ms).max(0.0) * 0.10)
    }

    fn check_liquidity_guard(&self, token_id: &str, side: OrderSide, size_usdc: f64) -> (Option<f64>, &'static str) {
        let realism_mode = std::env::var("PAPER_REALISM_MODE")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let min_trade_usdc = std::env::var("PAPER_MIN_TRADE_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(2.0)
            .max(1.0);
        let depth_capture_ratio = std::env::var("PAPER_DEPTH_CAPTURE_RATIO")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(if realism_mode { 0.60 } else { 0.90 })
            .clamp(0.10, 1.00);
        let thin_book_fallback = std::env::var("PAPER_THIN_BOOK_FALLBACK_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(6.0)
            .max(min_trade_usdc);
        let hard_reject_enabled = std::env::var("PAPER_LIQUIDITY_HARD_REJECT")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(realism_mode);

        let Some((price, level_size)) = self.book_store.top_of_book(token_id, side) else {
            if hard_reject_enabled {
                return (None, "reject");
            }
            return (Some(thin_book_fallback.min(size_usdc)), "downsized");
        };
        let px = price as f64 / 1_000.0;
        let depth_usdc = (level_size as f64 / 1_000.0) * px;
        if depth_usdc <= 0.0 {
            if hard_reject_enabled {
                return (None, "reject");
            }
            return (Some(thin_book_fallback.min(size_usdc)), "downsized");
        }
        if size_usdc <= depth_usdc {
            return (Some(size_usdc), "ok");
        }
        let captured = depth_usdc * depth_capture_ratio;
        if captured >= min_trade_usdc {
            return (Some(captured.min(size_usdc)), "downsized");
        }
        if !hard_reject_enabled {
            return (Some(thin_book_fallback.min(size_usdc)), "downsized");
        }
        (None, "reject")
    }

    /// 2B: Depth-weighted VWAP slippage — walks the order book to simulate realistic fill cost.
    ///
    /// Returns `(slippage_bps, vwap_price)` where `vwap_price` is 0.0 when the book is empty
    /// on the relevant side (caller should keep the original entry_price).
    /// Falls back to a sqrt market-impact model when our side has no depth.
    fn compute_vwap_slippage(&self, token_id: &str, side: OrderSide, size_usdc: f64) -> (f64, f64) {
        let exponent: f64 = std::env::var("SLIPPAGE_IMPACT_EXPONENT")
            .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.5).clamp(0.1, 1.0);
        let coeff: f64 = std::env::var("SLIPPAGE_IMPACT_COEFF_BPS")
            .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(50.0).clamp(0.0, 500.0);

        let Some(book) = self.book_store.get_book_snapshot(token_id) else {
            return (0.0, 0.0);
        };

        let ref_price_u = match side {
            OrderSide::Buy  => book.best_ask(),
            OrderSide::Sell => book.best_bid(),
        };

        let Some(ref_price_u) = ref_price_u else {
            // Our side is empty — fall back to sqrt market-impact over total book depth.
            let total_depth_usdc: f64 = book.bids.iter()
                .chain(book.asks.iter())
                .map(|(p, s)| (*p as f64 / 1_000.0) * (*s as f64 / 1_000.0))
                .sum::<f64>()
                .max(1.0);
            let impact = coeff * (size_usdc / total_depth_usdc).powf(exponent);
            return (impact.clamp(0.0, 500.0), 0.0);
        };

        let ref_price = ref_price_u as f64 / 1_000.0;

        // Walk the book and consume `size_usdc` notional.
        let levels: Vec<(f64, f64)> = match side {
            OrderSide::Buy  => book.asks.iter()
                .map(|(p, s)| (*p as f64 / 1_000.0, *s as f64 / 1_000.0))
                .collect(),
            OrderSide::Sell => book.bids.iter().rev()
                .map(|(p, s)| (*p as f64 / 1_000.0, *s as f64 / 1_000.0))
                .collect(),
        };

        let mut remaining   = size_usdc;
        let mut total_cost  = 0.0_f64;
        let mut total_shares = 0.0_f64;
        for (px, shares) in levels {
            if remaining <= 0.0 { break; }
            let level_usdc   = shares * px;
            let filled_usdc  = remaining.min(level_usdc);
            let filled_shares = filled_usdc / px;
            total_cost   += filled_shares * px;
            total_shares += filled_shares;
            remaining    -= filled_usdc;
        }

        if total_shares <= 0.0 || ref_price <= 0.0 {
            return (0.0, ref_price);
        }

        let vwap     = total_cost / total_shares;
        let slip_bps = ((vwap - ref_price).abs() / ref_price * 10_000.0).clamp(0.0, 500.0);
        (slip_bps, vwap)
    }

    /// 1D: Composite signal confidence score [0.0, 1.0].
    ///
    /// Combines book depth coverage, spread quality, signal recency, and price certainty.
    fn compute_signal_confidence(
        &self,
        token_id:   &str,
        side:       OrderSide,
        entry_price: f64,
        size_usdc:  f64,
        recency_ms: f64,
    ) -> f64 {
        let (depth_score, spread_score) = if let Some(book) = self.book_store.get_book_snapshot(token_id) {
            let side_depth_usdc: f64 = match side {
                OrderSide::Buy  => book.asks.iter()
                    .map(|(p, s)| (*p as f64 / 1_000.0) * (*s as f64 / 1_000.0))
                    .sum(),
                OrderSide::Sell => book.bids.iter()
                    .map(|(p, s)| (*p as f64 / 1_000.0) * (*s as f64 / 1_000.0))
                    .sum(),
            };
            let d = (side_depth_usdc / size_usdc.max(1.0) / 10.0).clamp(0.0, 1.0);
            let spread = book.spread_bps().unwrap_or(500) as f64;
            let s = (1.0 - spread / 500.0).clamp(0.0, 1.0);
            (d, s)
        } else {
            (0.3, 0.3) // no book data — moderate penalty
        };
        let recency_score  = (1.0 - recency_ms / 5_000.0).clamp(0.0, 1.0);
        // Prices far from 0.5 are more certain; near 0.5 outcome is least predictable.
        let certainty      = (2.0 * (entry_price - 0.5).abs()).clamp(0.0, 1.0);
        let price_score    = 0.3 + 0.7 * certainty;

        depth_score * 0.35 + spread_score * 0.25 + recency_score * 0.25 + price_score * 0.15
    }

    fn estimate_slippage_bps(&self, token_id: &str, side: OrderSide, fill_price: f64) -> f64 {
        let ref_price = self
            .book_store
            .top_of_book(token_id, side)
            .map(|(p, _)| p as f64 / 1_000.0)
            .unwrap_or(fill_price);
        if ref_price <= 0.0 {
            return 0.0;
        }
        ((fill_price - ref_price).abs() / ref_price) * 10_000.0
    }

    async fn record_rejection(&self, reason: &str) {
        let mut rej = self.rejection_analytics.lock().await;
        rej.schema_version = 1;
        rej.reasons
            .entry(reason.to_string())
            .or_default()
            .push(Utc::now().timestamp());
    }

    pub async fn rejection_trend_24h(&self) -> HashMap<String, Vec<RejectionTrendPoint>> {
        let now = Utc::now().timestamp();
        let min_ts = now - 24 * 3600;
        let rej = self.rejection_analytics.lock().await;
        let mut out = HashMap::new();
        for (reason, timestamps) in &rej.reasons {
            let mut buckets: HashMap<i64, usize> = HashMap::new();
            for ts in timestamps.iter().copied().filter(|ts| *ts >= min_ts) {
                let hour = ts - (ts % 3600);
                *buckets.entry(hour).or_insert(0) += 1;
            }
            let mut pts: Vec<RejectionTrendPoint> = buckets
                .into_iter()
                .map(|(h, c)| RejectionTrendPoint { hour_utc_epoch: h, count: c })
                .collect();
            pts.sort_by_key(|p| p.hour_utc_epoch);
            out.insert(reason.clone(), pts);
        }
        out
    }

    pub async fn execution_summary(&self) -> ExecutionSummary {
        let realism_gap = self.shadow_realism_gap_bps().await;
        let p = self.portfolio.lock().await;
        if p.closed_trades.is_empty() {
            return ExecutionSummary::default();
        }
        let mut total_slip = 0.0;
        let mut total_delay = 0.0;
        let mut tags: HashMap<String, usize> = HashMap::new();
        let attempts = (p.filled_orders + p.aborted_orders + p.skipped_orders).max(1) as f64;
        for t in &p.closed_trades {
            total_slip += t.scorecard.slippage_bps;
            total_delay += t.scorecard.queue_delay_ms as f64;
            for tag in &t.scorecard.outcome_tags {
                *tags.entry(tag.clone()).or_insert(0) += 1;
            }
        }
        ExecutionSummary {
            trades: p.closed_trades.len(),
            fill_rate_pct: (p.filled_orders as f64 / attempts) * 100.0,
            reject_rate_pct: ((p.skipped_orders + p.aborted_orders) as f64 / attempts) * 100.0,
            avg_slippage_bps: total_slip / p.closed_trades.len() as f64,
            avg_queue_delay_ms: total_delay / p.closed_trades.len() as f64,
            shadow_realism_gap_bps: realism_gap,
            tags,
        }
    }

    pub async fn shadow_realism_gap_bps(&self) -> f64 {
        let comp = self.shadow_comparator.lock().await;
        if comp.observations.is_empty() {
            return 0.0;
        }
        let sum = comp.observations.iter().map(|o| {
            if o.expected_price <= 0.0 {
                0.0
            } else {
                ((o.paper_fill_price - o.expected_price).abs() / o.expected_price) * 10_000.0
            }
        }).sum::<f64>();
        sum / comp.observations.len() as f64
    }

    pub async fn experiment_metrics(&self) -> ExperimentMetrics {
        let p = self.portfolio.lock().await;
        let mut m = ExperimentMetrics::default();
        for pos in &p.positions {
            if pos.experiment_variant == "B" {
                m.variant_b_fills += 1;
            } else {
                m.variant_a_fills += 1;
            }
        }
        for t in &p.closed_trades {
            let is_b = t
                .scorecard
                .outcome_tags
                .iter()
                .any(|t| t == "variant:B");
            if is_b {
                m.variant_b_realized_pnl += t.realized_pnl;
            } else {
                m.variant_a_realized_pnl += t.realized_pnl;
            }
        }
        m
    }

    pub fn experiment_switches(&self) -> ExperimentSwitches {
        self.experiments.lock().unwrap().clone()
    }

    pub fn set_experiment_switches(&self, switches: ExperimentSwitches) {
        *self.experiments.lock().unwrap() = switches;
    }

    pub fn experiment_switches_handle(&self) -> Arc<std::sync::Mutex<ExperimentSwitches>> {
        Arc::clone(&self.experiments)
    }

    pub async fn save_warm_state(
        &self,
        path: &str,
        market_subscriptions: &[String],
        portfolio_path: &str,
    ) -> std::io::Result<()> {
        let books = self.book_store.all_snapshots();
        let rejections = self.rejection_analytics.lock().await.clone();
        let comparator = self.shadow_comparator.lock().await.clone();
        let experiments = self.experiments.lock().unwrap().clone();
        let mut state = WarmState {
            schema_version: 1,
            saved_at_ms: Utc::now().timestamp_millis(),
            market_subscriptions: market_subscriptions.to_vec(),
            order_books: books,
            portfolio_path: portfolio_path.to_string(),
            rejection_analytics: rejections,
            comparator,
            experiments,
            checksum: 0,
        };
        state.checksum = warm_state_checksum(&state);
        atomic_write_with_backup(path, &state)
    }

    pub async fn load_warm_state_if_present(
        &self,
        path: &str,
        market_subscriptions: &Arc<std::sync::Mutex<Vec<String>>>,
    ) -> std::io::Result<bool> {
        let Some(raw) = read_json_with_fallback(path)? else {
            return Ok(false);
        };
        let mut state: WarmState = serde_json::from_value(raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let expected = state.checksum;
        state.checksum = 0;
        if warm_state_checksum(&state) != expected {
            return Ok(false);
        }
        self.book_store.restore_snapshots(&state.order_books);
        {
            let mut subs = market_subscriptions.lock().unwrap();
            *subs = state.market_subscriptions.clone();
        }
        *self.rejection_analytics.lock().await = state.rejection_analytics;
        *self.shadow_comparator.lock().await = state.comparator;
        *self.experiments.lock().unwrap() = state.experiments;
        Ok(true)
    }

    /// Look up the current mid-price for a token from the live order book.
    /// Falls back to best bid/ask if only one side is present.
    /// Returns `None` only when no order book levels exist.
    #[inline]
    fn get_market_price(&self, token_id: &str) -> Option<f64> {
        self.book_store
            .get_mark_price(token_id)
            .map(|p| p as f64 / 1_000.0)
    }

    /// Updates all open position mark prices from the live order book store,
    /// then appends an equity curve sample. Call from a background timer (every
    /// ~1 s) to keep unrealized PnL and the equity chart live in web mode.
    pub async fn tick_mark_prices(&self) {
        let tick = self.equity_tick.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let should_push = tick % 1 == 0; // push every tick (called every 1s)
        // Use try_lock first to detect contention
        let mut p = match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            self.portfolio.lock(),
        ).await {
            Ok(guard) => guard,
            Err(_) => {
                tracing::warn!(tick, "tick_mark_prices: portfolio lock timeout (500ms) — skipping tick");
                return;
            }
        };
        if p.positions.is_empty() {
            if should_push {
                p.push_equity_snapshot();
                self.emit_equity_snapshot(&p);
            }
            return;
        }
        let updates: Vec<(String, f64)> = p
            .positions
            .iter()
            .filter_map(|pos| {
                self.get_market_price(&pos.token_id)
                    .map(|pr| (pos.token_id.clone(), pr))
            })
            .collect();
        for (token_id, price) in updates {
            p.update_price(&token_id, price);
        }
        if should_push {
            p.push_equity_snapshot();
            self.emit_equity_snapshot(&p);
        }
    }

    fn emit_equity_snapshot(&self, p: &PaperPortfolio) {
        let Some(ref tx) = self.warehouse_tx else { return };
        let nav = p.nav();
        let unrealised_pnl = nav - p.cash_usdc;
        let ev = EquitySnapshot {
            timestamp_ms:   crate::clickhouse_logger::now_ms(),
            nav_usdc:        nav,
            cash_usdc:       p.cash_usdc,
            unrealised_pnl,
            open_positions:  p.positions.len() as u32,
        };
        let _ = tx.try_send(WarehouseEvent::EquitySnapshot(ev));
    }

    async fn enrich_signal_metadata(&self, signal: &mut RN1Signal) {
        let title_ok = signal.market_title.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false);
        let outcome_ok = signal.market_outcome.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false);

        // Check cache for timing first (avoids redundant Gamma API calls)
        {
            let cache = self.signal_meta_cache.lock().await;
            if let Some(entry) = cache.get(&signal.token_id) {
                if signal.event_start_time.is_none() { signal.event_start_time = entry.event_start_time; }
                if signal.event_end_time.is_none()   { signal.event_end_time   = entry.event_end_time; }
            }
        }

        if title_ok && outcome_ok && signal.event_start_time.is_some() {
            return;
        }
        if let Some((title, outcome)) = self.lookup_signal_metadata(&signal.token_id, Some(&signal.order_id)).await {
            if signal.market_title.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                signal.market_title = title;
            }
            if signal.market_outcome.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                signal.market_outcome = outcome;
            }
        }
        // Enrich event timing from Gamma API if still not set
        if signal.event_start_time.is_none() || signal.event_end_time.is_none() {
            if let Some((start, end)) = self.fetch_event_timing(&signal.token_id).await {
                if signal.event_start_time.is_none() { signal.event_start_time = start; }
                if signal.event_end_time.is_none()   { signal.event_end_time   = end; }
                // Persist timing into cache so future signals for this token don't re-fetch
                let mut cache = self.signal_meta_cache.lock().await;
                if let Some(entry) = cache.get_mut(&signal.token_id) {
                    entry.event_start_time = signal.event_start_time;
                    entry.event_end_time   = signal.event_end_time;
                }
            }
        }
    }

    async fn lookup_signal_metadata(&self, token_id: &str, order_id: Option<&str>) -> Option<(Option<String>, Option<String>)> {
        const CACHE_TTL: Duration = Duration::from_secs(600);

        {
            let cache = self.signal_meta_cache.lock().await;
            if let Some(entry) = cache.get(token_id) {
                if entry.cached_at.elapsed() < CACHE_TTL {
                    return Some((entry.market_title.clone(), entry.market_outcome.clone()));
                }
            }
        }

        let Some((title, outcome)) = self.fetch_signal_metadata(token_id, order_id).await else {
            return None;
        };

        let mut cache = self.signal_meta_cache.lock().await;
        cache.insert(
            token_id.to_string(),
            CachedSignalMeta {
                market_title: title.clone(),
                market_outcome: outcome.clone(),
                event_start_time: None,
                event_end_time: None,
                cached_at: Instant::now(),
            },
        );
        Some((title, outcome))
    }

    async fn fetch_signal_metadata(&self, token_id: &str, order_id: Option<&str>) -> Option<(Option<String>, Option<String>)> {
        if self.rn1_wallet.trim().is_empty() {
            return None;
        }
        const PAGE_LIMIT: usize = 200;
        const MAX_PAGES: usize = 12;
        for page in 0..MAX_PAGES {
            let offset = page * PAGE_LIMIT;
            let url = format!(
                "https://data-api.polymarket.com/trades?wallet={}&limit={}&offset={}",
                self.rn1_wallet, PAGE_LIMIT, offset
            );
            let resp = match self.metadata_client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    warn!(token_id = %token_id, error = %e, "metadata fetch request failed");
                    return None;
                }
            };
            if !resp.status().is_success() {
                warn!(token_id = %token_id, status = %resp.status(), "metadata fetch returned non-success");
                return None;
            }
            let entries: Vec<TradeMetaEntry> = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    warn!(token_id = %token_id, error = %e, "metadata response parse failed");
                    return None;
                }
            };
            if entries.is_empty() {
                break;
            }

            if let Some(oid) = order_id {
                if let Some(hit) = entries.iter().find(|e| {
                    e.transaction_hash.as_deref() == Some(oid)
                        && e.asset.as_deref() == Some(token_id)
                }) {
                    return Some((normalize_opt(hit.title.clone()), normalize_opt(hit.outcome.clone())));
                }
            }

            if let Some(hit) = entries.iter().find(|e| e.asset.as_deref() == Some(token_id)) {
                return Some((normalize_opt(hit.title.clone()), normalize_opt(hit.outcome.clone())));
            }
        }
        None
    }

    /// Fetch event timing (game start + market end) from Gamma API.
    async fn fetch_event_timing(&self, token_id: &str) -> Option<(Option<i64>, Option<i64>)> {
        let url = format!(
            "https://gamma-api.polymarket.com/markets?token_id={}",
            token_id
        );
        let resp = match self.metadata_client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(token_id = %token_id, error = %e, "Gamma API timing fetch failed");
                return None;
            }
        };
        if !resp.status().is_success() {
            return None;
        }
        let data: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return None,
        };
        let markets = data.as_array()?;
        let market = markets.first()?;

        let parse_ts = |s: &str| -> Option<i64> {
            let s = s.trim();
            // Full ISO 8601 / RFC 3339
            chrono::DateTime::parse_from_rfc3339(s).ok()
                .map(|dt| dt.timestamp())
                // "YYYY-MM-DD HH:MM:SS" (Polymarket Gamma legacy)
                .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
                    .map(|ndt| ndt.and_utc().timestamp()))
                // "YYYY-MM-DDTHH:MM:SS" (no tz)
                .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()
                    .map(|ndt| ndt.and_utc().timestamp()))
                // Date-only "YYYY-MM-DD" → midnight UTC
                .or_else(|| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
                    .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()))
        };

        // Try all known Polymarket field name variants (snake_case AND camelCase)
        let get_str = |market: &serde_json::Value, keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| market.get(*k)?.as_str().map(|s| s.to_string()))
        };

        let event_start = get_str(market, &[
            "game_start_date", "gameStartDate",
            "start_date_iso",  "startDateIso",
            "start_date",      "startDate",
            "gameStartTime",   "game_start_time",
        ]).and_then(|s| parse_ts(&s));

        let event_end = get_str(market, &[
            "end_date_iso", "endDateIso",
            "end_date",     "endDate",
            "resolution_date", "resolutionDate",
        ]).and_then(|s| parse_ts(&s));

        Some((event_start, event_end))
    }

    /// Resets daily P&L and rate-limit counters in the risk manager.
    ///
    /// Call at UTC midnight via a scheduled task.
    pub fn reset_daily_risk(&self) {
        self.risk.lock().unwrap().reset_daily();
        if let Some(ref log) = self.activity {
            log_push(log, EntryKind::Engine,
                "🌅 Daily risk counters reset (UTC midnight)".to_string());
        }
        info!("Daily risk counters reset");
    }

    pub async fn run_autoclaim(&self) {
        let mut enabled = std::env::var("AUTOCLAIM_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);
        if self.experiments.lock().unwrap().autoclaim_variant_b {
            enabled = !enabled;
        }
        if !enabled {
            return;
        }

        let exit_config = ExitConfig::from_env();

        let mut p = self.portfolio.lock().await;
        if p.positions.is_empty() {
            return;
        }

        // Refresh prices from live order book before evaluating exits.
        // Positions without a live price keep their last-known current_price
        // so time-based exits (max_hold, stale, stagnant) still fire.
        let token_prices: Vec<(String, f64)> = p.positions
            .iter()
            .filter_map(|pos| self.get_market_price(&pos.token_id).map(|pr| (pos.token_id.clone(), pr)))
            .collect();
        for (token_id, price) in token_prices {
            p.update_price(&token_id, price);
        }

        // Collect which tokens currently have a live order book price.
        let live_tokens: std::collections::HashSet<String> = p.positions
            .iter()
            .filter(|pos| self.get_market_price(&pos.token_id).is_some())
            .map(|pos| pos.token_id.clone())
            .collect();

        // Delegate exit decisions to the pure exit_strategy module.
        // 4C: Per-category exit config override — evaluate each position with its own
        // patched ExitConfig so e.g. sports positions can have tighter stops than crypto.
        let decisions: Vec<crate::exit_strategy::ExitDecision> = p.positions
            .iter()
            .enumerate()
            .flat_map(|(real_idx, pos)| {
                let patched = patched_exit_config_for_category(&exit_config, pos.market_title.as_deref());
                evaluate_exits(std::slice::from_ref(pos), &patched, |tid| {
                    live_tokens.contains(tid)
                })
                .into_iter()
                .map(move |mut d| { d.position_idx = real_idx; d })
            })
            .collect();

        // 3C: emit warnings for positions approaching event close (not yet in force-close window).
        // 4A: update momentum reference prices for positions that aren't being closed.
        let now_ts_check = chrono::Utc::now().timestamp();
        let pre_event_warn_secs = 600i64;
        let pre_close_secs = exit_config.pre_event_close_secs as i64;
        let momentum_interval = exit_config.momentum_check_interval_secs as i64;
        let exiting_set: std::collections::HashSet<usize> =
            decisions.iter().map(|d| d.position_idx).collect();
        for pos in p.positions.iter() {
            if let Some(end_ts) = pos.event_end_time {
                let secs_left = end_ts - now_ts_check;
                if secs_left > pre_close_secs && secs_left <= pre_event_warn_secs {
                    let msg = format!(
                        "⏳ {}s to event close for {} — position approaching auto-close window",
                        secs_left,
                        pos.market_title.as_deref().unwrap_or(&pos.token_id)
                    );
                    if let Some(ref log) = self.activity {
                        log_push(log, EntryKind::Warn, msg.clone());
                    }
                    info!("{msg}");
                }
            }
        }
        for (i, pos) in p.positions.iter_mut().enumerate() {
            if !exiting_set.contains(&i)
                && now_ts_check - pos.momentum_ref_ts >= momentum_interval
            {
                pos.momentum_ref_price = pos.current_price;
                pos.momentum_ref_ts = now_ts_check;
            }
        }

        if decisions.is_empty() {
            return;
        }

        // Process decisions in reverse index order to preserve indices during removal.
        let mut sorted_decisions = decisions;
        sorted_decisions.sort_by(|a, b| b.position_idx.cmp(&a.position_idx));

        let mut total_realized = 0.0f64;
        let mut action_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();

        for decision in &sorted_decisions {
            let idx = decision.position_idx;
            if idx >= p.positions.len() {
                continue;
            }

            let reason = decision.action.reason();
            let fraction = decision.action.fraction();

            let action_key = match &decision.action {
                ExitAction::TakeProfit { .. } => "AUTOCLAIM",
                ExitAction::StopLoss { .. } => "STOP-LOSS",
                ExitAction::TrailingStop { .. } => "TRAILING-STOP",
                ExitAction::StagnantExit { .. } => "STAGNANT-EXIT",
                ExitAction::Resolved { .. } => "RESOLVED",
                ExitAction::MarketNotLive { .. } => "STALE-CLOSE",
                ExitAction::MaxHoldExpired { .. } => "MAX-HOLD",
                ExitAction::PreResolutionStop { .. } => "PRE-RESOLUTION",
                ExitAction::PreEventClose { .. } => "PRE-EVENT-CLOSE",
                ExitAction::AdverseMomentum { .. } => "ADVERSE-MOMENTUM",
            };
            *action_counts.entry(action_key).or_insert(0) += 1;

            let before = p.closed_trades.len();
            let _removed = p.close_position_fraction(idx, fraction, reason);

            // Emit ClosedTrade events to ClickHouse for each newly closed slice.
            if let Some(ref tx) = self.warehouse_tx {
                for ct in &p.closed_trades[before..] {
                    let ev = ClosedTradeFull {
                        timestamp_ms:   crate::clickhouse_logger::now_ms(),
                        token_id:       ct.token_id.clone(),
                        market_title:   ct.market_title.clone().unwrap_or_default(),
                        side:           format!("{:?}", ct.side),
                        entry_price:    ct.entry_price,
                        exit_price:     ct.exit_price,
                        shares:         ct.shares,
                        realized_pnl:   ct.realized_pnl,
                        fees_paid_usdc: ct.fees_paid_usdc,
                        duration_secs:  ct.duration_secs,
                        reason:         ct.reason.clone(),
                    };
                    let _ = tx.try_send(WarehouseEvent::ClosedTrade(ev));
                }
            }

            // Sum realized P&L from newly closed trades.
            let slice_pnl: f64 = p.closed_trades[before..].iter().map(|t| t.realized_pnl).sum();
            total_realized += slice_pnl;
        }

        // Update risk manager with total realized P&L.
        if total_realized.abs() > f64::EPSILON {
            self.risk.lock().unwrap().record_close(total_realized);
        }

        // Log summary per action type.
        for (action_key, count) in &action_counts {
            let msg = format!("{}: {} position(s) closed  total_pnl={:+.2}", action_key, count, total_realized);
            if let Some(ref log) = self.activity {
                let kind = if *action_key == "STOP-LOSS" { EntryKind::Warn } else { EntryKind::Engine };
                log_push(log, kind, msg.clone());
            }
            info!("{msg}");
        }

        // Immediate save after any position close.
        if !action_counts.is_empty() {
            let state_path = std::env::var("PAPER_STATE_PATH")
                .unwrap_or_else(|_| "logs\\paper_portfolio_state.json".to_string());
            let _ = self.save_portfolio(&state_path).await;
        }
    }
}

fn normalize_opt(v: Option<String>) -> Option<String> {
    v.and_then(|s| {
        let t = s.trim();
        if t.is_empty() || t == "?" {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn env_flag(key: &str) -> bool {
    std::env::var(key)
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

pub(crate) fn parse_autoclaim_tiers() -> Vec<(f64, f64)> {
    let raw = std::env::var("AUTOCLAIM_TIERS").unwrap_or_else(|_| "40:0.30,70:0.30,100:1.0".to_string());
    let mut out: Vec<(f64, f64)> = raw
        .split(',')
        .filter_map(|item| {
            let mut parts = item.split(':');
            let a = parts.next()?.trim().parse::<f64>().ok()?;
            let b = parts.next()?.trim().parse::<f64>().ok()?;
            Some((a, b.clamp(0.0, 1.0)))
        })
        .collect();
    out.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(Ordering::Equal));
    if out.is_empty() {
        out.push((100.0, 1.0));
    }
    out
}

fn warm_state_checksum(state: &WarmState) -> u64 {
    let json = serde_json::to_vec(state).unwrap_or_default();
    json.iter()
        .fold(0u64, |acc, b| acc.wrapping_mul(131).wrapping_add(*b as u64))
}

fn atomic_write_with_backup<T: Serialize>(path: &str, value: &T) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let tmp_path = format!("{path}.tmp");
    std::fs::write(&tmp_path, &json)?;
    let backup1 = format!("{path}.bak1");
    let backup2 = format!("{path}.bak2");
    if std::path::Path::new(&backup1).exists() {
        let _ = std::fs::rename(&backup1, &backup2);
    }
    if std::path::Path::new(path).exists() {
        let _ = std::fs::rename(path, &backup1);
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn read_json_with_fallback(path: &str) -> std::io::Result<Option<serde_json::Value>> {
    let candidates = [path.to_string(), format!("{path}.bak1"), format!("{path}.bak2")];
    for p in candidates {
        if !std::path::Path::new(&p).exists() {
            continue;
        }
        let data = std::fs::read_to_string(&p)?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            return Ok(Some(v));
        }
    }
    Ok(None)
}
