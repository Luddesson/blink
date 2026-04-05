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
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::activity_log::{ActivityLog, EntryKind, push as log_push};
use crate::latency_tracker::LatencyStats;
use crate::order_book::{OrderBook, OrderBookStore};
use crate::paper_portfolio::{DRIFT_THRESHOLD, PaperPortfolio, STARTING_BALANCE_USDC};
use crate::risk_manager::{RiskConfig, RiskManager};
use crate::types::{OrderSide, RN1Signal, format_price};

// ─── PaperEngine ─────────────────────────────────────────────────────────────

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
    token_cooldowns: Arc<Mutex<HashMap<String, Instant>>>,
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
    pub fn new(book_store: Arc<OrderBookStore>, activity: Option<ActivityLog>) -> Self {
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
            token_cooldowns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn twin_health_handle(&self) -> Arc<Mutex<TwinHealth>> {
        Arc::clone(&self.twin_health)
    }

    pub fn risk_status(&self) -> String {
        self.risk.lock().unwrap().status_line()
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
        let p = self.portfolio.lock().await;
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
        // ── Order-ID dedup: skip if we've already processed this transaction ──
        {
            let mut seen = self.seen_order_ids.lock().await;
            if !seen.insert(signal.order_id.clone()) {
                warn!(order_id = %signal.order_id, "⏭️  Duplicate order_id — skipping");
                return;
            }
        }

        // ── Per-token cooldown: 5s between signals for the same token_id ──
        {
            let mut cooldowns = self.token_cooldowns.lock().await;
            if let Some(last) = cooldowns.get(&signal.token_id) {
                if last.elapsed() < Duration::from_secs(5) {
                    warn!(token_id = %signal.token_id, "⏭️  Token cooldown active — skipping");
                    let mut p = self.portfolio.lock().await;
                    p.skipped_orders += 1;
                    self.record_rejection("token_cooldown").await;
                    return;
                }
            }
            cooldowns.insert(signal.token_id.clone(), Instant::now());
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

        // ── Sizing (brief lock, no await) ─────────────────────────────
        let (size_usdc, current_nav) = {
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            // Keep paper marks alive even when WS mark prices are stale/missing.
            // RN1 signal price becomes a fallback mark update for this token.
            p.update_price(&signal.token_id, entry_price);
            let size = p.calculate_size_usdc(rn1_notional_usd);
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
            let th = self.twin_health.lock().await.clone();
            let throttle = if th.abort_rate > 0.70 && th.close_rate < 0.05 {
                0.4
            } else if th.abort_rate > 0.50 {
                0.7
            } else {
                1.0
            };
            size_usdc *= throttle;
        }

        let min_trade_usdc = std::env::var("PAPER_MIN_TRADE_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(5.0)
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

        // ── Risk check ────────────────────────────────────────────────────
        if let Err(violation) = self.risk.lock().unwrap().check_pre_order(
            size_usdc, {let p = self.portfolio.lock().await; p.positions.len()},
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
        let filled = self
            .check_fill_window(&signal.token_id, entry_price, signal.side)
            .await;

        if !filled {
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

        let slippage_bps = self.estimate_slippage_bps(&signal.token_id, signal.side, entry_price);
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
            )
        };
        // Record fill in risk manager for VaR tracking (does not affect daily P&L).
        self.risk.lock().unwrap().record_fill(size_usdc);

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
            .unwrap_or(5.0)
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

    async fn enrich_signal_metadata(&self, signal: &mut RN1Signal) {
        let title_ok = signal.market_title.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false);
        let outcome_ok = signal.market_outcome.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false);
        if title_ok && outcome_ok {
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

        let tiers = parse_autoclaim_tiers();

        let mut p = self.portfolio.lock().await;
        if p.positions.is_empty() {
            return;
        }

        let token_prices: Vec<(String, f64)> = p.positions
            .iter()
            .filter_map(|pos| self.get_market_price(&pos.token_id).map(|pr| (pos.token_id.clone(), pr)))
            .collect();
        for (token_id, price) in token_prices {
            p.update_price(&token_id, price);
        }

        // If a position's market is no longer live (no book price), close it to
        // avoid stale overnight carry. This preserves the user's rule: keep open
        // positions only while event data remains live.
        let stale_indexes: Vec<usize> = p.positions
            .iter()
            .enumerate()
            .filter_map(|(idx, pos)| {
                if self.get_market_price(&pos.token_id).is_none() { Some(idx) } else { None }
            })
            .collect();
        for idx in stale_indexes.into_iter().rev() {
            let pos = p.positions.remove(idx);
            let pnl = match pos.side {
                OrderSide::Buy => (pos.current_price - pos.entry_price) * pos.shares,
                OrderSide::Sell => (pos.entry_price - pos.current_price) * pos.shares,
            };
            p.cash_usdc += pos.usdc_spent + pnl;
            // Update risk manager with realized P&L from stale close.
            self.risk.lock().unwrap().record_close(pnl);
            p.closed_trades.push(crate::paper_portfolio::ClosedTrade {
                token_id: pos.token_id.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                exit_price: pos.current_price,
                shares: pos.shares,
                realized_pnl: pnl,
                reason: "autoclaim@market_not_live".to_string(),
                opened_at_wall: pos.opened_at_wall,
                closed_at_wall: chrono::Local::now(),
                duration_secs: pos.opened_at.elapsed().as_secs(),
                scorecard: crate::paper_portfolio::ExecutionScorecard {
                    slippage_bps: pos.entry_slippage_bps,
                    queue_delay_ms: pos.queue_delay_ms,
                    outcome_tags: vec![
                        "market_not_live".to_string(),
                        format!("variant:{}", pos.experiment_variant),
                    ],
                },
            });
        }

        let closed_before = p.closed_trades.len();
        let closed = p.autoclaim_tiered(&tiers);
        if closed > 0 {
            // Update risk manager with realized P&L from tiered closes.
            let realized: f64 = p.closed_trades[closed_before..]
                .iter()
                .map(|t| t.realized_pnl)
                .sum();
            self.risk.lock().unwrap().record_close(realized);
            let msg = format!("AUTOCLAIM: {} tiered close action(s)", closed);
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Engine, msg.clone());
            }
            info!("{msg}");
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
