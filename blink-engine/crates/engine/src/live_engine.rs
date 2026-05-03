use anyhow::{bail, Context, Result};
use chrono::Timelike;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::activity_log::{push as log_push, ActivityLog, EntryKind};
use crate::clob_client::ClobClient;
use crate::config::Config;
use crate::exit_strategy::{evaluate_exits, patched_exit_config_for_category, ExitConfig};
use crate::hot_metrics::{HotStage, StageTimer};
use crate::market_metadata::MetadataFetcher;
use crate::mev_router::MevRouter;
use crate::order_book::OrderBookStore;
use crate::order_executor::OrderExecutor;
use crate::order_router::fill_hook::{RouterFillEvent, RouterFillHook};
use crate::order_router::reconciler::spawn_reconciler;
use crate::order_router::{OrderIntent, OrderRouter};
use crate::order_signer::{
    sign_order_for_intent_with_vault_handle_policy, OrderParams, OrderSigningPolicy,
};
#[cfg(feature = "legacy-fill-window")]
use crate::paper_portfolio::drift_threshold;
use crate::paper_portfolio::{PaperPortfolio, PaperPosition, STARTING_BALANCE_USDC};
use crate::postgres_logger::{Rn1SignalRecord, ShadowDecisionRecord, WarehouseEvent};
use crate::pretrade_gate::{GateConfig, GateDecision, PretradeGate};
use crate::quant_strategy::{score_signal, QuantSignalFeatures, QuantSignalScore};
use crate::risk_manager::{RiskConfig, RiskManager};
use crate::strategy::{StrategyController, StrategySnapshot};
use crate::timed_mutex::TimedMutex;
use crate::truth_reconciler::{
    process_order_status, PendingOrder, PendingOrderWal, ReconciliationOutcome,
};
use crate::types::{parse_price, MarketMetadata, OrderSide, PriceLevel, RN1Signal, TimeInForce};

const WALLET_TRUTH_PAGE_LIMIT: usize = 500;

// ─── Lock Hierarchy ──────────────────────────────────────────────────────────
// Two mutex flavours are used.  Always acquire in the order listed below;
// never hold a sync guard across an `.await` point.
//
// ASYNC (tokio::sync::Mutex) — may be held across `.await`:
//   1. accounted_closed_trades  — acquired first in sync_risk_closes_from_portfolio
//   2. portfolio                — acquired after #1 when nested; standalone elsewhere
//   3. pending_orders           — standalone; never nested with #1 or #2
//
// SYNC (TimedMutex) — brief critical sections, dropped before any `.await`:
//   4. risk             — pre-order risk checks and fill accounting
//   5. failsafe_metrics — drift counters and fill/no-fill tallies
//   6. canary_state     — session order cap and reject-streak tracking
//   7. mev_router       — dead code; not in hot path
//
// Invariant: sync guards (#4–#7) are always dropped before the next `.await`.
// ─────────────────────────────────────────────────────────────────────────────

pub struct LiveEngine {
    pub portfolio: Arc<Mutex<PaperPortfolio>>,
    book_store: Arc<OrderBookStore>,
    activity: Option<ActivityLog>,
    pub executor: OrderExecutor,
    vault: Option<Arc<tee_vault::VaultHandle>>,
    funder_addr: String,
    pub risk: Arc<TimedMutex<RiskManager>>,
    /// Shared Phase 3 admission gate (cloned into `order_router`).
    pub risk_gate: Arc<crate::risk_manager::StreamRiskGate>,
    #[allow(dead_code)]
    mev_router: Option<std::sync::Mutex<MevRouter>>,
    accounted_closed_trades: Mutex<usize>,
    signing_policy: OrderSigningPolicy,
    nonce_counter: AtomicU64,
    /// Live orders submitted to the exchange that have not yet been reconciled.
    /// Fill recording is deferred until the reconciliation worker confirms the
    /// actual fill amounts via `GET /order/{id}`.
    pending_orders: Mutex<HashMap<String, PendingOrder>>,
    /// Token IDs with live exit SELL intents already queued.
    pending_exit_intents: Arc<Mutex<HashMap<String, u64>>>,
    reconcile_interval: Duration,
    pub failsafe_metrics: TimedMutex<FailsafeMetrics>,
    canary_policy: CanaryPolicy,
    canary_state: TimedMutex<CanaryState>,
    /// Path to the pending-orders WAL file. Written atomically after every
    /// insert/remove so that crash recovery can reconcile against the exchange.
    wal_path: String,
    strategy_controller: Arc<StrategyController>,
    pub order_router: OrderRouter,
    pretrade_gate: PretradeGate,
    execution_profile: crate::execution_profile::ExecutionProfile,
    metadata_fetcher: MetadataFetcher,
    market_safety_policy: MarketSafetyPolicy,
    shadow_audit: ShadowAuditPolicy,
    warehouse_tx: Option<crossbeam_channel::Sender<WarehouseEvent>>,
    quant_canary_policy: QuantCanaryPolicy,
    live_exit_canary_policy: LiveExitCanaryPolicy,
    live_exit_canary_state: Arc<TimedMutex<LiveExitCanaryState>>,
    starting_nav_usdc: f64,
    rest_clob: ClobClient,
    last_wallet_truth_sync_ms: AtomicU64,
}

/// Point-in-time snapshot of live SLO metrics for dashboards and alerting.
#[derive(Debug, Default, Clone, Copy)]
pub struct FailsafeMetricsSnapshot {
    pub trigger_count: u64,
    pub check_count: u64,
    pub max_observed_drift_bps: u64,
    /// Total exchange-confirmed fills recorded by the reconciliation worker.
    pub confirmed_fills: u64,
    /// Orders that returned no fill (rejected / cancelled / expired).
    pub no_fills: u64,
    /// Orders suspected stale (pending > MAX_PENDING_AGE_SECS).
    pub stale_orders: u64,
    /// Fill confirmation rate: confirmed / (confirmed + no_fills) × 100.
    /// `None` when no orders have been resolved yet.
    pub confirmation_rate_pct: Option<f64>,
    /// Whether the heartbeat was alive at snapshot time.
    pub heartbeat_ok_count: u64,
    pub heartbeat_fail_count: u64,
    pub heartbeat_consecutive_fail_count: u64,
    pub heartbeat_last_ok_ms: u64,
}

#[derive(Debug, Default)]
pub struct FailsafeMetrics {
    pub trigger_count: u64,
    pub check_count: u64,
    pub max_observed_drift_bps: u64,
    pub confirmed_fills: u64,
    pub no_fills: u64,
    pub stale_orders: u64,
    pub heartbeat_ok_count: u64,
    pub heartbeat_fail_count: u64,
    pub heartbeat_consecutive_fail_count: u64,
    pub heartbeat_last_ok_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalIntent {
    NewExposure,
    AddExposure,
    HedgeOrFlatten,
    Ambiguous,
}

#[derive(Debug, Clone)]
struct CanaryPolicy {
    #[allow(dead_code)]
    stage: u8,
    max_order_usdc: f64,
    max_session_spend_usdc: f64,
    max_orders_per_session: usize,
    daytime_only: bool,
    start_hour_utc: u8,
    end_hour_utc: u8,
    max_reject_streak: usize,
    max_loss_streak: usize,
    allowed_markets: Vec<String>,
}

#[derive(Debug, Default)]
struct CanaryState {
    accepted_orders: usize,
    accepted_spend_usdc: f64,
    reject_streak: usize,
    loss_streak: usize,
    halted: bool,
    last_accept_ms: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CanaryStateSnapshot {
    pub stage: u8,
    pub max_order_usdc: f64,
    pub max_session_spend_usdc: f64,
    pub max_orders_per_session: usize,
    pub accepted_orders: usize,
    pub accepted_spend_usdc: f64,
    pub session_spend_remaining_usdc: f64,
    pub reject_streak: usize,
    pub loss_streak: usize,
    pub halted: bool,
    pub last_accept_ms: u64,
}

#[derive(Debug, Clone)]
struct MarketSafetyPolicy {
    allow_neg_risk: bool,
    allow_unknown_metadata: bool,
    allow_live_sell: bool,
    min_supported_tick_size: f64,
}

impl MarketSafetyPolicy {
    fn from_env() -> Self {
        Self {
            allow_neg_risk: env_bool("BLINK_ALLOW_NEG_RISK", false),
            allow_unknown_metadata: env_bool("BLINK_ALLOW_UNKNOWN_MARKET_METADATA", false),
            allow_live_sell: env_bool("BLINK_ALLOW_LIVE_SELL", false),
            min_supported_tick_size: std::env::var("BLINK_MIN_SUPPORTED_TICK_SIZE")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.001),
        }
    }
}

#[derive(Debug, Clone)]
struct ShadowAuditPolicy {
    enabled: bool,
    path: String,
}

impl ShadowAuditPolicy {
    fn from_env(default_enabled: bool) -> Self {
        Self {
            enabled: env_bool("BLINK_SHADOW_AUDIT", default_enabled),
            path: std::env::var("BLINK_SHADOW_AUDIT_PATH")
                .unwrap_or_else(|_| "logs/shadow_live_audit.jsonl".to_string()),
        }
    }
}

#[derive(Debug, Clone)]
struct QuantCanaryPolicy {
    enabled: bool,
    require_complete_features: bool,
    min_score_bps: i64,
    max_toxicity_bps: i64,
}

impl QuantCanaryPolicy {
    fn from_env() -> Self {
        Self {
            enabled: env_bool("LIVE_CANARY_SCORE_GATE", false),
            require_complete_features: env_bool("LIVE_CANARY_SCORE_GATE_REQUIRE_FEATURES", false),
            min_score_bps: std::env::var("LIVE_CANARY_MIN_SCORE_BPS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(4_500)
                .clamp(0, 10_000),
            max_toxicity_bps: std::env::var("LIVE_CANARY_MAX_TOXICITY_BPS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(12_000)
                .clamp(0, 20_000),
        }
    }
}

#[derive(Debug, Clone)]
struct QuantSignalAudit {
    score: QuantSignalScore,
    spread_bps: Option<u64>,
    book_age_ms: Option<u64>,
    depth_usdc: Option<f64>,
}

#[derive(Debug, Clone)]
struct LiveExitRequest {
    token_id: String,
    position_id: usize,
    reason: String,
    shares: f64,
    price_u64: u64,
    notional_usdc: f64,
    pnl_pct: f64,
    current_price: f64,
    top_of_book_confirmed: bool,
}

#[derive(Debug, Clone)]
struct LiveExitCanaryPolicy {
    enabled: bool,
    max_orders_per_session: usize,
    max_order_usdc: f64,
    require_wallet_confirmation: bool,
    require_top_of_book: bool,
}

impl LiveExitCanaryPolicy {
    fn from_env() -> Self {
        Self {
            enabled: env_bool("BLINK_LIVE_EXIT_CANARY_ENABLED", true),
            max_orders_per_session: env_usize("BLINK_LIVE_EXIT_CANARY_MAX_ORDERS_PER_SESSION", 1)
                .clamp(1, 100),
            max_order_usdc: env_f64("BLINK_LIVE_EXIT_CANARY_MAX_ORDER_USDC", 1.0)
                .clamp(0.01, 500_000.0),
            require_wallet_confirmation: env_bool(
                "BLINK_LIVE_EXIT_REQUIRE_WALLET_CONFIRMATION",
                true,
            ),
            require_top_of_book: env_bool("BLINK_LIVE_EXIT_REQUIRE_TOP_OF_BOOK", true),
        }
    }
}

#[derive(Debug, Default)]
struct LiveExitCanaryState {
    queued_orders: usize,
    confirmed_fills: usize,
    last_queued_ms: u64,
    last_confirmed_ms: u64,
}

struct LiveRouterFillHook {
    tx: tokio::sync::mpsc::UnboundedSender<RouterFillEvent>,
}

impl RouterFillHook for LiveRouterFillHook {
    fn on_fill_update(&self, event: RouterFillEvent) {
        if self.tx.send(event).is_err() {
            warn!("Router fill hook dropped event because live consumer is closed");
        }
    }

    fn on_partial_fill(&self, _intent_id: u64, _delta_size_u64: u64, _fill_price_u64: u64) {}

    fn on_full_fill(&self, _intent_id: u64) {}
}

impl LiveEngine {
    const MIN_CLOB_PRICE_U64: u64 = 1;
    const MAX_CLOB_PRICE_U64: u64 = 999;

    pub fn new(
        config: Arc<Config>,
        book_store: Arc<OrderBookStore>,
        activity: Option<ActivityLog>,
        strategy_controller: Arc<StrategyController>,
        execution_profile: crate::execution_profile::ExecutionProfile,
        starting_cash_usdc: Option<f64>,
        warehouse_tx: Option<crossbeam_channel::Sender<WarehouseEvent>>,
    ) -> Result<Self> {
        let executor = OrderExecutor::from_config(&config)?;

        // Initialize the TEE vault for key isolation.
        // P1-7: When LIVE_TRADING=true, vault init failure is FATAL — the
        // engine must not silently degrade to dry-run behavior.
        let vault = if config.live_trading && !config.signer_private_key.is_empty() {
            match tee_vault::VaultHandle::spawn(&config.signer_private_key) {
                Ok(handle) => {
                    info!(
                        address = %handle.signer_address(),
                        "TEE vault initialized — private key isolated in vault task"
                    );
                    Some(Arc::new(handle))
                }
                Err(e) => {
                    if config.live_trading {
                        bail!(
                            "FATAL: LIVE_TRADING=true but TEE vault initialization failed: {e}. \
                             Refusing to start — fix vault configuration or disable live trading."
                        );
                    }
                    error!(error = %e, "Failed to initialize TEE vault — live signing disabled");
                    None
                }
            }
        } else {
            None
        };

        let funder_addr = config.funder_address.clone();
        let mut risk_manager = RiskManager::new(RiskConfig::from_env());
        if should_trip_startup_operator_guard(
            config.live_trading,
            risk_manager.config().trading_enabled,
            std::env::var("WEB_OPERATOR_TOKEN").ok().as_deref(),
        ) {
            let reason = "operator_token_missing_live_startup_guard";
            risk_manager.trip_circuit_breaker(reason);
            warn!(
                reason,
                "Live startup circuit breaker tripped because WEB_OPERATOR_TOKEN is not configured"
            );
            if let Some(ref log) = activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!("LIVE-STARTUP-GUARD: {reason}"),
                );
            }
        }
        let risk_gate = Arc::clone(&risk_manager.gate);
        let risk = Arc::new(TimedMutex::new("risk", risk_manager));
        let signing_policy = OrderSigningPolicy {
            expiration: config.polymarket_order_expiration,
            signature_type: config.polymarket_signature_type,
        };
        let reconcile_interval_secs = std::env::var("LIVE_RECONCILE_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(3, 300))
            .unwrap_or(10);
        let canary_policy = CanaryPolicy {
            stage: config.live_rollout_stage,
            max_order_usdc: config.live_canary_max_order_usdc,
            max_session_spend_usdc: env_f64("LIVE_CANARY_MAX_SESSION_SPEND_USDC", 1.0)
                .clamp(0.01, 500_000.0),
            max_orders_per_session: config.live_canary_max_orders_per_session,
            daytime_only: config.live_canary_daytime_only,
            start_hour_utc: config.live_canary_start_hour_utc,
            end_hour_utc: config.live_canary_end_hour_utc,
            max_reject_streak: config.live_canary_max_reject_streak,
            max_loss_streak: config.live_canary_max_loss_streak,
            allowed_markets: config.live_canary_allowed_markets.clone(),
        };
        let market_safety_policy = MarketSafetyPolicy::from_env();
        let shadow_audit = ShadowAuditPolicy::from_env(config.live_trading);
        let quant_canary_policy = QuantCanaryPolicy::from_env();
        let live_exit_canary_policy = LiveExitCanaryPolicy::from_env();
        let starting_cash_usdc = starting_cash_usdc
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(STARTING_BALANCE_USDC);
        let mut portfolio = PaperPortfolio::new();
        portfolio.cash_usdc = starting_cash_usdc;

        if let Some(ref log) = activity {
            log_push(
                log,
                EntryKind::Engine,
                format!(
                    "LiveEngine started — live={} vault={} dry_run={}",
                    config.live_trading,
                    vault.is_some(),
                    !config.live_trading
                ),
            );
        }

        // Initialize MEV router from env if MEV_ROUTER is set.
        let mev = if std::env::var("MEV_ROUTER").is_ok() {
            info!("MEV router enabled");
            Some(std::sync::Mutex::new(MevRouter::from_env()))
        } else {
            None
        };

        Ok(Self {
            portfolio: Arc::new(Mutex::new(portfolio)),
            book_store: Arc::clone(&book_store),
            activity,
            executor,
            vault,
            funder_addr,
            risk,
            risk_gate,
            mev_router: mev,
            accounted_closed_trades: Mutex::new(0),
            signing_policy,
            nonce_counter: AtomicU64::new(config.polymarket_order_nonce),
            pending_orders: Mutex::new(HashMap::new()),
            pending_exit_intents: Arc::new(Mutex::new(HashMap::new())),
            reconcile_interval: Duration::from_secs(reconcile_interval_secs),
            failsafe_metrics: TimedMutex::new("failsafe_metrics", FailsafeMetrics::default()),
            canary_policy,
            canary_state: TimedMutex::new("canary_state", CanaryState::default()),
            wal_path: std::env::var("PENDING_ORDERS_WAL_PATH")
                .unwrap_or_else(|_| "logs/live_pending_orders_wal.json".to_string()),
            strategy_controller,
            order_router: OrderRouter::new(),
            pretrade_gate: PretradeGate::new(book_store),
            execution_profile,
            metadata_fetcher: MetadataFetcher::new(),
            market_safety_policy,
            shadow_audit,
            warehouse_tx,
            quant_canary_policy,
            live_exit_canary_policy,
            live_exit_canary_state: Arc::new(TimedMutex::new(
                "live_exit_canary_state",
                LiveExitCanaryState::default(),
            )),
            starting_nav_usdc: starting_cash_usdc,
            rest_clob: ClobClient::new(&config.clob_host),
            last_wallet_truth_sync_ms: AtomicU64::new(0),
        })
    }

    /// Returns the execution profile this engine was constructed with.
    pub fn execution_profile(&self) -> crate::execution_profile::ExecutionProfile {
        self.execution_profile
    }

    pub fn strategy_snapshot(&self) -> StrategySnapshot {
        self.strategy_controller.snapshot()
    }

    /// Spawn the router's submit-worker pool and reconciler.
    /// Must be called once after `Arc<LiveEngine>` is constructed.
    pub async fn spawn_router_workers(&self) {
        let (fill_tx, fill_rx) = tokio::sync::mpsc::unbounded_channel();
        self.spawn_router_fill_consumer(fill_rx);
        let fill_hook = Arc::new(LiveRouterFillHook { tx: fill_tx });
        let exec = Arc::new(self.executor.clone());
        self.order_router.set_risk_gate(Arc::clone(&self.risk_gate));
        crate::risk_manager::StreamRiskGate::spawn_token_refill(Arc::clone(&self.risk_gate));
        self.order_router.spawn_workers(Arc::clone(&exec)).await;
        spawn_reconciler(
            Arc::clone(&self.order_router.store),
            Arc::clone(&self.order_router.counters),
            Arc::clone(&exec),
            Some(fill_hook),
            Some(Arc::clone(&self.risk_gate)),
        );

        #[cfg(feature = "maker-layering")]
        crate::live_engine::spawn_maker_layering_task(
            Arc::clone(&self.risk_gate),
            Arc::clone(&exec),
            self.order_router.inbound_sender(),
        );
    }

    fn spawn_router_fill_consumer(
        &self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<RouterFillEvent>,
    ) {
        let portfolio = Arc::clone(&self.portfolio);
        let risk = Arc::clone(&self.risk);
        let activity = self.activity.clone();
        let pending_exit_intents = Arc::clone(&self.pending_exit_intents);
        let live_exit_canary_state = Arc::clone(&self.live_exit_canary_state);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if event.delta_size_u64 == 0 {
                    continue;
                }

                let fill_usdc = event.delta_size_u64 as f64 / 1_000.0;
                let fill_price = event.fill_price_u64 as f64 / 1_000.0;
                match event.side {
                    OrderSide::Buy => {
                        {
                            let mut p = portfolio.lock().await;
                            p.open_position(
                                event.token_id.clone(),
                                event.side,
                                fill_price,
                                fill_usdc,
                                format!("router:{}", event.intent_id),
                            );
                        }
                        risk.lock_or_recover().record_fill(fill_usdc);
                        if let Some(ref log) = activity {
                            log_push(
                                log,
                                EntryKind::Fill,
                                format!(
                                    "ROUTER FILL {} @{:.3} ${:.2} intent={}",
                                    event.side, fill_price, fill_usdc, event.intent_id
                                ),
                            );
                        }
                    }
                    OrderSide::Sell => {
                        let close_shares = if fill_price > 0.0 {
                            fill_usdc / fill_price
                        } else {
                            0.0
                        };
                        let (closed_actions, realized_pnl) = {
                            let mut p = portfolio.lock().await;
                            let before = p.closed_trades.len();
                            let actions = p.close_token_shares_at_price(
                                &event.token_id,
                                close_shares,
                                fill_price,
                                format!("router_exit:{}", event.intent_id),
                            );
                            let realized = p.closed_trades[before..]
                                .iter()
                                .map(|trade| trade.realized_pnl)
                                .sum();
                            (actions, realized)
                        };
                        if closed_actions > 0 {
                            risk.lock_or_recover().record_close(realized_pnl);
                        }
                        if event.is_full {
                            pending_exit_intents.lock().await.remove(&event.token_id);
                        }
                        {
                            let mut canary = live_exit_canary_state.lock_or_recover();
                            canary.confirmed_fills = canary.confirmed_fills.saturating_add(1);
                            canary.last_confirmed_ms = current_time_ms();
                        }
                        if closed_actions == 0 {
                            warn!(
                                intent_id = event.intent_id,
                                token_id = %event.token_id,
                                fill_usdc,
                                fill_price,
                                close_shares,
                                "Router SELL fill had no matching local BUY position to close"
                            );
                        }
                        if let Some(ref log) = activity {
                            log_push(
                                log,
                                EntryKind::Fill,
                                format!(
                                    "ROUTER EXIT FILL {} @{:.3} ${:.2} shares={:.4} intent={}",
                                    event.token_id,
                                    fill_price,
                                    fill_usdc,
                                    close_shares,
                                    event.intent_id
                                ),
                            );
                        }
                    }
                }
            }
        });
    }

    /// Atomically persist the current `pending_orders` map to the WAL file.
    ///
    /// Writes to `<wal_path>.tmp` then renames to avoid partial reads on crash.
    /// Called after every insert and remove from `pending_orders`.
    async fn persist_wal(&self) {
        let entries: Vec<PendingOrderWal> = self
            .pending_orders
            .lock()
            .await
            .values()
            .map(PendingOrderWal::from)
            .collect();

        let json = match serde_json::to_string_pretty(&entries) {
            Ok(j) => j,
            Err(e) => {
                error!("WAL serialize failed: {e}");
                return;
            }
        };

        let tmp = format!("{}.tmp", self.wal_path);
        if let Err(e) = std::fs::write(&tmp, &json) {
            error!("WAL write to {tmp} failed: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &self.wal_path) {
            error!("WAL rename {tmp} → {} failed: {e}", self.wal_path);
        }
    }

    /// Load pending orders from WAL on startup and run immediate reconciliation
    /// against the exchange. Returns the number of orders recovered.
    ///
    /// Must be called before `spawn_reconciliation_worker` and before accepting
    /// any new signals. In dry-run mode this is a no-op.
    pub async fn startup_reconcile(&self) -> usize {
        if self.executor.dry_run {
            return 0;
        }

        let wal_json = match std::fs::read_to_string(&self.wal_path) {
            Ok(s) if !s.trim().is_empty() => s,
            Ok(_) => return 0, // empty file
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
            Err(e) => {
                error!("Failed to read WAL {}: {e}", self.wal_path);
                return 0;
            }
        };

        let wal_entries: Vec<PendingOrderWal> = match serde_json::from_str(&wal_json) {
            Ok(v) => v,
            Err(e) => {
                error!("WAL parse error (will ignore and start fresh): {e}");
                return 0;
            }
        };

        if wal_entries.is_empty() {
            return 0;
        }

        warn!(
            count = wal_entries.len(),
            "🔄 WAL recovery: found pending orders from previous session — reconciling with exchange"
        );
        if let Some(ref log) = self.activity {
            log_push(
                log,
                EntryKind::Warn,
                format!(
                    "WAL RECOVERY: {} pending orders from previous session — reconciling",
                    wal_entries.len()
                ),
            );
        }

        // Load WAL entries into pending_orders, then run a full reconciliation pass.
        {
            let mut map = self.pending_orders.lock().await;
            for entry in wal_entries {
                let order_id = entry.exchange_order_id.clone();
                map.insert(order_id, PendingOrder::from(entry));
            }
        }

        let recovered = self.pending_orders.lock().await.len();

        // Run reconciliation immediately — don't wait for the periodic worker.
        self.run_reconciliation_pass().await;

        // Persist updated state (some orders may have been resolved above).
        self.persist_wal().await;

        let remaining = self.pending_orders.lock().await.len();
        let pending_exit_intents = self
            .rebuild_pending_exit_intents_from_pending_orders()
            .await;
        info!(
            recovered,
            remaining, pending_exit_intents, "WAL startup reconciliation complete"
        );
        recovered
    }

    async fn rebuild_pending_exit_intents_from_pending_orders(&self) -> usize {
        let tokens = {
            let pending_orders = self.pending_orders.lock().await;
            pending_exit_tokens_from_pending_orders(pending_orders.values())
        };
        let count = tokens.len();
        let now_ms = current_time_ms();
        let mut pending_exits = self.pending_exit_intents.lock().await;
        pending_exits.clear();
        pending_exits.extend(tokens.into_iter().map(|token_id| (token_id, now_ms)));
        count
    }

    /// Clears any exchange-side open orders before the live engine accepts
    /// fresh signals. This prevents old maker orders from surviving a restart.
    pub async fn startup_cancel_all_open_orders(&self) -> Result<()> {
        if self.executor.dry_run || !env_bool("LIVE_STARTUP_CANCEL_ALL", true) {
            return Ok(());
        }

        warn!("LIVE_STARTUP_CANCEL_ALL=true — cancelling all open exchange orders before signal intake");
        if let Some(ref log) = self.activity {
            log_push(
                log,
                EntryKind::Warn,
                "STARTUP SCRUB: cancelling all open exchange orders".to_string(),
            );
        }

        self.executor
            .cancel_all_orders()
            .await
            .context("startup cancel_all_orders failed")?;
        self.run_reconciliation_pass().await;
        self.persist_wal().await;
        Ok(())
    }

    pub fn spawn_reconciliation_worker(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                sleep(self.reconcile_interval).await;
                self.run_reconciliation_pass().await;
            }
        });
    }

    pub fn spawn_wallet_truth_worker(self: Arc<Self>) {
        let poll_interval = std::env::var("BLINK_WALLET_TRUTH_POLL_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(1_000, 60_000))
            .unwrap_or(1_000);
        tokio::spawn(async move {
            loop {
                if let Err(e) = self.sync_wallet_positions_from_exchange().await {
                    warn!(error = %e, "Live wallet truth background sync failed");
                }
                sleep(Duration::from_millis(poll_interval)).await;
            }
        });
    }

    pub fn spawn_exit_strategy_auditor(self: Arc<Self>) {
        if !env_bool("BLINK_LIVE_EXIT_AUDIT_ENABLED", true) {
            return;
        }

        let scan_interval_ms = std::env::var("BLINK_LIVE_EXIT_SCAN_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(100, 5_000))
            .unwrap_or(500);
        let repeat_log_ms = std::env::var("BLINK_LIVE_EXIT_AUDIT_REPEAT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(1_000, 60_000))
            .unwrap_or(5_000);

        tokio::spawn(async move {
            let mut last_logged_ms: HashMap<String, u64> = HashMap::new();
            loop {
                sleep(Duration::from_millis(scan_interval_ms)).await;
                self.run_exit_strategy_audit_once(&mut last_logged_ms, repeat_log_ms)
                    .await;
            }
        });

        info!(
            scan_interval_ms,
            repeat_log_ms, "Live exit strategy auditor started"
        );
    }

    async fn run_exit_strategy_audit_once(
        &self,
        last_logged_ms: &mut HashMap<String, u64>,
        repeat_log_ms: u64,
    ) {
        let exit_config = ExitConfig::from_env();
        let mut p = self.portfolio.lock().await;
        if p.positions.is_empty() {
            return;
        }

        let token_prices: Vec<(String, f64)> = p
            .positions
            .iter()
            .filter_map(|pos| {
                self.live_mark_price(&pos.token_id)
                    .map(|price| (pos.token_id.clone(), price))
            })
            .collect();
        for (token_id, price) in token_prices {
            p.update_price(&token_id, price);
        }

        let live_tokens: HashSet<String> = p
            .positions
            .iter()
            .filter(|pos| self.live_mark_price(&pos.token_id).is_some())
            .map(|pos| pos.token_id.clone())
            .collect();

        let decisions: Vec<crate::exit_strategy::ExitDecision> = p
            .positions
            .iter()
            .enumerate()
            .flat_map(|(real_idx, pos)| {
                let patched =
                    patched_exit_config_for_category(&exit_config, pos.market_title.as_deref());
                evaluate_exits(
                    std::slice::from_ref(pos),
                    &patched,
                    |tid| live_tokens.contains(tid),
                    |tid| self.book_store.get_spread_bps(tid),
                )
                .into_iter()
                .map(move |mut decision| {
                    decision.position_idx = real_idx;
                    decision
                })
            })
            .collect();

        if decisions.is_empty() {
            return;
        }

        let max_exit_notional_usdc = self.live_exit_effective_max_order_usdc();
        let records: Vec<LiveExitRequest> = decisions
            .into_iter()
            .filter_map(|decision| {
                p.positions.get(decision.position_idx).and_then(|pos| {
                    let top_of_book = self.book_store.top_of_book(&pos.token_id, OrderSide::Sell);
                    let (price_u64, top_of_book_confirmed, top_level_shares) = match top_of_book {
                        Some((price, size)) if Self::is_valid_clob_price_u64(price) => {
                            (price, true, size as f64 / 1_000.0)
                        }
                        _ => (
                            self.book_store.get_mark_price(&pos.token_id)?,
                            false,
                            f64::INFINITY,
                        ),
                    };
                    if !Self::is_valid_clob_price_u64(price_u64) {
                        return None;
                    }
                    let exit_price = price_u64 as f64 / 1_000.0;
                    let mut shares = pos.shares * decision.action.fraction();
                    shares = shares.min(top_level_shares);
                    if max_exit_notional_usdc > 0.0 {
                        shares = shares.min(max_exit_notional_usdc / exit_price.max(0.001));
                    }
                    let notional_usdc = shares * exit_price;
                    if shares <= 0.0 || notional_usdc <= 0.0 {
                        return None;
                    }
                    Some(LiveExitRequest {
                        token_id: pos.token_id.clone(),
                        position_id: pos.id,
                        reason: decision.action.reason(),
                        shares,
                        price_u64,
                        notional_usdc,
                        pnl_pct: pos.unrealized_pnl_pct(),
                        current_price: pos.current_price,
                        top_of_book_confirmed,
                    })
                })
            })
            .collect();
        drop(p);

        let now_ms = current_time_ms();
        let exit_execution_enabled = self.market_safety_policy.allow_live_sell
            && env_bool("BLINK_LIVE_EXIT_EXECUTION_ENABLED", false);
        self.expire_pending_exit_intents(now_ms).await;

        for request in records {
            let key = format!("{}:{}", request.token_id, request.reason);
            if let Some(last) = last_logged_ms.get(&key) {
                if now_ms.saturating_sub(*last) < repeat_log_ms {
                    continue;
                }
            }
            last_logged_ms.insert(key, now_ms);

            let static_block_reason = self.live_exit_static_block_reason(&request);
            self.audit_live_exit_shadow_decision(
                &request,
                exit_execution_enabled,
                static_block_reason.as_deref(),
                now_ms,
            );
            if !exit_execution_enabled {
                let alert_key = format!("live_exit_trigger:{}", request.reason);
                crate::operator_alerts::emit_operator_alert(
                    "live_exit_trigger",
                    "warn",
                    &alert_key,
                    "Live exit trigger observed without SELL execution",
                    serde_json::json!({
                        "token_id": &request.token_id,
                        "position_id": request.position_id,
                        "reason": &request.reason,
                        "pnl_pct": request.pnl_pct,
                        "current_price": request.current_price,
                        "notional_usdc": request.notional_usdc,
                        "allow_live_sell": self.market_safety_policy.allow_live_sell,
                        "exit_execution_enabled": exit_execution_enabled,
                        "static_block_reason": static_block_reason,
                    }),
                );
            }

            if exit_execution_enabled {
                match self.submit_live_exit_order(request.clone()).await {
                    Ok(true) => {
                        info!(
                            token_id = %request.token_id,
                            position_id = request.position_id,
                            reason = %request.reason,
                            shares = request.shares,
                            notional_usdc = request.notional_usdc,
                            price_u64 = request.price_u64,
                            "Live exit SELL queued"
                        );
                    }
                    Ok(false) => {}
                    Err(e) => {
                        warn!(
                            token_id = %request.token_id,
                            position_id = request.position_id,
                            reason = %request.reason,
                            error = %e,
                            "Live exit SELL queue failed"
                        );
                    }
                }
            } else if self.market_safety_policy.allow_live_sell {
                warn!(
                    token_id = %request.token_id,
                    position_id = request.position_id,
                    reason = %request.reason,
                    pnl_pct = request.pnl_pct,
                    current_price = request.current_price,
                    "Live exit trigger observed; BLINK_LIVE_EXIT_EXECUTION_ENABLED=false"
                );
            } else {
                warn!(
                    token_id = %request.token_id,
                    position_id = request.position_id,
                    reason = %request.reason,
                    pnl_pct = request.pnl_pct,
                    current_price = request.current_price,
                    "Live exit audit trigger"
                );
            }
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!(
                        "LIVE EXIT AUDIT pos={} {} reason={} pnl={:.2}% mark={:.3}",
                        request.position_id,
                        request.token_id,
                        request.reason,
                        request.pnl_pct,
                        request.current_price
                    ),
                );
            }
        }
    }

    async fn expire_pending_exit_intents(&self, now_ms: u64) {
        let ttl_ms = std::env::var("BLINK_LIVE_EXIT_PENDING_TTL_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(5_000, 600_000))
            .unwrap_or(120_000);
        self.pending_exit_intents
            .lock()
            .await
            .retain(|_, ts| now_ms.saturating_sub(*ts) <= ttl_ms);
    }

    async fn submit_live_exit_order(&self, request: LiveExitRequest) -> Result<bool> {
        if request.shares <= 0.0 || request.notional_usdc <= 0.0 {
            return Ok(false);
        }

        let now_ms = current_time_ms();
        {
            let mut pending = self.pending_exit_intents.lock().await;
            if pending.contains_key(&request.token_id) {
                return Ok(false);
            }
            pending.insert(request.token_id.clone(), now_ms);
        }

        let pre_submit_check = self.live_exit_pre_submit_block_reason(&request).await;
        let block_reason = match pre_submit_check {
            Ok(reason) => reason,
            Err(e) => {
                self.pending_exit_intents
                    .lock()
                    .await
                    .remove(&request.token_id);
                return Err(e);
            }
        };
        if let Some(reason) = block_reason {
            warn!(
                token_id = %request.token_id,
                position_id = request.position_id,
                reason = %request.reason,
                block_reason = %reason,
                shares = request.shares,
                notional_usdc = request.notional_usdc,
                price_u64 = request.price_u64,
                "Live exit SELL blocked by canary guard"
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!(
                        "LIVE EXIT BLOCKED pos={} {} reason={} block={} ${:.2}",
                        request.position_id,
                        request.token_id,
                        request.reason,
                        reason,
                        request.notional_usdc
                    ),
                );
            }
            self.pending_exit_intents
                .lock()
                .await
                .remove(&request.token_id);
            return Ok(false);
        }

        let submit_result = self.submit_live_exit_order_inner(&request).await;
        if submit_result.is_err() {
            self.pending_exit_intents
                .lock()
                .await
                .remove(&request.token_id);
        } else if matches!(submit_result, Ok(true)) {
            let mut canary = self.live_exit_canary_state.lock_or_recover();
            canary.queued_orders = canary.queued_orders.saturating_add(1);
            canary.last_queued_ms = now_ms;
        }
        submit_result
    }

    async fn live_exit_pre_submit_block_reason(
        &self,
        request: &LiveExitRequest,
    ) -> Result<Option<String>> {
        let min_order_usdc = live_exit_min_order_usdc();
        if let Some(reason) = self.live_exit_static_block_reason_with_min(request, min_order_usdc) {
            return Ok(Some(reason));
        }

        if self.live_exit_canary_policy.require_wallet_confirmation {
            let wallet_confirmed = self.wallet_confirms_live_exit(request).await?;
            if !wallet_confirmed {
                return Ok(Some("wallet_position_not_confirmed".to_string()));
            }
        }

        Ok(None)
    }

    fn live_exit_static_block_reason(&self, request: &LiveExitRequest) -> Option<String> {
        self.live_exit_static_block_reason_with_min(request, live_exit_min_order_usdc())
    }

    fn live_exit_static_block_reason_with_min(
        &self,
        request: &LiveExitRequest,
        min_order_usdc: f64,
    ) -> Option<String> {
        let canary = self.live_exit_canary_state.lock_or_recover();
        live_exit_canary_static_block_reason(
            &self.live_exit_canary_policy,
            &canary,
            request,
            min_order_usdc,
        )
    }

    fn audit_live_exit_shadow_decision(
        &self,
        request: &LiveExitRequest,
        exit_execution_enabled: bool,
        static_block_reason: Option<&str>,
        now_ms: u64,
    ) {
        if !env_bool("BLINK_LIVE_EXIT_SHADOW_AUDIT_ENABLED", true) {
            return;
        }

        let path = std::env::var("BLINK_LIVE_EXIT_SHADOW_AUDIT_PATH")
            .unwrap_or_else(|_| "logs/live_exit_shadow_audit.jsonl".to_string());
        let path_ref = Path::new(&path);
        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    warn!(path = %path, error = %e, "live exit shadow audit mkdir failed");
                    return;
                }
            }
        }

        let (canary_queued_orders, canary_confirmed_fills, canary_last_queued_ms) = {
            let canary = self.live_exit_canary_state.lock_or_recover();
            (
                canary.queued_orders,
                canary.confirmed_fills,
                canary.last_queued_ms,
            )
        };
        let runtime_block_reason = if !self.market_safety_policy.allow_live_sell {
            Some("live_sell_disabled")
        } else if !env_bool("BLINK_LIVE_EXIT_EXECUTION_ENABLED", false) {
            Some("live_exit_execution_disabled")
        } else {
            static_block_reason
        };
        let would_attempt_submit = exit_execution_enabled && static_block_reason.is_none();

        let event = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "timestamp_ms": now_ms,
            "kind": "live_exit_shadow",
            "token_id": &request.token_id,
            "position_id": request.position_id,
            "exit_reason": &request.reason,
            "shares": request.shares,
            "price_u64": request.price_u64,
            "price": request.price_u64 as f64 / 1_000.0,
            "notional_usdc": request.notional_usdc,
            "pnl_pct": request.pnl_pct,
            "current_price": request.current_price,
            "top_of_book_confirmed": request.top_of_book_confirmed,
            "would_attempt_submit": would_attempt_submit,
            "runtime_block_reason": runtime_block_reason,
            "static_block_reason": static_block_reason,
            "allow_live_sell": self.market_safety_policy.allow_live_sell,
            "exit_execution_enabled": exit_execution_enabled,
            "effective_max_order_usdc": self.live_exit_effective_max_order_usdc(),
            "min_order_usdc": live_exit_min_order_usdc(),
            "canary_enabled": self.live_exit_canary_policy.enabled,
            "canary_max_orders_per_session": self.live_exit_canary_policy.max_orders_per_session,
            "canary_max_order_usdc": self.live_exit_canary_policy.max_order_usdc,
            "canary_require_wallet_confirmation": self.live_exit_canary_policy.require_wallet_confirmation,
            "canary_require_top_of_book": self.live_exit_canary_policy.require_top_of_book,
            "canary_queued_orders": canary_queued_orders,
            "canary_confirmed_fills": canary_confirmed_fills,
            "canary_last_queued_ms": canary_last_queued_ms,
        });

        match OpenOptions::new().create(true).append(true).open(path_ref) {
            Ok(mut file) => {
                if let Ok(line) = serde_json::to_string(&event) {
                    if let Err(e) = writeln!(file, "{line}") {
                        warn!(path = %path, error = %e, "live exit shadow audit write failed");
                    }
                }
            }
            Err(e) => warn!(path = %path, error = %e, "live exit shadow audit open failed"),
        }
    }

    async fn wallet_confirms_live_exit(&self, request: &LiveExitRequest) -> Result<bool> {
        let wallet_positions = fetch_wallet_positions_from_data_api(&self.funder_addr).await?;
        let wallet_shares = wallet_positions
            .iter()
            .filter(|pos| pos.token_id == request.token_id)
            .map(|pos| pos.shares)
            .sum::<f64>();
        Ok(wallet_shares + 0.000_001 >= request.shares)
    }

    async fn submit_live_exit_order_inner(&self, request: &LiveExitRequest) -> Result<bool> {
        let intent_id = self.next_order_timestamp_ms();
        let params = OrderParams {
            token_id: request.token_id.clone(),
            side: OrderSide::Sell,
            price: request.price_u64,
            size: request.shares,
            maker: self.funder_addr.clone(),
            builder: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            timestamp: intent_id,
            metadata: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        };

        if self.executor.dry_run || self.vault.is_none() {
            info!(
                intent_id,
                token_id = %request.token_id,
                shares = request.shares,
                notional_usdc = request.notional_usdc,
                "DRY-RUN: would queue live exit SELL"
            );
            return Ok(true);
        }

        let vault = self
            .vault
            .as_ref()
            .expect("vault is Some; guarded by is_none() check above");
        let signed = sign_order_for_intent_with_vault_handle_policy(
            vault.as_ref(),
            &params,
            self.signing_policy,
            intent_id,
        )
        .await
        .context("sign live exit SELL")?;

        let order_intent = OrderIntent {
            intent_id,
            market_id: request.token_id.clone(),
            token_id: request.token_id.clone(),
            side: OrderSide::Sell,
            price_u64: request.price_u64,
            size_u64: (request.notional_usdc * 1_000.0).round() as u64,
            tif: TimeInForce::Fak,
            strategy_mode: self.strategy_controller.snapshot().current_mode,
            requested_at: Instant::now(),
            signed_payload: Some(signed),
        };

        self.order_router
            .submit(order_intent)
            .await
            .map_err(|e| anyhow::anyhow!("queue live exit SELL: {e}"))?;
        Ok(true)
    }

    fn live_exit_effective_max_order_usdc(&self) -> f64 {
        let base = live_exit_max_order_usdc();
        if self.live_exit_canary_policy.enabled {
            base.min(self.live_exit_canary_policy.max_order_usdc)
        } else {
            base
        }
    }

    #[inline]
    fn live_mark_price(&self, token_id: &str) -> Option<f64> {
        self.book_store
            .get_mark_price(token_id)
            .map(|p| p as f64 / 1_000.0)
    }

    async fn ensure_wallet_truth_for_hot_path(&self) -> Result<()> {
        if self.executor.dry_run && !env_bool("BLINK_REQUIRE_WALLET_TRUTH_IN_DRY_RUN", false) {
            return Ok(());
        }

        let max_age_ms = std::env::var("BLINK_HOT_PATH_WALLET_TRUTH_MAX_AGE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(1_000, 60_000))
            .unwrap_or(10_000);
        let last_sync = self.last_wallet_truth_sync_ms.load(Ordering::Relaxed);
        if last_sync > 0 && current_time_ms().saturating_sub(last_sync) <= max_age_ms {
            return Ok(());
        }

        self.sync_wallet_positions_from_exchange().await
    }

    /// Validates L2 HMAC credentials against the Polymarket exchange before
    /// any live order is attempted.
    ///
    /// **Must be called and awaited before starting live trading.**
    /// Returns `Err` with a clear message if credentials are rejected, so the
    /// operator can fix the issue before capital is at risk.
    pub async fn preflight_check(&self) -> anyhow::Result<()> {
        self.executor.validate_credentials().await
    }

    pub fn risk_status(&self) -> String {
        self.risk.lock_or_recover().status_line()
    }

    pub fn wallet_truth_last_sync_ms(&self) -> Option<u64> {
        let last_sync = self.last_wallet_truth_sync_ms.load(Ordering::Relaxed);
        (last_sync > 0).then_some(last_sync)
    }

    pub fn wallet_truth_sync_age_ms(&self) -> Option<u64> {
        self.wallet_truth_last_sync_ms()
            .map(|last_sync| current_time_ms().saturating_sub(last_sync))
    }

    async fn sync_wallet_positions_from_exchange(&self) -> Result<()> {
        if self.executor.dry_run && !env_bool("BLINK_REQUIRE_WALLET_TRUTH_IN_DRY_RUN", false) {
            return Ok(());
        }
        if self.funder_addr.trim().is_empty() {
            bail!("missing funder wallet");
        }

        let now_ms = current_time_ms();
        let sync_ttl_ms = std::env::var("BLINK_WALLET_TRUTH_SYNC_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(500, 60_000))
            .unwrap_or(3_000);
        let last_sync = self.last_wallet_truth_sync_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last_sync) < sync_ttl_ms {
            return Ok(());
        }

        let hydrate_open_times_from_trades = last_sync == 0;
        let mut wallet_positions = fetch_wallet_positions_from_data_api(&self.funder_addr).await?;
        if wallet_positions.is_empty() {
            let local_count = self.portfolio.lock().await.positions.len();
            if local_count > 0 {
                warn!(
                    local_positions = local_count,
                    "Wallet truth returned empty while local ledger had positions; confirming once"
                );
                sleep(Duration::from_millis(250)).await;
                wallet_positions = fetch_wallet_positions_from_data_api(&self.funder_addr).await?;
            }
        }
        if hydrate_open_times_from_trades && !wallet_positions.is_empty() {
            if let Err(e) = hydrate_wallet_position_open_times_from_trades(
                &self.funder_addr,
                &mut wallet_positions,
            )
            .await
            {
                warn!(
                    error = %e,
                    "Wallet truth trade-history open-time hydration failed; using snapshot time"
                );
            }
        }
        let wallet_ids: HashSet<String> = wallet_positions
            .iter()
            .map(|pos| pos.token_id.clone())
            .collect();

        let mut p = self.portfolio.lock().await;
        let local_ids: HashSet<String> =
            p.positions.iter().map(|pos| pos.token_id.clone()).collect();
        if local_ids != wallet_ids {
            warn!(
                local_positions = local_ids.len(),
                wallet_positions = wallet_ids.len(),
                "Live wallet truth sync replaced local position ledger"
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!(
                        "TRUTH-SYNC: local_positions={} wallet_positions={}",
                        local_ids.len(),
                        wallet_ids.len()
                    ),
                );
            }
        }
        preserve_wallet_position_lifecycle(&mut wallet_positions, &p.positions);
        p.positions = wallet_positions;
        self.last_wallet_truth_sync_ms
            .store(now_ms, Ordering::Relaxed);
        Ok(())
    }

    /// Flush all open positions from the internal order cache.
    ///
    /// Called when a game-start signal is detected — the CLOB clears all
    /// outstanding orders, so our local state must be flushed too.
    pub async fn flush_order_cache(&self, token_id: &str) {
        let mut p = self.portfolio.lock().await;
        let before = p.positions.len();
        p.positions.retain(|pos| pos.token_id != token_id);
        let after = p.positions.len();
        let flushed = before - after;

        if flushed > 0 {
            warn!(
                token_id,
                flushed, "🧹 Pre-game order wipe: flushed {flushed} positions for {token_id}"
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!("GAME START: flushed {flushed} positions for {token_id}"),
                );
            }
        }
    }

    pub async fn handle_signal(&self, signal: RN1Signal) {
        let handle_started_at = Instant::now();
        self.record_rn1_signal(&signal);
        // ── Stage: Enrich (intent classification + price extraction) ────────
        let _enrich_timer = StageTimer::start(HotStage::Enrich);
        if let Err(e) = self.ensure_wallet_truth_for_hot_path().await {
            let reason = format!("wallet_truth_unverified: {e}");
            warn!(token_id = %signal.token_id, side = %signal.side, reason, "Blocking live signal");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("TRUTH-BLOCKED: {reason}"));
            }
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            p.skipped_orders += 1;
            self.audit_signal_decision(
                &signal,
                "blocked_wallet_truth_unverified",
                Some(&reason),
                None,
                None,
                handle_started_at,
            );
            return;
        }
        self.sync_risk_closes_from_portfolio().await;
        // reconciliation is now owned by the OrderRouter reconciler task (250 ms sweeps)

        let intent = self.classify_signal_intent(&signal).await;
        if matches!(
            intent,
            SignalIntent::HedgeOrFlatten | SignalIntent::Ambiguous
        ) {
            let reason = match intent {
                SignalIntent::HedgeOrFlatten => "hedge_or_flatten",
                SignalIntent::Ambiguous => "ambiguous_multi_side_exposure",
                _ => "unknown",
            };
            warn!(
                token_id = %signal.token_id,
                side = %signal.side,
                reason,
                "Skipping RN1 signal due to intent classification"
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!(
                        "INTENT-SKIP {} token={} side={}",
                        reason, signal.token_id, signal.side
                    ),
                );
            }
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            p.skipped_orders += 1;
            self.audit_signal_decision(
                &signal,
                "blocked_intent",
                Some(reason),
                None,
                None,
                handle_started_at,
            );
            return;
        }

        let metadata = match self.check_market_metadata_gate(&signal).await {
            Ok(metadata) => metadata,
            Err(reason) => {
                warn!(
                    token_id = %signal.token_id,
                    side = %signal.side,
                    reason,
                    "Market metadata gate blocked order"
                );
                if let Some(ref log) = self.activity {
                    log_push(log, EntryKind::Warn, format!("METADATA-BLOCKED: {reason}"));
                }
                let mut p = self.portfolio.lock().await;
                p.total_signals += 1;
                p.skipped_orders += 1;
                self.audit_signal_decision(
                    &signal,
                    "blocked_market_metadata",
                    Some(&reason),
                    None,
                    None,
                    handle_started_at,
                );
                return;
            }
        };
        if let Err(reason) = self.check_live_side_gate(&signal) {
            warn!(token_id = %signal.token_id, side = %signal.side, reason, "Live side gate blocked order");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("SIDE-BLOCKED: {reason}"));
            }
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            p.skipped_orders += 1;
            self.audit_signal_decision(
                &signal,
                "blocked_side",
                Some(&reason),
                None,
                Some(&metadata),
                handle_started_at,
            );
            return;
        }

        drop(_enrich_timer);

        // ── Stage: Sizing ─────────────────────────────────────────────────
        let _sizing_timer = StageTimer::start(HotStage::Sizing);

        // 1. Calculate entry_price, rn1_shares, rn1_notional_usd
        let entry_price = signal.price as f64 / 1_000.0;
        let rn1_shares = signal.size as f64 / 1_000.0;
        let rn1_notional_usd = rn1_shares * entry_price;
        let strategy_snapshot = self.strategy_controller.snapshot();
        let strategy_profile = strategy_snapshot.profile;
        let min_notional_base = std::env::var("MIN_SIGNAL_NOTIONAL_USD")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(10.0);
        let min_notional = (min_notional_base * strategy_profile.min_notional_multiplier).max(0.0);
        if rn1_notional_usd < min_notional {
            warn!(
                token_id = %signal.token_id,
                rn1_notional_usd = %format!("${:.2}", rn1_notional_usd),
                min_notional = %format!("${:.2}", min_notional),
                strategy_mode = %strategy_snapshot.current_mode,
                "Skipping live signal: strategy min-notional gate"
            );
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            p.skipped_orders += 1;
            self.audit_signal_decision(
                &signal,
                "blocked_min_notional",
                Some("rn1_notional_below_strategy_min"),
                None,
                Some(&metadata),
                handle_started_at,
            );
            return;
        }
        let strategy_adjusted_notional = rn1_notional_usd * strategy_profile.sizing_multiplier;

        // 2. Size the order — brief lock on portfolio
        let (size_usdc, current_nav, open_positions) = {
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            let size = p.calculate_size_usdc(strategy_adjusted_notional);
            let nav = p.nav();
            let open = p.positions.len();
            (size, nav, open)
        };

        drop(_sizing_timer);

        // 3. Resolve executable size before touching market data.
        let size_usdc = match size_usdc {
            Some(s) => s,
            None => {
                // Skip like PaperEngine
                self.mark_skipped_order().await;
                self.audit_signal_decision(
                    &signal,
                    "blocked_sizing",
                    Some("portfolio_size_none"),
                    None,
                    Some(&metadata),
                    handle_started_at,
                );
                return;
            }
        };

        let quant_audit = self.quant_signal_audit(&signal, Some(size_usdc), Some(&metadata));
        if let Err(reason) = self.check_quant_canary_gate(&quant_audit) {
            warn!(token_id = %signal.token_id, side = %signal.side, reason, "Quant canary score gate blocked order");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("QUANT-BLOCKED: {reason}"));
            }
            let mut p = self.portfolio.lock().await;
            p.skipped_orders += 1;
            self.audit_signal_decision(
                &signal,
                "blocked_quant_score",
                Some(&reason),
                Some(size_usdc),
                Some(&metadata),
                handle_started_at,
            );
            return;
        }

        if let Err(reason) = self.check_canary_gate(&signal, size_usdc) {
            warn!(token_id = %signal.token_id, side = %signal.side, reason, "Canary gate blocked order");
            let alert_key = format!("live_canary_block:{reason}");
            crate::operator_alerts::emit_operator_alert(
                "live_canary_block",
                "warn",
                &alert_key,
                "Live canary blocked an entry order",
                serde_json::json!({
                    "token_id": &signal.token_id,
                    "side": signal.side.to_string(),
                    "reason": &reason,
                    "size_usdc": size_usdc,
                }),
            );
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("CANARY-BLOCKED: {reason}"));
            }
            let mut p = self.portfolio.lock().await;
            p.skipped_orders += 1;
            self.audit_signal_decision(
                &signal,
                "blocked_canary",
                Some(&reason),
                Some(size_usdc),
                Some(&metadata),
                handle_started_at,
            );
            return;
        }

        // ── Stage: Drift (FreshnessGate) ─────────────────────────────────
        let _drift_timer = StageTimer::start(HotStage::Drift);

        let gate_cfg = GateConfig::from_profile_and_env(self.execution_profile);
        let fast_taker_enabled = std::env::var("BLINK_FAST_TAKER")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false);
        let fast_taker_max_chase_bps = std::env::var("BLINK_FAST_TAKER_MAX_CHASE_BPS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(100)
            .min(1_000);
        let fast_taker_max_chase_ticks = std::env::var("BLINK_FAST_TAKER_MAX_CHASE_TICKS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(50)
            .min(250);

        let mut execution_price_u64 = signal.price.max(1);
        if fast_taker_enabled {
            if let Some((top_price, top_size)) =
                self.book_store.top_of_book(&signal.token_id, signal.side)
            {
                let signal_price = signal.price.max(1) as u128;
                let top = top_price as u128;
                let chase = fast_taker_max_chase_bps as u128;
                let within_chase = match signal.side {
                    OrderSide::Buy => top * 10_000 <= signal_price * (10_000 + chase),
                    OrderSide::Sell => top * 10_000 >= signal_price * (10_000 - chase),
                };
                let within_abs_chase = match signal.side {
                    OrderSide::Buy => {
                        top <= signal_price.saturating_add(fast_taker_max_chase_ticks as u128)
                    }
                    OrderSide::Sell => {
                        top.saturating_add(fast_taker_max_chase_ticks as u128) >= signal_price
                    }
                };
                if within_chase && within_abs_chase {
                    let max_price = ((signal_price * (10_000 + chase)) / 10_000) as u64;
                    let budget_base = (size_usdc * 1_000_000.0).floor() as u64;
                    let quant_price = match signal.side {
                        OrderSide::Buy => Self::best_budget_compatible_buy_price(
                            top_price.max(1),
                            max_price.max(top_price),
                            budget_base,
                        ),
                        OrderSide::Sell => Some(top_price.max(1)),
                    };
                    let Some(quant_price) = quant_price else {
                        warn!(
                            token_id = %signal.token_id,
                            side = %signal.side,
                            top_price = top_price,
                            max_price = max_price,
                            budget_base = budget_base,
                            "Fast taker skipped: no budget-compatible exact tick/precision price"
                        );
                        self.mark_skipped_order().await;
                        return;
                    };
                    execution_price_u64 = quant_price;
                    info!(
                        token_id = %signal.token_id,
                        side = %signal.side,
                        rn1_price = signal.price,
                        top_price = top_price,
                        execution_price = execution_price_u64,
                        top_size = top_size,
                        max_chase_bps = fast_taker_max_chase_bps,
                        max_chase_ticks = fast_taker_max_chase_ticks,
                        "Fast taker repriced to live top-of-book"
                    );
                }
            }
        }
        // Category-aware drift tolerance (min() semantics — override can only tighten).
        let market_class =
            crate::market_class::MarketClass::from_title_opt(signal.market_title.as_deref());
        let max_drift_bps = gate_cfg.max_drift_bps_for_class(market_class);
        let gate_post_only = gate_cfg.post_only && !fast_taker_enabled;
        let mut gate_result = self.pretrade_gate.check(
            &signal.token_id,
            signal.side,
            execution_price_u64,
            gate_cfg.stale_ms,
            max_drift_bps,
            gate_post_only,
        );
        if matches!(gate_result, GateDecision::SkipStale)
            && self.seed_order_book_from_rest(&signal.token_id).await
        {
            if fast_taker_enabled {
                if let Some((top_price, top_size)) =
                    self.book_store.top_of_book(&signal.token_id, signal.side)
                {
                    let signal_price = signal.price.max(1) as u128;
                    let top = top_price as u128;
                    let chase = fast_taker_max_chase_bps as u128;
                    let within_chase = match signal.side {
                        OrderSide::Buy => top * 10_000 <= signal_price * (10_000 + chase),
                        OrderSide::Sell => top * 10_000 >= signal_price * (10_000 - chase),
                    };
                    let within_abs_chase = match signal.side {
                        OrderSide::Buy => {
                            top <= signal_price.saturating_add(fast_taker_max_chase_ticks as u128)
                        }
                        OrderSide::Sell => {
                            top.saturating_add(fast_taker_max_chase_ticks as u128) >= signal_price
                        }
                    };
                    if within_chase && within_abs_chase {
                        let max_price = ((signal_price * (10_000 + chase)) / 10_000) as u64;
                        let budget_base = (size_usdc * 1_000_000.0).floor() as u64;
                        let quant_price = match signal.side {
                            OrderSide::Buy => Self::best_budget_compatible_buy_price(
                                top_price.max(1),
                                max_price.max(top_price),
                                budget_base,
                            ),
                            OrderSide::Sell => Some(top_price.max(1)),
                        };
                        let Some(quant_price) = quant_price else {
                            warn!(
                                token_id = %signal.token_id,
                                side = %signal.side,
                                top_price = top_price,
                                max_price = max_price,
                                budget_base = budget_base,
                                "Fast taker skipped: no budget-compatible exact tick/precision price"
                            );
                            self.mark_skipped_order().await;
                            return;
                        };
                        execution_price_u64 = quant_price;
                        info!(
                            token_id = %signal.token_id,
                            side = %signal.side,
                            rn1_price = signal.price,
                            top_price = top_price,
                            execution_price = execution_price_u64,
                            top_size = top_size,
                            max_chase_bps = fast_taker_max_chase_bps,
                            max_chase_ticks = fast_taker_max_chase_ticks,
                            "Fast taker repriced to REST-seeded top-of-book"
                        );
                    }
                }
            }
            gate_result = self.pretrade_gate.check(
                &signal.token_id,
                signal.side,
                execution_price_u64,
                gate_cfg.stale_ms,
                max_drift_bps,
                gate_post_only,
            );
        }
        match gate_result {
            GateDecision::Proceed => {
                crate::hot_metrics::counters()
                    .gate_proceed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            GateDecision::SkipStale => {
                crate::hot_metrics::counters()
                    .gate_skip_stale
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                warn!(token_id = %signal.token_id, "⛔ Gate: stale book snapshot — signal dropped");
                self.mark_skipped_order().await;
                self.audit_signal_decision(
                    &signal,
                    "blocked_stale_book",
                    Some("pretrade_gate_stale_snapshot"),
                    Some(size_usdc),
                    Some(&metadata),
                    handle_started_at,
                );
                return;
            }
            GateDecision::SkipDrift { bps } => {
                crate::hot_metrics::counters()
                    .gate_skip_drift
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.failsafe_metrics.lock_or_recover().trigger_count += 1;
                warn!(token_id = %signal.token_id, drift_bps = bps, "⛔ Gate: drift too large — signal dropped");
                self.mark_skipped_order().await;
                let reason = format!("pretrade_gate_drift_{bps}_bps");
                self.audit_signal_decision(
                    &signal,
                    "blocked_drift",
                    Some(&reason),
                    Some(size_usdc),
                    Some(&metadata),
                    handle_started_at,
                );
                return;
            }
            GateDecision::SkipPostOnlyCross => {
                crate::hot_metrics::counters()
                    .gate_skip_post_only
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                warn!(token_id = %signal.token_id, "⛔ Gate: post-only cross — signal dropped");
                self.mark_skipped_order().await;
                self.audit_signal_decision(
                    &signal,
                    "blocked_post_only_cross",
                    Some("pretrade_gate_post_only_cross"),
                    Some(size_usdc),
                    Some(&metadata),
                    handle_started_at,
                );
                return;
            }
        }

        drop(_drift_timer);

        if !Self::is_valid_clob_price_u64(execution_price_u64) {
            warn!(
                token_id = %signal.token_id,
                side = %signal.side,
                execution_price = execution_price_u64,
                "Skipping live signal: execution price outside Polymarket CLOB range"
            );
            self.mark_skipped_order().await;
            self.audit_signal_decision(
                &signal,
                "blocked_invalid_clob_price",
                Some("execution_price_must_be_between_0_and_1"),
                Some(size_usdc),
                Some(&metadata),
                handle_started_at,
            );
            return;
        }

        if matches!(signal.side, OrderSide::Buy) {
            let budget_base_units = (size_usdc * 1_000_000.0).floor() as u64;
            let min_required_base = Self::min_valid_buy_maker_amount_base(execution_price_u64);
            if budget_base_units < min_required_base {
                warn!(
                    token_id = %signal.token_id,
                    side = %signal.side,
                    execution_price = execution_price_u64,
                    budget_base_units = budget_base_units,
                    min_required_base = min_required_base,
                    "Skipping live BUY signal: budget cannot satisfy Polymarket min order after tick/precision quantization"
                );
                self.mark_skipped_order().await;
                self.audit_signal_decision(
                    &signal,
                    "blocked_min_order_quantization",
                    Some("budget_below_min_required_for_price_precision"),
                    Some(size_usdc),
                    Some(&metadata),
                    handle_started_at,
                );
                return;
            }
        }

        // ── Stage: Risk ───────────────────────────────────────────────────
        let _risk_timer = StageTimer::start(HotStage::Risk);

        // Risk check must run after local price/book gates. Otherwise drift- or
        // stale-rejected candidates consume the 1/sec live-canary rate token and
        // block the first actually executable signal in the same burst.
        if let Err(violation) = self.risk.lock_or_recover().check_pre_order(
            size_usdc,
            open_positions,
            current_nav,
            self.starting_nav_usdc,
        ) {
            warn!("🛑 Risk check blocked order: {violation}");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("BLOCKED: {violation}"));
            }
            self.mark_skipped_order().await;
            self.audit_signal_decision(
                &signal,
                "blocked_risk",
                Some(&violation.to_string()),
                Some(size_usdc),
                Some(&metadata),
                handle_started_at,
            );
            return;
        }

        drop(_risk_timer);

        // ── Stage: Sign ───────────────────────────────────────────────────
        let _sign_timer = StageTimer::start(HotStage::Sign);

        // 5. Build and sign (or dry-run)
        let now = self.next_order_timestamp_ms();

        let params = OrderParams {
            token_id: signal.token_id.clone(),
            side: signal.side,
            price: execution_price_u64,
            size: size_usdc,
            maker: self.funder_addr.clone(),
            builder: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            timestamp: now,
            metadata: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        };

        let dry_run_accept = || {
            info!("DRY-RUN: would sign & submit order");
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Fill,
                    format!(
                        "DRY-RUN {} @{:.3} ${:.2}",
                        signal.side, entry_price, size_usdc
                    ),
                );
            }
            (true, None::<String>)
        };

        let (accepted, exchange_order_id) = if self.executor.dry_run {
            dry_run_accept()
        } else if let Some(vault) = self.vault.as_ref() {
            drop(_sign_timer);
            match sign_order_for_intent_with_vault_handle_policy(
                vault.as_ref(),
                &params,
                self.signing_policy,
                signal.intent_id,
            )
            .await
            {
                Ok(signed) => {
                    // Build OrderIntent with pre-signed payload for idempotent retry.
                    let order_intent = OrderIntent {
                        intent_id: signal.intent_id,
                        market_id: signal.market_id.clone().unwrap_or_default(),
                        token_id: signal.token_id.clone(),
                        side: signal.side,
                        price_u64: execution_price_u64,
                        size_u64: (size_usdc * 1_000.0) as u64,
                        tif: if fast_taker_enabled {
                            TimeInForce::Fak
                        } else {
                            TimeInForce::Gtc
                        },
                        strategy_mode: strategy_snapshot.current_mode,
                        requested_at: std::time::Instant::now(),
                        signed_payload: Some(signed),
                    };
                    match self.order_router.submit(order_intent).await {
                        Ok(()) => {
                            info!(
                                intent_id = signal.intent_id,
                                "→ OrderRouter: intent queued for submission"
                            );
                            if let Some(ref log) = self.activity {
                                log_push(
                                    log,
                                    EntryKind::Fill,
                                    format!(
                                        "SUBMIT QUEUED (NOT FILLED) {} @{:.3} ${:.2}",
                                        signal.side, entry_price, size_usdc
                                    ),
                                );
                            }
                            // Fill accounting is performed by the router reconciler.
                            (true, None)
                        }
                        Err(e) => {
                            error!(
                                intent_id = signal.intent_id,
                                error = %e,
                                "❌ OrderRouter full — intent dropped"
                            );
                            (false, None)
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "❌ EIP-712 signing failed");
                    (false, None)
                }
            }
        } else {
            dry_run_accept()
        };

        if !accepted {
            self.bump_reject_streak();
            warn!(
                token_id = %signal.token_id,
                side = %signal.side,
                "Skipping local fill accounting because live order was not accepted"
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!(
                        "LIVE REJECTED {} @{:.3} ${:.2}",
                        signal.side, entry_price, size_usdc
                    ),
                );
            }
            self.mark_skipped_order().await;
            self.audit_signal_decision(
                &signal,
                "rejected_submit_or_sign",
                Some("order_not_accepted"),
                Some(size_usdc),
                Some(&metadata),
                handle_started_at,
            );
            return;
        }

        self.record_canary_accept(size_usdc);
        self.audit_signal_decision(
            &signal,
            if self.executor.dry_run || self.vault.is_none() {
                "accepted_dry_run"
            } else {
                "queued_live"
            },
            None,
            Some(size_usdc),
            Some(&metadata),
            handle_started_at,
        );

        // 6. Fill accounting — exchange-first (SSOT) principle.
        //
        // For live orders with a real exchange_order_id: the fill is NOT
        // recorded locally yet.  Instead, the order is queued in
        // `pending_orders` and the reconciliation worker will call
        // `process_order_status()` to confirm the actual `size_matched` from
        // the exchange before updating portfolio and risk manager.
        //
        // For dry-run orders (no exchange_order_id): record the fill
        // immediately because there is no exchange to confirm against.
        if let Some(order_id) = exchange_order_id {
            self.pending_orders.lock().await.insert(
                order_id.clone(),
                PendingOrder::new(
                    order_id,
                    signal.token_id.clone(),
                    signal.side,
                    size_usdc,
                    entry_price,
                ),
            );
            self.persist_wal().await;
            info!(
                token_id      = %signal.token_id,
                expected_usdc = size_usdc,
                "🕐 Fill deferred — awaiting exchange confirmation via reconciliation"
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Fill,
                    format!(
                        "PENDING CONFIRM {} @{:.3} ${:.2}",
                        signal.side, entry_price, size_usdc
                    ),
                );
            }
        } else if self.executor.dry_run || self.vault.is_none() {
            // DRY-RUN / paper mode: no exchange_order_id means no real order
            // was submitted, so record fill immediately (simulation only).
            {
                let mut p = self.portfolio.lock().await;
                p.open_position(
                    signal.token_id.clone(),
                    signal.side,
                    entry_price,
                    size_usdc,
                    signal.order_id.clone(),
                );
            }
            self.risk.lock_or_recover().record_fill(size_usdc);
        } else {
            // Live router path: OrderRouter::submit() means queued locally, not
            // exchange-accepted. Never mark a fill here; reconciler/exchange
            // truth must be the source of portfolio accounting.
            info!(
                intent_id = signal.intent_id,
                token_id = %signal.token_id,
                "Live order queued without exchange order id; local fill accounting skipped"
            );
        }
    }

    fn best_budget_compatible_buy_price(
        top_price_u64: u64,
        max_price_u64: u64,
        budget_base_units: u64,
    ) -> Option<u64> {
        const MIN_MARKETABLE_BUY_BASE: u64 = 1_000_000;
        if budget_base_units < MIN_MARKETABLE_BUY_BASE {
            return None;
        }

        let lower = top_price_u64.max(Self::MIN_CLOB_PRICE_U64);
        let upper = max_price_u64.max(lower).min(Self::MAX_CLOB_PRICE_U64);
        if lower > upper {
            return None;
        }

        (lower..=upper)
            .find(|&price| Self::min_valid_buy_maker_amount_base(price) <= budget_base_units)
    }

    fn is_valid_clob_price_u64(price_u64: u64) -> bool {
        (Self::MIN_CLOB_PRICE_U64..=Self::MAX_CLOB_PRICE_U64).contains(&price_u64)
    }

    fn min_valid_buy_maker_amount_base(price_u64: u64) -> u64 {
        const MIN_MARKETABLE_BUY_BASE: u64 = 1_000_000;
        let exact_step = Self::exact_buy_maker_step_base(price_u64);
        let steps = MIN_MARKETABLE_BUY_BASE.div_ceil(exact_step);
        exact_step.saturating_mul(steps)
    }

    fn exact_buy_maker_step_base(price_u64: u64) -> u64 {
        let price = price_u64.max(1);
        let lot_multiple = 10_000u64 / Self::gcd_u64(price, 10_000);
        price.saturating_mul(lot_multiple)
    }

    fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
        while b != 0 {
            let r = a % b;
            a = b;
            b = r;
        }
        a.max(1)
    }

    fn next_order_timestamp_ms(&self) -> u64 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut current = self.nonce_counter.load(Ordering::Relaxed);

        loop {
            let next = now_ms.max(current.saturating_add(1));
            match self.nonce_counter.compare_exchange(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
    }

    async fn check_market_metadata_gate(
        &self,
        signal: &RN1Signal,
    ) -> std::result::Result<MarketMetadata, String> {
        let metadata = match self.metadata_fetcher.fetch(&signal.token_id).await {
            Ok(metadata) => metadata,
            Err(e) if self.market_safety_policy.allow_unknown_metadata => {
                warn!(
                    token_id = %signal.token_id,
                    error = %e,
                    "Market metadata fetch failed but BLINK_ALLOW_UNKNOWN_MARKET_METADATA=true"
                );
                MarketMetadata {
                    market_id: signal.market_id.clone().unwrap_or_default(),
                    token_id: signal.token_id.clone(),
                    category: "unknown".to_string(),
                    tags: Vec::new(),
                    volume_24h: 0.0,
                    liquidity: 0.0,
                    event_start_time: signal.event_start_time,
                    event_end_time: signal.event_end_time,
                    closed: false,
                    neg_risk: false,
                    enable_neg_risk: false,
                    minimum_tick_size: None,
                }
            }
            Err(e) => return Err(format!("metadata_fetch_failed: {e}")),
        };

        if metadata.closed {
            return Err("market_closed".to_string());
        }

        if (metadata.neg_risk || metadata.enable_neg_risk)
            && !self.market_safety_policy.allow_neg_risk
        {
            crate::operator_alerts::emit_operator_alert(
                "negative_risk_blocked",
                "warn",
                "negative_risk_blocked",
                "Negative-risk market blocked before live submit",
                serde_json::json!({
                    "token_id": &signal.token_id,
                    "market_id": &metadata.market_id,
                    "neg_risk": metadata.neg_risk,
                    "enable_neg_risk": metadata.enable_neg_risk,
                    "allow_neg_risk": self.market_safety_policy.allow_neg_risk,
                }),
            );
            return Err("neg_risk_market_blocked_until_neg_risk_signing_enabled".to_string());
        }

        match metadata
            .minimum_tick_size
            .as_deref()
            .and_then(parse_tick_size)
        {
            Some(tick)
                if tick + f64::EPSILON < self.market_safety_policy.min_supported_tick_size =>
            {
                return Err(format!(
                    "tick_size_unsupported {:.6}<min_supported {:.6}",
                    tick, self.market_safety_policy.min_supported_tick_size
                ));
            }
            Some(_) => {}
            None if !self.market_safety_policy.allow_unknown_metadata => {
                return Err("metadata_missing_or_invalid_tick_size".to_string());
            }
            None => {}
        }

        Ok(metadata)
    }

    fn check_live_side_gate(&self, signal: &RN1Signal) -> std::result::Result<(), String> {
        if signal.side == OrderSide::Sell && !self.market_safety_policy.allow_live_sell {
            return Err(
                "live_sell_blocked_until_position_unwind_and_allowance_support".to_string(),
            );
        }
        Ok(())
    }

    async fn seed_order_book_from_rest(&self, token_id: &str) -> bool {
        let book = match self.rest_clob.get_order_book(token_id).await {
            Ok(book) => book,
            Err(e) => {
                warn!(
                    token_id,
                    error = %e,
                    "REST order-book seed failed after stale pretrade snapshot"
                );
                return false;
            }
        };

        let bids = parse_rest_book_levels(&book, "bids");
        let asks = parse_rest_book_levels(&book, "asks");
        if bids.is_empty() && asks.is_empty() {
            warn!(
                token_id,
                "REST order-book seed returned no priced levels after stale pretrade snapshot"
            );
            return false;
        }

        self.book_store.replace_snapshot(token_id, &bids, &asks);
        info!(
            token_id,
            bids = bids.len(),
            asks = asks.len(),
            "REST order-book seed applied for stale pretrade snapshot"
        );
        true
    }

    async fn mark_skipped_order(&self) {
        let mut p = self.portfolio.lock().await;
        p.skipped_orders += 1;
    }

    fn record_rn1_signal(&self, signal: &RN1Signal) {
        let Some(tx) = self.warehouse_tx.as_ref() else {
            return;
        };

        let event = WarehouseEvent::Rn1Signal(Rn1SignalRecord {
            timestamp_ms: current_time_ms(),
            token_id: signal.token_id.clone(),
            side: signal.side.to_string(),
            price: signal.price,
            size: signal.size,
            wallet: signal.source_wallet.clone(),
        });

        if tx.try_send(event).is_err() {
            warn!(
                token_id = %signal.token_id,
                "warehouse channel full: dropped live RN1 signal record"
            );
        }
    }

    fn quant_signal_audit(
        &self,
        signal: &RN1Signal,
        size_usdc: Option<f64>,
        metadata: Option<&MarketMetadata>,
    ) -> QuantSignalAudit {
        let book = self.book_store.get_book_snapshot(&signal.token_id);
        let spread_bps = book.as_ref().and_then(|book| book.spread_bps());
        let book_age_ms = self
            .book_store
            .get_snapshot_age_ms(&signal.token_id)
            .map(u64::from);
        let depth_usdc = book
            .as_ref()
            .map(|book| contra_depth_usdc(book, signal.side, 5));
        let price = signal.price as f64 / 1_000.0;
        let rn1_shares = signal.size as f64 / 1_000.0;
        let score = score_signal(QuantSignalFeatures {
            price_u64: signal.price,
            rn1_notional_usd: rn1_shares * price,
            intended_size_usdc: size_usdc,
            spread_bps,
            book_age_ms,
            contra_depth_usdc: depth_usdc,
            market_liquidity_usd: metadata.map(|m| m.liquidity),
            volume_24h_usd: metadata.map(|m| m.volume_24h),
            neg_risk: metadata
                .map(|m| m.neg_risk || m.enable_neg_risk)
                .unwrap_or(false),
        });

        QuantSignalAudit {
            score,
            spread_bps,
            book_age_ms,
            depth_usdc,
        }
    }

    fn check_quant_canary_gate(&self, audit: &QuantSignalAudit) -> Result<(), String> {
        if !self.quant_canary_policy.enabled {
            return Ok(());
        }

        let has_complete_features =
            audit.spread_bps.is_some() && audit.book_age_ms.is_some() && audit.depth_usdc.is_some();
        if self.quant_canary_policy.require_complete_features && !has_complete_features {
            return Err("quant_features_incomplete".to_string());
        }
        if !has_complete_features {
            return Ok(());
        }

        if audit.score.score_bps < self.quant_canary_policy.min_score_bps {
            return Err(format!(
                "score_bps_below_canary_min {}<{} grade={}",
                audit.score.score_bps, self.quant_canary_policy.min_score_bps, audit.score.grade
            ));
        }
        if audit.score.toxicity_bps > self.quant_canary_policy.max_toxicity_bps {
            return Err(format!(
                "toxicity_bps_above_canary_max {}>{}",
                audit.score.toxicity_bps, self.quant_canary_policy.max_toxicity_bps
            ));
        }
        Ok(())
    }

    fn emit_shadow_decision(
        &self,
        signal: &RN1Signal,
        decision: &str,
        reason: Option<&str>,
        size_usdc: Option<f64>,
        started_at: Instant,
        audit: &QuantSignalAudit,
    ) {
        let Some(tx) = self.warehouse_tx.as_ref() else {
            return;
        };

        let price = signal.price as f64 / 1_000.0;
        let rn1_shares = signal.size as f64 / 1_000.0;
        let strategy_snapshot = self.strategy_controller.snapshot();
        let event = WarehouseEvent::ShadowDecision(ShadowDecisionRecord {
            timestamp_ms: current_time_ms(),
            token_id: signal.token_id.clone(),
            side: signal.side.to_string(),
            decision: decision.to_string(),
            reason: reason.unwrap_or("").to_string(),
            quant_decision: audit.score.shadow_decision.to_string(),
            quant_reason: audit.score.shadow_reason.to_string(),
            signal_price: signal.price,
            signal_size: signal.size,
            rn1_notional_usd: rn1_shares * price,
            intended_size_usdc: size_usdc.unwrap_or(0.0),
            score_bps: audit.score.score_bps,
            score_grade: audit.score.grade.to_string(),
            toxicity_bps: audit.score.toxicity_bps,
            spread_bps: audit.spread_bps.map(|v| v as i64).unwrap_or(-1),
            book_age_ms: audit.book_age_ms.map(|v| v as i64).unwrap_or(-1),
            depth_usdc: audit.depth_usdc.unwrap_or(0.0),
            decision_latency_ms: started_at.elapsed().as_millis() as u64,
            signal_source: signal.signal_source.clone(),
            strategy_mode: strategy_snapshot.current_mode.to_string(),
        });

        if tx.try_send(event).is_err() {
            warn!(
                token_id = %signal.token_id,
                "warehouse channel full: dropped live shadow decision record"
            );
        }
    }

    fn audit_signal_decision(
        &self,
        signal: &RN1Signal,
        decision: &str,
        reason: Option<&str>,
        size_usdc: Option<f64>,
        metadata: Option<&MarketMetadata>,
        started_at: Instant,
    ) {
        let quant_audit = self.quant_signal_audit(signal, size_usdc, metadata);
        self.emit_shadow_decision(
            signal,
            decision,
            reason,
            size_usdc,
            started_at,
            &quant_audit,
        );
        let rn1_shares = signal.size as f64 / 1_000.0;
        let price = signal.price as f64 / 1_000.0;

        if !self.shadow_audit.enabled {
            return;
        }

        let path = Path::new(&self.shadow_audit.path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    warn!(path = %self.shadow_audit.path, error = %e, "shadow audit mkdir failed");
                    return;
                }
            }
        }

        let event = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "decision": decision,
            "reason": reason,
            "token_id": &signal.token_id,
            "market_id": signal.market_id.as_deref(),
            "market_title": signal.market_title.as_deref(),
            "market_outcome": signal.market_outcome.as_deref(),
            "side": signal.side.to_string(),
            "price_scaled": signal.price,
            "price": price,
            "size_usdc": size_usdc,
            "rn1_size_scaled": signal.size,
            "rn1_shares": rn1_shares,
            "rn1_notional_usd": rn1_shares * price,
            "intent_id": signal.intent_id,
            "source_order_id": signal.source_order_id.as_deref(),
            "source_seq": signal.source_seq,
            "signal_source": &signal.signal_source,
            "signal_age_ms": signal.detected_at.elapsed().as_millis() as u64,
            "queue_age_ms": signal.enqueued_at.elapsed().as_millis() as u64,
            "decision_latency_ms": started_at.elapsed().as_millis() as u64,
            "quant_score_bps": quant_audit.score.score_bps,
            "quant_score_grade": quant_audit.score.grade,
            "quant_toxicity_bps": quant_audit.score.toxicity_bps,
            "quant_shadow_decision": quant_audit.score.shadow_decision,
            "quant_shadow_reason": quant_audit.score.shadow_reason,
            "quant_spread_bps": quant_audit.spread_bps,
            "quant_book_age_ms": quant_audit.book_age_ms,
            "quant_depth_usdc": quant_audit.depth_usdc,
            "quant_canary_score_gate_enabled": self.quant_canary_policy.enabled,
            "quant_canary_min_score_bps": self.quant_canary_policy.min_score_bps,
            "quant_canary_max_toxicity_bps": self.quant_canary_policy.max_toxicity_bps,
            "metadata_neg_risk": metadata.map(|m| m.neg_risk),
            "metadata_enable_neg_risk": metadata.map(|m| m.enable_neg_risk),
            "metadata_tick_size": metadata.and_then(|m| m.minimum_tick_size.as_deref()),
            "metadata_category": metadata.map(|m| m.category.as_str()),
            "metadata_liquidity": metadata.map(|m| m.liquidity),
            "metadata_volume_24h": metadata.map(|m| m.volume_24h),
        });

        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                if let Ok(line) = serde_json::to_string(&event) {
                    if let Err(e) = writeln!(file, "{line}") {
                        warn!(path = %self.shadow_audit.path, error = %e, "shadow audit write failed");
                    }
                }
            }
            Err(e) => warn!(path = %self.shadow_audit.path, error = %e, "shadow audit open failed"),
        }
    }

    fn check_canary_gate(&self, signal: &RN1Signal, size_usdc: f64) -> Result<(), String> {
        let state = self.canary_state.lock_or_recover();
        if state.halted {
            return Err("canary_halted_after_reject_streak".to_string());
        }
        if size_usdc > self.canary_policy.max_order_usdc {
            return Err(format!(
                "size_usdc_exceeds_canary_limit {:.2}>{:.2}",
                size_usdc, self.canary_policy.max_order_usdc
            ));
        }
        if state.accepted_spend_usdc + size_usdc > self.canary_policy.max_session_spend_usdc {
            return Err(format!(
                "session_spend_cap_reached {:.2}+{:.2}>{:.2}",
                state.accepted_spend_usdc, size_usdc, self.canary_policy.max_session_spend_usdc
            ));
        }
        if self.canary_policy.max_orders_per_session > 0
            && state.accepted_orders >= self.canary_policy.max_orders_per_session
        {
            return Err("session_order_cap_reached".to_string());
        }
        if self.canary_policy.daytime_only {
            let hour = chrono::Utc::now().hour() as u8;
            if !hour_in_window(
                hour,
                self.canary_policy.start_hour_utc,
                self.canary_policy.end_hour_utc,
            ) {
                return Err(format!(
                    "outside_daytime_window hour={} window={}..{}",
                    hour, self.canary_policy.start_hour_utc, self.canary_policy.end_hour_utc
                ));
            }
        }
        if !self.canary_policy.allowed_markets.is_empty()
            && !self
                .canary_policy
                .allowed_markets
                .iter()
                .any(|m| m == &signal.token_id)
        {
            return Err("token_not_in_canary_allowlist".to_string());
        }
        Ok(())
    }

    fn bump_reject_streak(&self) {
        let mut state = self.canary_state.lock_or_recover();
        state.reject_streak += 1;
        if state.reject_streak >= self.canary_policy.max_reject_streak {
            state.halted = true;
            let _ = std::fs::write(
                "logs/CANARY_HALTED.flag",
                format!(
                    "halted=true reject_streak={} threshold={}\n",
                    state.reject_streak, self.canary_policy.max_reject_streak
                ),
            );
            error!(
                reject_streak = state.reject_streak,
                threshold = self.canary_policy.max_reject_streak,
                "CANARY HALTED: operator intervention required"
            );
            crate::operator_alerts::emit_operator_alert(
                "live_canary_halted",
                "critical",
                "live_canary_halted:reject_streak",
                "Live canary halted after reject streak",
                serde_json::json!({
                    "reject_streak": state.reject_streak,
                    "threshold": self.canary_policy.max_reject_streak,
                    "flag_path": "logs/CANARY_HALTED.flag",
                }),
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!(
                        "CANARY HALTED: reject_streak={} threshold={} (logs/CANARY_HALTED.flag)",
                        state.reject_streak, self.canary_policy.max_reject_streak
                    ),
                );
            }
        }
    }

    fn record_canary_accept(&self, size_usdc: f64) {
        let mut state = self.canary_state.lock_or_recover();
        state.accepted_orders += 1;
        state.accepted_spend_usdc += size_usdc.max(0.0);
        state.reject_streak = 0;
        state.last_accept_ms = current_time_ms();
    }

    fn record_canary_closed_trade(&self, realized_pnl: f64) {
        if self.canary_policy.max_loss_streak == 0 {
            return;
        }

        let mut state = self.canary_state.lock_or_recover();
        if realized_pnl < 0.0 {
            state.loss_streak += 1;
            if state.loss_streak >= self.canary_policy.max_loss_streak {
                state.halted = true;
                let _ = std::fs::write(
                    "logs/CANARY_HALTED.flag",
                    format!(
                        "halted=true loss_streak={} threshold={}\n",
                        state.loss_streak, self.canary_policy.max_loss_streak
                    ),
                );
                error!(
                    loss_streak = state.loss_streak,
                    threshold = self.canary_policy.max_loss_streak,
                    "CANARY HALTED: realized loss streak threshold reached"
                );
                crate::operator_alerts::emit_operator_alert(
                    "live_canary_halted",
                    "critical",
                    "live_canary_halted:loss_streak",
                    "Live canary halted after realized loss streak",
                    serde_json::json!({
                        "loss_streak": state.loss_streak,
                        "threshold": self.canary_policy.max_loss_streak,
                        "flag_path": "logs/CANARY_HALTED.flag",
                    }),
                );
                if let Some(ref log) = self.activity {
                    log_push(
                        log,
                        EntryKind::Warn,
                        format!(
                            "CANARY HALTED: loss_streak={} threshold={} (logs/CANARY_HALTED.flag)",
                            state.loss_streak, self.canary_policy.max_loss_streak
                        ),
                    );
                }
            }
        } else if realized_pnl > 0.0 {
            state.loss_streak = 0;
        }
    }

    async fn classify_signal_intent(&self, signal: &RN1Signal) -> SignalIntent {
        let (same_side, opposite_side) = {
            let p = self.portfolio.lock().await;
            let mut same = 0usize;
            let mut opposite = 0usize;
            for pos in p
                .positions
                .iter()
                .filter(|pos| pos.token_id == signal.token_id)
            {
                if pos.side == signal.side {
                    same += 1;
                } else {
                    opposite += 1;
                }
            }
            (same, opposite)
        };
        classify_intent_from_counts(same_side, opposite_side)
    }

    async fn sync_risk_closes_from_portfolio(&self) {
        let mut accounted = self.accounted_closed_trades.lock().await;
        let (new_count, realized_delta, realized_trades) = {
            let p = self.portfolio.lock().await;
            if *accounted >= p.closed_trades.len() {
                return;
            }
            let trades = p.closed_trades[*accounted..]
                .iter()
                .map(|t| t.realized_pnl)
                .collect::<Vec<_>>();
            let delta = trades.iter().copied().sum::<f64>();
            (p.closed_trades.len(), delta, trades)
        };

        self.risk.lock_or_recover().record_close(realized_delta);
        for realized_pnl in realized_trades {
            self.record_canary_closed_trade(realized_pnl);
        }
        *accounted = new_count;
    }

    async fn run_reconciliation_pass(&self) {
        let _reconcile_start = std::time::Instant::now();
        let _reconcile_timer = StageTimer::start(HotStage::Reconcile);
        self.sync_risk_closes_from_portfolio().await;

        let pending_ids: Vec<String> = self.pending_orders.lock().await.keys().cloned().collect();

        let mut resolved = 0usize;
        let mut fills_recorded = 0usize;

        for order_id in pending_ids {
            // Fetch latest status from exchange.
            let status = match self.executor.get_order_status(&order_id).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(order_id = %order_id, error = %e, "Reconciliation status fetch failed");
                    continue;
                }
            };

            // Run the SSOT reconciler to determine the outcome.
            let outcome = {
                let mut map = self.pending_orders.lock().await;
                match map.get_mut(&order_id) {
                    Some(pending) => process_order_status(pending, &status),
                    None => continue, // removed by concurrent path
                }
            };

            match outcome {
                // ── Exchange confirmed fill ────────────────────────────────
                ReconciliationOutcome::Fill {
                    token_id,
                    side,
                    actual_size_usdc,
                    actual_size_shares: _,
                    submitted_price,
                    partial_fill,
                    fill_ratio,
                } => {
                    // Record fill using exchange-confirmed actual amounts —
                    // NOT the expected amounts from submission time.
                    {
                        let mut p = self.portfolio.lock().await;
                        p.open_position(
                            token_id.clone(),
                            side,
                            submitted_price,
                            actual_size_usdc,
                            order_id.clone(),
                        );
                    }
                    self.risk.lock_or_recover().record_fill(actual_size_usdc);
                    fills_recorded += 1;
                    resolved += 1;
                    self.failsafe_metrics.lock_or_recover().confirmed_fills += 1;

                    if partial_fill {
                        warn!(
                            order_id   = %order_id,
                            fill_ratio = format!("{:.1}%", fill_ratio * 100.0),
                            actual_usdc = actual_size_usdc,
                            "⚠️  Partial fill recorded from exchange confirmation"
                        );
                        if let Some(ref log) = self.activity {
                            log_push(
                                log,
                                EntryKind::Warn,
                                format!(
                                    "PARTIAL FILL {side} @{submitted_price:.3} ${actual_size_usdc:.2} ({:.0}%)",
                                    fill_ratio * 100.0
                                ),
                            );
                        }
                    } else {
                        info!(
                            order_id    = %order_id,
                            actual_usdc = actual_size_usdc,
                            "✅ Fill confirmed and recorded from exchange"
                        );
                        if let Some(ref log) = self.activity {
                            log_push(
                                log,
                                EntryKind::Fill,
                                format!(
                                    "CONFIRMED FILL {side} @{submitted_price:.3} ${actual_size_usdc:.2}"
                                ),
                            );
                        }
                    }
                    self.pending_orders.lock().await.remove(&order_id);
                    self.persist_wal().await;
                }

                // ── Exchange did not fill ──────────────────────────────────
                ReconciliationOutcome::NoFill { token_id, reason } => {
                    // The order was not filled — local state must NOT be
                    // updated.  No position, no risk charge.
                    warn!(
                        order_id = %order_id,
                        token_id = %token_id,
                        reason   = %reason,
                        "Order not filled by exchange — no local state recorded"
                    );
                    if let Some(ref log) = self.activity {
                        log_push(
                            log,
                            EntryKind::Warn,
                            format!("NO FILL {token_id}: {reason}"),
                        );
                    }
                    self.bump_reject_streak();
                    resolved += 1;
                    self.failsafe_metrics.lock_or_recover().no_fills += 1;
                    self.pending_orders.lock().await.remove(&order_id);
                    self.persist_wal().await;
                }

                // ── Stale order alert ──────────────────────────────────────
                ReconciliationOutcome::SuspectedStale { elapsed_secs } => {
                    error!(
                        order_id     = %order_id,
                        elapsed_secs,
                        "🚨 Stale pending order — investigate on exchange; no local state change"
                    );
                    crate::operator_alerts::emit_operator_alert(
                        "stale_order",
                        "critical",
                        &format!("stale_order:{order_id}"),
                        "Live order is stale and still pending reconciliation",
                        serde_json::json!({
                            "order_id": order_id,
                            "elapsed_secs": elapsed_secs,
                        }),
                    );
                    if let Some(ref log) = self.activity {
                        log_push(
                            log,
                            EntryKind::Warn,
                            format!("STALE ORDER {order_id} pending {elapsed_secs}s — operator review required"),
                        );
                    }
                    self.failsafe_metrics.lock_or_recover().stale_orders += 1;
                    // Keep in pending_orders — will retry every reconcile pass.
                }

                // ── Not terminal yet ──────────────────────────────────────
                ReconciliationOutcome::StillPending => {
                    // Order is still live on exchange; retry next pass.
                }
            }
        }

        if resolved > 0 {
            let pending = self.pending_orders.lock().await.len();
            info!(
                resolved,
                fills_recorded, pending, "Reconciliation pass completed"
            );
        }

        // Update reconcile lag counter
        crate::hot_metrics::counters().reconcile_lag_ms_last.store(
            _reconcile_start.elapsed().as_millis() as i64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    #[cfg(feature = "legacy-fill-window")]
    async fn check_fill_window(&self, token_id: &str, entry_price: f64, _side: OrderSide) -> bool {
        for check in 0_u8..6 {
            sleep(Duration::from_millis(500)).await;
            if let Some(current) = self.get_market_price(token_id) {
                let drift = (current - entry_price).abs() / entry_price;
                {
                    let mut metrics = self.failsafe_metrics.lock_or_recover();
                    metrics.check_count += 1;
                    let drift_bps = (drift * 10_000.0).round().max(0.0) as u64;
                    if drift_bps > metrics.max_observed_drift_bps {
                        metrics.max_observed_drift_bps = drift_bps;
                    }
                }
                if drift > drift_threshold() {
                    self.failsafe_metrics.lock_or_recover().trigger_count += 1;
                    warn!(
                        check,
                        "🚨 Fill window abort: price drifted {:.2}%",
                        drift * 100.0
                    );
                    return false;
                }
            }
        }
        true
    }

    #[cfg(feature = "legacy-fill-window")]
    fn get_market_price(&self, token_id: &str) -> Option<f64> {
        self.book_store
            .get_mid_price(token_id)
            .map(|p| p as f64 / 1_000.0)
    }

    pub fn failsafe_metrics_snapshot(&self) -> FailsafeMetricsSnapshot {
        let m = self.failsafe_metrics.lock_or_recover();
        let total_resolved = m.confirmed_fills + m.no_fills;
        let confirmation_rate_pct = if total_resolved > 0 {
            Some(m.confirmed_fills as f64 / total_resolved as f64 * 100.0)
        } else {
            None
        };
        FailsafeMetricsSnapshot {
            trigger_count: m.trigger_count,
            check_count: m.check_count,
            max_observed_drift_bps: m.max_observed_drift_bps,
            confirmed_fills: m.confirmed_fills,
            no_fills: m.no_fills,
            stale_orders: m.stale_orders,
            confirmation_rate_pct,
            heartbeat_ok_count: m.heartbeat_ok_count,
            heartbeat_fail_count: m.heartbeat_fail_count,
            heartbeat_consecutive_fail_count: m.heartbeat_consecutive_fail_count,
            heartbeat_last_ok_ms: m.heartbeat_last_ok_ms,
        }
    }

    pub async fn pending_orders_count(&self) -> usize {
        self.pending_orders.lock().await.len()
    }

    pub fn canary_state_snapshot(&self) -> CanaryStateSnapshot {
        let state = self.canary_state.lock_or_recover();
        CanaryStateSnapshot {
            stage: self.canary_policy.stage,
            max_order_usdc: self.canary_policy.max_order_usdc,
            max_session_spend_usdc: self.canary_policy.max_session_spend_usdc,
            max_orders_per_session: self.canary_policy.max_orders_per_session,
            accepted_orders: state.accepted_orders,
            accepted_spend_usdc: state.accepted_spend_usdc,
            session_spend_remaining_usdc: (self.canary_policy.max_session_spend_usdc
                - state.accepted_spend_usdc)
                .max(0.0),
            reject_streak: state.reject_streak,
            loss_streak: state.loss_streak,
            halted: state.halted,
            last_accept_ms: state.last_accept_ms,
        }
    }

    /// Emergency stop: trips circuit breaker, cancels all open exchange orders,
    /// runs a final reconciliation pass, and writes an incident flag file.
    ///
    /// Call this when drift, auth failures, or any critical anomaly is detected.
    /// Trading will remain halted until the operator resets the circuit breaker.
    pub async fn emergency_stop(&self, reason: &str) {
        error!("🚨 EMERGENCY STOP triggered: {reason}");

        // 1. Trip circuit breaker — blocks all new orders immediately.
        self.risk.lock_or_recover().trip_circuit_breaker(reason);

        // 2. Cancel all open orders on the exchange.
        match self.executor.cancel_all_orders().await {
            Ok(()) => info!("Emergency stop: exchange cancel-all succeeded"),
            Err(e) => error!("Emergency stop: cancel_all_orders failed: {e}"),
        }

        // 3. Run reconciliation to sync any fills that arrived before cancel.
        self.run_reconciliation_pass().await;

        // 4. Write persistent incident flag for operator review.
        let pending = self.pending_orders.lock().await.len();
        let flag_content = format!(
            "reason={reason}\ntimestamp={}\npending_orders_after_cancel={pending}\n",
            chrono::Utc::now()
        );
        let _ = std::fs::create_dir_all("logs");
        let flag_path = Path::new("logs").join("EMERGENCY_STOP.flag");
        let _ = std::fs::write(&flag_path, &flag_content);

        if let Some(ref log) = self.activity {
            log_push(
                log,
                EntryKind::Warn,
                format!(
                    "🚨 EMERGENCY STOP: {reason} — circuit breaker tripped, all orders cancelled"
                ),
            );
        }

        error!(
            reason,
            pending_after_cancel = pending,
            "🚨 EMERGENCY STOP complete — trading halted. See logs/EMERGENCY_STOP.flag"
        );
    }

    /// Graceful shutdown sequence (SIGTERM / Ctrl-C path).
    ///
    /// Order of operations:
    /// 1. Trip circuit breaker — blocks any new order from being submitted.
    /// 2. Cancel all open exchange orders.
    /// 3. Run a final reconciliation pass — capture fills that arrived before
    ///    the cancel was processed.
    /// 4. Flush the WAL — persist the post-cancel pending-order state so that
    ///    a subsequent restart can reconcile any remainder.
    /// 5. Log final portfolio snapshot.
    ///
    /// Called with a 30-second timeout from `main`.  If it exceeds the timeout
    /// the engine exits anyway — the WAL ensures recovery on next start.
    pub async fn graceful_shutdown(&self) {
        info!("🛑 Live engine graceful shutdown initiated");

        if let Some(ref log) = self.activity {
            log_push(
                log,
                EntryKind::Engine,
                "GRACEFUL SHUTDOWN: starting (cancel + reconcile + WAL flush)".to_string(),
            );
        }

        // 1. Trip circuit breaker — no new orders during shutdown.
        self.risk
            .lock_or_recover()
            .trip_circuit_breaker("graceful_shutdown");

        // 2. Cancel all open exchange orders.
        if !self.executor.dry_run {
            match self.executor.cancel_all_orders().await {
                Ok(()) => info!("Graceful shutdown: exchange cancel-all succeeded"),
                Err(e) => {
                    warn!("Graceful shutdown: cancel_all_orders failed (will still reconcile): {e}")
                }
            }
        }

        // 3. Final reconciliation — capture fills that arrived before cancel.
        self.run_reconciliation_pass().await;

        // 4. Flush WAL with post-reconcile state.
        self.persist_wal().await;

        // 5. Log final state.
        let (nav, positions, pending) = {
            let p = self.portfolio.lock().await;
            (
                p.nav(),
                p.positions.len(),
                self.pending_orders.lock().await.len(),
            )
        };
        info!(
            nav = %format!("{:.2}", nav),
            positions,
            pending_orders = pending,
            "🛑 Live engine shutdown complete"
        );
        if let Some(ref log) = self.activity {
            log_push(
                log,
                EntryKind::Engine,
                format!(
                    "GRACEFUL SHUTDOWN: complete — NAV={:.2} positions={} pending={}",
                    nav, positions, pending
                ),
            );
        }
    }
}

fn classify_intent_from_counts(same_side: usize, opposite_side: usize) -> SignalIntent {
    if same_side == 0 && opposite_side == 0 {
        return SignalIntent::NewExposure;
    }
    if same_side > 0 && opposite_side == 0 {
        return SignalIntent::AddExposure;
    }
    if same_side == 0 && opposite_side > 0 {
        return SignalIntent::HedgeOrFlatten;
    }
    SignalIntent::Ambiguous
}

fn hour_in_window(hour: u8, start: u8, end: u8) -> bool {
    if start == end {
        return true;
    }
    if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

async fn fetch_wallet_positions_from_data_api(user: &str) -> Result<Vec<PaperPosition>> {
    let client = reqwest::Client::new();
    let attempts = wallet_truth_retries();
    let max_items = wallet_truth_max_items();
    let mut raw_positions = Vec::new();
    let mut seen = HashSet::new();

    for offset in (0..max_items).step_by(WALLET_TRUTH_PAGE_LIMIT) {
        let limit = WALLET_TRUTH_PAGE_LIMIT.min(max_items - offset);
        let page_positions =
            fetch_wallet_positions_page(&client, user, limit, offset, attempts).await?;
        let page_len = page_positions.len();
        let mut added = 0usize;

        for position in page_positions {
            if seen.insert(data_api_position_key(&position)) {
                raw_positions.push(position);
                added += 1;
            }
        }

        if page_len < limit || added == 0 {
            break;
        }
    }

    raw_positions
        .iter()
        .enumerate()
        .map(|(idx, value)| wallet_position_from_json(idx, value))
        .collect()
}

async fn hydrate_wallet_position_open_times_from_trades(
    user: &str,
    positions: &mut [PaperPosition],
) -> Result<()> {
    let asset_ids = positions
        .iter()
        .map(|pos| pos.token_id.clone())
        .collect::<HashSet<_>>();
    if asset_ids.is_empty() {
        return Ok(());
    }

    let trade_open_times = fetch_wallet_buy_trade_open_times(user, &asset_ids).await?;
    apply_wallet_position_open_times_from_trades(positions, &trade_open_times);
    Ok(())
}

async fn fetch_wallet_buy_trade_open_times(
    user: &str,
    asset_ids: &HashSet<String>,
) -> Result<HashMap<String, i64>> {
    let client = reqwest::Client::new();
    let attempts = wallet_truth_retries();
    let max_items = wallet_truth_trade_max_items();
    let mut open_times = HashMap::new();

    for offset in (0..max_items).step_by(WALLET_TRUTH_PAGE_LIMIT) {
        let limit = WALLET_TRUTH_PAGE_LIMIT.min(max_items - offset);
        let page_trades = fetch_wallet_trades_page(&client, user, limit, offset, attempts).await?;
        let page_len = page_trades.len();

        for trade in page_trades {
            let side = json_text(&trade, &["side"]).unwrap_or_default();
            if !side.eq_ignore_ascii_case("BUY") {
                continue;
            }
            let Some(asset) = json_text(&trade, &["asset", "token_id", "tokenId", "conditionId"])
            else {
                continue;
            };
            if !asset_ids.contains(&asset) || open_times.contains_key(&asset) {
                continue;
            }
            if let Some(timestamp) = json_timestamp_secs(&trade, &["timestamp", "time"]) {
                open_times.insert(asset, timestamp);
            }
        }

        if open_times.len() >= asset_ids.len() || page_len < limit {
            break;
        }
    }

    Ok(open_times)
}

fn apply_wallet_position_open_times_from_trades(
    positions: &mut [PaperPosition],
    open_times: &HashMap<String, i64>,
) {
    let now_wall = chrono::Local::now();
    let now_instant = Instant::now();

    for position in positions {
        let Some(timestamp) = open_times.get(&position.token_id).copied() else {
            continue;
        };
        let Some(opened_utc) = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0) else {
            continue;
        };
        let opened_wall = opened_utc.with_timezone(&chrono::Local);
        let age_secs = (now_wall - opened_wall).num_seconds().max(0) as u64;
        position.opened_at_wall = opened_wall;
        position.opened_at = now_instant
            .checked_sub(Duration::from_secs(age_secs))
            .unwrap_or(now_instant);
    }
}

fn wallet_truth_retries() -> usize {
    std::env::var("BLINK_WALLET_TRUTH_RETRIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(1, 5))
        .unwrap_or(2)
}

fn wallet_truth_max_items() -> usize {
    std::env::var("BLINK_WALLET_TRUTH_MAX_ITEMS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(WALLET_TRUTH_PAGE_LIMIT, 50_000))
        .unwrap_or(10_000)
}

fn wallet_truth_trade_max_items() -> usize {
    std::env::var("BLINK_WALLET_TRUTH_TRADE_MAX_ITEMS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(WALLET_TRUTH_PAGE_LIMIT, 50_000))
        .unwrap_or(5_000)
}

async fn fetch_wallet_positions_page(
    client: &reqwest::Client,
    user: &str,
    limit: usize,
    offset: usize,
    attempts: usize,
) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "https://data-api.polymarket.com/positions?user={user}&limit={limit}&offset={offset}"
    );
    let mut last_error = "Polymarket positions request failed".to_string();

    for attempt in 1..=attempts {
        let response =
            tokio::time::timeout(Duration::from_millis(2_000), client.get(&url).send()).await;

        match response {
            Ok(Ok(resp)) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(value) => {
                        return Ok(data_api_entries_from_body(value));
                    }
                    Err(e) => {
                        last_error = format!("Polymarket positions response was not JSON: {e}");
                        warn!(attempt, attempts, error = %e, "Wallet truth JSON parse failed");
                    }
                }
            }
            Ok(Ok(resp)) => {
                last_error = format!("Polymarket positions returned HTTP {}", resp.status());
                warn!(
                    attempt,
                    attempts,
                    status = %resp.status(),
                    "Wallet truth request returned non-success"
                );
            }
            Ok(Err(e)) => {
                last_error = format!("Polymarket positions request failed: {e}");
                warn!(attempt, attempts, error = %e, "Wallet truth request failed");
            }
            Err(_) => {
                last_error = "Polymarket positions request timed out".to_string();
                warn!(attempt, attempts, "Wallet truth request timed out");
            }
        }

        if attempt < attempts {
            sleep(Duration::from_millis(150)).await;
        }
    }

    bail!(last_error)
}

async fn fetch_wallet_trades_page(
    client: &reqwest::Client,
    user: &str,
    limit: usize,
    offset: usize,
    attempts: usize,
) -> Result<Vec<serde_json::Value>> {
    let url =
        format!("https://data-api.polymarket.com/trades?user={user}&limit={limit}&offset={offset}");
    let mut last_error = "Polymarket trades request failed".to_string();

    for attempt in 1..=attempts {
        let response = tokio::time::timeout(
            Duration::from_millis(2_000),
            client.get(&url).header("accept", "application/json").send(),
        )
        .await;

        match response {
            Ok(Ok(resp)) if resp.status().is_success() => {
                return resp
                    .json::<serde_json::Value>()
                    .await
                    .map(data_api_entries_from_body)
                    .with_context(|| "Polymarket trades response was not JSON");
            }
            Ok(Ok(resp)) => {
                last_error = format!("Polymarket trades returned HTTP {}", resp.status());
            }
            Ok(Err(e)) => {
                last_error = format!("Polymarket trades request failed: {e}");
            }
            Err(_) => {
                last_error = "Polymarket trades request timed out".to_string();
            }
        }

        if attempt < attempts {
            sleep(Duration::from_millis(150)).await;
        }
    }

    bail!(last_error)
}

fn data_api_entries_from_body(body: serde_json::Value) -> Vec<serde_json::Value> {
    if let Some(arr) = body.as_array() {
        arr.clone()
    } else {
        body.get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }
}

fn data_api_position_key(position: &serde_json::Value) -> String {
    let asset = json_text(position, &["asset", "token_id", "tokenId", "conditionId"])
        .unwrap_or_else(|| "no_asset".to_string());
    let outcome =
        json_text(position, &["outcome", "side"]).unwrap_or_else(|| "no_outcome".to_string());
    format!("{asset}:{outcome}")
}

fn wallet_position_from_json(index: usize, value: &serde_json::Value) -> Result<PaperPosition> {
    let token_id = json_text(value, &["asset", "token_id", "tokenId", "conditionId"])
        .with_context(|| format!("wallet position {index} missing asset token id"))?;
    let entry_price = json_f64(value, &["avgPrice", "avg_price", "averagePrice"]).unwrap_or(0.0);
    let current_price = json_f64(value, &["curPrice", "currentPrice", "price"])
        .unwrap_or(entry_price)
        .clamp(0.0, 1.0);
    let shares = json_f64(value, &["size", "tokens", "quantity"]).unwrap_or(0.0);
    let usdc_spent = json_f64(value, &["initialValue", "initial_value"])
        .filter(|v| v.is_finite() && *v > f64::EPSILON)
        .unwrap_or(shares * entry_price);
    let now_wall = chrono::Local::now();
    let event_start_time = json_timestamp_secs(
        value,
        &[
            "gameStartDate",
            "game_start_date",
            "gameStartTime",
            "game_start_time",
            "startDate",
            "start_date",
            "startDateIso",
            "start_date_iso",
        ],
    );
    let event_end_time = json_timestamp_secs(
        value,
        &[
            "endDate",
            "end_date",
            "endDateIso",
            "end_date_iso",
            "resolutionDate",
            "resolution_date",
        ],
    );

    Ok(PaperPosition {
        id: index + 1,
        token_id: token_id.clone(),
        market_title: json_text(value, &["title", "market", "eventTitle"]),
        market_outcome: json_text(value, &["outcome", "side"]),
        side: OrderSide::Buy,
        entry_price,
        shares,
        usdc_spent,
        entry_fee_paid_usdc: 0.0,
        current_price,
        peak_price: current_price.max(entry_price),
        fee_category: "exchange".to_string(),
        fee_rate: 0.0,
        opened_at: Instant::now(),
        rn1_order_id: format!("exchange:{token_id}"),
        opened_at_wall: now_wall,
        entry_slippage_bps: 0.0,
        queue_delay_ms: 0,
        experiment_variant: "exchange_truth".to_string(),
        event_start_time,
        event_end_time,
        momentum_ref_price: current_price,
        momentum_ref_ts: now_wall.timestamp(),
        last_claimed_tier_pct: 0.0,
        signal_source: "exchange".to_string(),
        analysis_id: None,
    })
}

fn preserve_wallet_position_lifecycle(
    wallet_positions: &mut [PaperPosition],
    local_positions: &[PaperPosition],
) {
    let local_by_token = local_positions
        .iter()
        .map(|pos| (pos.token_id.as_str(), pos))
        .collect::<HashMap<_, _>>();

    for wallet_pos in wallet_positions {
        let Some(local_pos) = local_by_token.get(wallet_pos.token_id.as_str()) else {
            continue;
        };

        wallet_pos.id = local_pos.id;
        if wallet_pos.market_title.is_none() {
            wallet_pos.market_title = local_pos.market_title.clone();
        }
        if wallet_pos.market_outcome.is_none() {
            wallet_pos.market_outcome = local_pos.market_outcome.clone();
        }

        wallet_pos.entry_fee_paid_usdc = local_pos.entry_fee_paid_usdc;
        wallet_pos.opened_at = local_pos.opened_at;
        wallet_pos.opened_at_wall = local_pos.opened_at_wall;
        wallet_pos.rn1_order_id = local_pos.rn1_order_id.clone();
        wallet_pos.entry_slippage_bps = local_pos.entry_slippage_bps;
        wallet_pos.queue_delay_ms = local_pos.queue_delay_ms;
        wallet_pos.experiment_variant = local_pos.experiment_variant.clone();
        wallet_pos.signal_source = local_pos.signal_source.clone();
        wallet_pos.analysis_id = local_pos.analysis_id.clone();

        if wallet_pos.fee_category == "exchange" && local_pos.fee_category != "exchange" {
            wallet_pos.fee_category = local_pos.fee_category.clone();
            wallet_pos.fee_rate = local_pos.fee_rate;
        } else if wallet_pos.fee_rate.abs() <= f64::EPSILON
            && local_pos.fee_rate.abs() > f64::EPSILON
        {
            wallet_pos.fee_rate = local_pos.fee_rate;
        }

        if wallet_pos.event_start_time.is_none() {
            wallet_pos.event_start_time = local_pos.event_start_time;
        }
        if wallet_pos.event_end_time.is_none() {
            wallet_pos.event_end_time = local_pos.event_end_time;
        }

        wallet_pos.momentum_ref_price = local_pos.momentum_ref_price;
        wallet_pos.momentum_ref_ts = local_pos.momentum_ref_ts;
        wallet_pos.last_claimed_tier_pct = local_pos.last_claimed_tier_pct;
        wallet_pos.peak_price = local_pos
            .peak_price
            .max(wallet_pos.peak_price)
            .max(wallet_pos.current_price)
            .max(wallet_pos.entry_price);
    }
}

fn json_f64(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))
    })
}

fn json_timestamp_secs(value: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let raw = value.get(*key)?;
        if let Some(ts) = raw.as_i64().and_then(normalize_epoch_secs) {
            return Some(ts);
        }
        if let Some(ts) = raw.as_f64().and_then(|v| normalize_epoch_secs(v as i64)) {
            return Some(ts);
        }
        raw.as_str().and_then(parse_timestamp_secs)
    })
}

fn normalize_epoch_secs(ts: i64) -> Option<i64> {
    if ts <= 0 {
        None
    } else if ts > 10_000_000_000 {
        Some(ts / 1_000)
    } else {
        Some(ts)
    }
}

fn parse_timestamp_secs(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    raw.parse::<i64>()
        .ok()
        .and_then(normalize_epoch_secs)
        .or_else(|| {
            raw.parse::<f64>()
                .ok()
                .and_then(|v| normalize_epoch_secs(v as i64))
        })
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(raw)
                .ok()
                .map(|dt| dt.timestamp())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|dt| dt.and_utc().timestamp())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|dt| dt.and_utc().timestamp())
        })
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()
                .map(|d| {
                    d.and_hms_opt(23, 59, 59)
                        .expect("infallible: 23:59:59 is always valid")
                        .and_utc()
                        .timestamp()
                })
        })
}

fn json_text(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    })
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn contra_depth_usdc(book: &crate::order_book::OrderBook, side: OrderSide, levels: usize) -> f64 {
    match side {
        OrderSide::Buy => book
            .asks
            .iter()
            .take(levels)
            .map(|(price, size)| (*price as f64 * *size as f64) / 1_000_000.0)
            .sum(),
        OrderSide::Sell => book
            .bids
            .iter()
            .rev()
            .take(levels)
            .map(|(price, size)| (*price as f64 * *size as f64) / 1_000_000.0)
            .sum(),
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1" || v.eq_ignore_ascii_case("yes"))
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn live_exit_max_order_usdc() -> f64 {
    std::env::var("BLINK_LIVE_EXIT_MAX_ORDER_USDC")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or_else(|| env_f64("MAX_SINGLE_ORDER_USDC", 1.0))
        .clamp(0.01, 500_000.0)
}

fn live_exit_min_order_usdc() -> f64 {
    std::env::var("BLINK_LIVE_EXIT_MIN_ORDER_USDC")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| {
            std::env::var("PAPER_MIN_TRADE_USDC")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
        })
        .unwrap_or(1.0)
        .clamp(0.0, 500_000.0)
}

fn live_exit_canary_static_block_reason(
    policy: &LiveExitCanaryPolicy,
    state: &LiveExitCanaryState,
    request: &LiveExitRequest,
    min_order_usdc: f64,
) -> Option<String> {
    if min_order_usdc > 0.0 && request.notional_usdc + f64::EPSILON < min_order_usdc {
        return Some("exit_notional_below_min_order".to_string());
    }

    if !policy.enabled {
        return None;
    }

    if state.queued_orders >= policy.max_orders_per_session {
        return Some("exit_canary_session_cap_reached".to_string());
    }
    if request.notional_usdc > policy.max_order_usdc + f64::EPSILON {
        return Some("exit_canary_notional_above_cap".to_string());
    }
    if policy.require_top_of_book && !request.top_of_book_confirmed {
        return Some("exit_top_of_book_unconfirmed".to_string());
    }

    None
}

fn pending_exit_tokens_from_pending_orders<'a>(
    orders: impl Iterator<Item = &'a PendingOrder>,
) -> HashSet<String> {
    orders
        .filter(|order| order.side == OrderSide::Sell && !order.is_terminal())
        .map(|order| order.token_id.clone())
        .collect()
}

fn parse_tick_size(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok()
}

fn parse_rest_book_levels(book: &serde_json::Value, side: &str) -> Vec<PriceLevel> {
    book.get(side)
        .and_then(|levels| levels.as_array())
        .map(|levels| {
            levels
                .iter()
                .filter_map(|level| {
                    let price = level.get("price").and_then(|v| v.as_str())?;
                    let size = level.get("size").and_then(|v| v.as_str())?;
                    let parsed = PriceLevel {
                        price: parse_price(price),
                        size: parse_price(size),
                    };
                    (parsed.price > 0 && parsed.size > 0).then_some(parsed)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn should_trip_startup_operator_guard(
    live_trading: bool,
    trading_enabled: bool,
    operator_token: Option<&str>,
) -> bool {
    live_trading && trading_enabled && operator_token.is_none_or(|token| token.trim().is_empty())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        apply_wallet_position_open_times_from_trades, classify_intent_from_counts,
        data_api_entries_from_body, data_api_position_key, hour_in_window,
        live_exit_canary_static_block_reason, pending_exit_tokens_from_pending_orders,
        preserve_wallet_position_lifecycle, should_trip_startup_operator_guard,
        wallet_position_from_json, LiveEngine, LiveExitCanaryPolicy, LiveExitCanaryState,
        LiveExitRequest, SignalIntent,
    };
    use crate::truth_reconciler::{FillLifecycle, PendingOrder};
    use crate::types::OrderSide;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "actual={actual} expected={expected}"
        );
    }

    #[test]
    fn startup_operator_guard_trips_only_for_live_enabled_without_token() {
        assert!(should_trip_startup_operator_guard(true, true, None));
        assert!(should_trip_startup_operator_guard(true, true, Some("  ")));
        assert!(!should_trip_startup_operator_guard(
            true,
            true,
            Some("secret")
        ));
        assert!(!should_trip_startup_operator_guard(true, false, None));
        assert!(!should_trip_startup_operator_guard(false, true, None));
    }

    fn test_exit_request(notional_usdc: f64) -> LiveExitRequest {
        LiveExitRequest {
            token_id: "asset-a".to_string(),
            position_id: 1,
            reason: "stop_loss".to_string(),
            shares: 2.0,
            price_u64: 500,
            notional_usdc,
            pnl_pct: -10.0,
            current_price: 0.50,
            top_of_book_confirmed: true,
        }
    }

    fn test_exit_canary_policy() -> LiveExitCanaryPolicy {
        LiveExitCanaryPolicy {
            enabled: true,
            max_orders_per_session: 1,
            max_order_usdc: 1.0,
            require_wallet_confirmation: true,
            require_top_of_book: true,
        }
    }

    #[test]
    fn live_exit_canary_blocks_after_session_cap() {
        let policy = test_exit_canary_policy();
        let state = LiveExitCanaryState {
            queued_orders: 1,
            ..Default::default()
        };
        let request = test_exit_request(1.0);

        assert_eq!(
            live_exit_canary_static_block_reason(&policy, &state, &request, 1.0).as_deref(),
            Some("exit_canary_session_cap_reached")
        );
    }

    #[test]
    fn live_exit_canary_requires_top_of_book_when_enabled() {
        let policy = test_exit_canary_policy();
        let state = LiveExitCanaryState::default();
        let mut request = test_exit_request(1.0);
        request.top_of_book_confirmed = false;

        assert_eq!(
            live_exit_canary_static_block_reason(&policy, &state, &request, 1.0).as_deref(),
            Some("exit_top_of_book_unconfirmed")
        );
    }

    #[test]
    fn live_exit_canary_blocks_below_min_order_even_when_canary_disabled() {
        let mut policy = test_exit_canary_policy();
        policy.enabled = false;
        let state = LiveExitCanaryState::default();
        let request = test_exit_request(0.5);

        assert_eq!(
            live_exit_canary_static_block_reason(&policy, &state, &request, 1.0).as_deref(),
            Some("exit_notional_below_min_order")
        );
    }

    #[test]
    fn pending_exit_tokens_only_include_non_terminal_sell_orders() {
        let buy = PendingOrder::new(
            "buy-order".to_string(),
            "buy-token".to_string(),
            OrderSide::Buy,
            1.0,
            0.5,
        );
        let active_sell = PendingOrder::new(
            "sell-order".to_string(),
            "sell-token".to_string(),
            OrderSide::Sell,
            1.0,
            0.5,
        );
        let mut terminal_sell = PendingOrder::new(
            "done-sell-order".to_string(),
            "done-sell-token".to_string(),
            OrderSide::Sell,
            1.0,
            0.5,
        );
        terminal_sell.lifecycle = FillLifecycle::NoFill {
            reason: "cancelled".to_string(),
        };
        let orders = [buy, active_sell, terminal_sell];

        let tokens = pending_exit_tokens_from_pending_orders(orders.iter());

        assert_eq!(tokens.len(), 1);
        assert!(tokens.contains("sell-token"));
    }

    #[test]
    fn budget_compatible_buy_price_never_returns_one_dollar() {
        assert_eq!(
            LiveEngine::best_budget_compatible_buy_price(970, 1050, 1_000_000),
            None
        );
    }

    #[test]
    fn budget_compatible_buy_price_accepts_valid_price_under_one_dollar() {
        assert_eq!(
            LiveEngine::best_budget_compatible_buy_price(800, 900, 1_000_000),
            Some(800)
        );
    }

    #[test]
    fn budget_compatible_buy_price_rejects_precision_min_above_cash() {
        assert_eq!(
            LiveEngine::best_budget_compatible_buy_price(859, 859, 1_075_998),
            None
        );
        assert_eq!(LiveEngine::min_valid_buy_maker_amount_base(859), 8_590_000);
    }

    #[test]
    fn classify_intent_new_exposure() {
        assert_eq!(classify_intent_from_counts(0, 0), SignalIntent::NewExposure);
    }

    #[test]
    fn classify_intent_add_exposure() {
        assert_eq!(classify_intent_from_counts(2, 0), SignalIntent::AddExposure);
    }

    #[test]
    fn classify_intent_hedge_or_flatten() {
        assert_eq!(
            classify_intent_from_counts(0, 1),
            SignalIntent::HedgeOrFlatten
        );
    }

    #[test]
    fn classify_intent_ambiguous() {
        assert_eq!(classify_intent_from_counts(1, 1), SignalIntent::Ambiguous);
    }

    #[test]
    fn daytime_window_non_wrapping() {
        assert!(hour_in_window(10, 8, 22));
        assert!(!hour_in_window(23, 8, 22));
    }

    #[test]
    fn daytime_window_wrapping() {
        assert!(hour_in_window(23, 22, 6));
        assert!(hour_in_window(2, 22, 6));
        assert!(!hour_in_window(12, 22, 6));
    }

    #[test]
    fn wallet_position_from_json_maps_data_api_position_to_internal_ledger() {
        let raw = json!({
            "asset": "108335214097330660216497436528140920329790228410878622712875555123360135252984",
            "conditionId": "0xf8b61bb1849d27296b9413e471bace0b49f53f87e51aea01b7ea545df52e4302",
            "size": 3.125,
            "avgPrice": 0.32,
            "initialValue": 1,
            "currentValue": 0.9531,
            "cashPnl": -0.0469,
            "percentPnl": -4.6899,
            "curPrice": 0.305,
            "title": "Club Atletico de Madrid vs. Arsenal FC: O/U 1.5",
            "outcome": "Under"
        });

        let position = wallet_position_from_json(0, &raw).expect("position should parse");

        assert_eq!(
            position.token_id,
            "108335214097330660216497436528140920329790228410878622712875555123360135252984"
        );
        assert_eq!(
            position.market_title.as_deref(),
            Some("Club Atletico de Madrid vs. Arsenal FC: O/U 1.5")
        );
        assert_eq!(position.market_outcome.as_deref(), Some("Under"));
        assert_close(position.shares, 3.125);
        assert_close(position.entry_price, 0.32);
        assert_close(position.current_price, 0.305);
        assert_close(position.usdc_spent, 1.0);
        assert_eq!(position.signal_source, "exchange");
        assert_eq!(position.experiment_variant, "exchange_truth");
    }

    #[test]
    fn wallet_position_from_json_falls_back_when_initial_value_is_zero() {
        let raw = json!({
            "asset": "asset-a",
            "size": 2.0,
            "avgPrice": 0.50,
            "initialValue": 0.0,
            "curPrice": 0.55
        });

        let position = wallet_position_from_json(0, &raw).expect("position should parse");

        assert_close(position.usdc_spent, 1.0);
        assert_close(position.unrealized_pnl_pct(), 10.0);
    }

    #[test]
    fn wallet_position_from_json_maps_date_only_end_date_to_event_end_time() {
        let raw = json!({
            "asset": "asset-a",
            "size": 2.0,
            "avgPrice": 0.50,
            "initialValue": 1.0,
            "curPrice": 0.55,
            "endDate": "2026-05-10"
        });

        let position = wallet_position_from_json(0, &raw).expect("position should parse");

        assert_eq!(position.event_end_time, Some(1_778_457_599));
    }

    #[test]
    fn preserve_wallet_position_lifecycle_keeps_exit_state_on_truth_refresh() {
        let mut local = wallet_position_from_json(
            0,
            &json!({
                "asset": "asset-a",
                "size": 2.0,
                "avgPrice": 0.50,
                "initialValue": 1.0,
                "curPrice": 0.60
            }),
        )
        .expect("local position should parse");
        let opened_at_wall = chrono::Local::now() - chrono::Duration::seconds(600);
        local.id = 7;
        local.opened_at = Instant::now() - Duration::from_secs(600);
        local.opened_at_wall = opened_at_wall;
        local.peak_price = 0.72;
        local.momentum_ref_price = 0.61;
        local.momentum_ref_ts = 111;
        local.last_claimed_tier_pct = 10.0;
        local.rn1_order_id = "rn1-order".to_string();
        local.experiment_variant = "rn1_live".to_string();
        local.signal_source = "rn1".to_string();
        local.event_end_time = Some(1_778_457_599);

        let mut refreshed = vec![wallet_position_from_json(
            0,
            &json!({
                "asset": "asset-a",
                "size": 2.5,
                "avgPrice": 0.52,
                "initialValue": 1.3,
                "curPrice": 0.65
            }),
        )
        .expect("wallet position should parse")];

        preserve_wallet_position_lifecycle(&mut refreshed, &[local]);
        let position = &refreshed[0];

        assert_eq!(position.id, 7);
        assert_eq!(position.opened_at_wall, opened_at_wall);
        assert!(position.opened_at.elapsed() >= Duration::from_secs(590));
        assert_close(position.shares, 2.5);
        assert_close(position.entry_price, 0.52);
        assert_close(position.usdc_spent, 1.3);
        assert_close(position.current_price, 0.65);
        assert_close(position.peak_price, 0.72);
        assert_close(position.momentum_ref_price, 0.61);
        assert_eq!(position.momentum_ref_ts, 111);
        assert_close(position.last_claimed_tier_pct, 10.0);
        assert_eq!(position.rn1_order_id, "rn1-order");
        assert_eq!(position.experiment_variant, "rn1_live");
        assert_eq!(position.signal_source, "rn1");
        assert_eq!(position.event_end_time, Some(1_778_457_599));
    }

    #[test]
    fn apply_wallet_position_open_times_from_trades_sets_opened_age() {
        let mut positions = vec![wallet_position_from_json(
            0,
            &json!({
                "asset": "asset-a",
                "size": 2.0,
                "avgPrice": 0.50,
                "initialValue": 1.0,
                "curPrice": 0.55
            }),
        )
        .expect("position should parse")];
        let opened_ts = chrono::Utc::now().timestamp() - 120;
        let mut open_times = HashMap::new();
        open_times.insert("asset-a".to_string(), opened_ts);

        apply_wallet_position_open_times_from_trades(&mut positions, &open_times);
        let position = &positions[0];

        assert!(position.opened_at.elapsed() >= Duration::from_secs(110));
        assert!((position.opened_at_wall.timestamp() - opened_ts).abs() <= 1);
    }

    #[test]
    fn wallet_truth_data_api_helpers_parse_wrappers_and_keys() {
        let direct = data_api_entries_from_body(json!([{ "asset": "direct" }]));
        let wrapped = data_api_entries_from_body(json!({ "data": [{ "asset": "wrapped" }] }));
        let position = json!({
            "tokenId": "asset-a",
            "outcome": "Under"
        });

        assert_eq!(direct[0]["asset"], "direct");
        assert_eq!(wrapped[0]["asset"], "wrapped");
        assert_eq!(data_api_position_key(&position), "asset-a:Under");
    }
}

// ─── Phase 5: maker-layering integration (feature-gated) ─────────────────────

/// Spawn the periodic (250 ms) maker-layering maintenance task.
///
/// Activated only under the `maker-layering` cargo feature — which will be
/// wired to `ExecutionProfile::HftMaker` once that profile lands. Until then
/// the body is dormant code: it owns a `MakerLayerEngine` and exercises the
/// reprice/metrics paths, but has no market source to plan fresh layers
/// against. `plan_layers` + submit wiring happens here once an upstream
/// supplier of `(market_id, token_id, mid)` is in place.
#[cfg(feature = "maker-layering")]
pub fn spawn_maker_layering_task(
    risk_gate: std::sync::Arc<crate::risk_manager::StreamRiskGate>,
    _executor: std::sync::Arc<crate::order_executor::OrderExecutor>,
    submit_tx: tokio::sync::mpsc::Sender<crate::order_router::OrderIntent>,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use crate::hot_metrics::counters;
    use crate::maker_layering::MakerLayerEngine;
    use crate::risk_manager::AdmitDecision;

    let engine = Arc::new(MakerLayerEngine::default());

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            ticker.tick().await;

            // Collect market keys without holding the DashMap shard lock across awaits.
            let markets: Vec<String> = engine.per_market.iter().map(|e| e.key().clone()).collect();

            for market_id in &markets {
                // TODO: once ExecutionProfile::HftMaker exposes a market
                // universe + mid oracle, call `engine.plan_layers(...)` here
                // and forward admitted intents to `submit_tx`. The
                // try_admit/submit path below exercises the wiring so the
                // integration is exactly where HftMaker activation will
                // plug in.
                let pending: Vec<OrderIntent> = Vec::new();
                for intent in pending {
                    counters()
                        .maker_layers_planned_total
                        .fetch_add(1, Ordering::Relaxed);
                    match risk_gate.try_admit(&intent) {
                        AdmitDecision::Admit => {
                            if submit_tx.try_send(intent).is_ok() {
                                counters()
                                    .maker_layers_placed_total
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        AdmitDecision::Throttle { .. } | AdmitDecision::Reject { .. } => {
                            // Skip this layer; will be re-planned on the next tick.
                        }
                    }
                }

                // Reprice stale / drifted layers. Cancel path: the reconciler
                // owns cancel submission (via OrderRouter::cancel_order);
                // here we just drop the tracked entry and bump metrics.
                // mid=0 short-circuits drift check — age eviction still fires.
                let evictions = engine.reprice_stale(market_id, 0, 50);
                for (_intent_id, reason) in evictions {
                    counters()
                        .maker_layers_reprice_total
                        .fetch_add(1, Ordering::Relaxed);
                    if matches!(reason, crate::maker_layering::RepriceReason::StaleAge) {
                        counters()
                            .maker_layers_stale_evictions_total
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    // TODO: call OrderRouter::cancel_order via a shared handle
                    // once HftMaker activation lands.
                }
            }

            counters()
                .maker_active_layers
                .store(engine.max_active_layers(), Ordering::Relaxed);
        }
    });
}
