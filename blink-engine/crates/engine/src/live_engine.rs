use anyhow::{Result, bail};
use std::sync::Arc;
use std::collections::HashMap;
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;
use tokio::time::sleep;
use chrono::Timelike;
use tracing::{info, warn, error};

use crate::activity_log::{ActivityLog, EntryKind, push as log_push};
use crate::config::Config;
use crate::mev_router::MevRouter;
use crate::order_book::OrderBookStore;
use crate::order_executor::OrderExecutor;
use crate::order_signer::{sign_order_with_vault_policy, OrderParams, OrderSigningPolicy};
use crate::paper_portfolio::{drift_threshold, PaperPortfolio, STARTING_BALANCE_USDC};
use crate::risk_manager::{RiskConfig, RiskManager};
use crate::truth_reconciler::{PendingOrder, PendingOrderWal, process_order_status, ReconciliationOutcome};
use crate::types::{OrderSide, RN1Signal, TimeInForce};

pub struct LiveEngine {
    pub portfolio: Arc<Mutex<PaperPortfolio>>,
    book_store: Arc<OrderBookStore>,
    activity: Option<ActivityLog>,
    pub executor: OrderExecutor,
    vault: Option<Arc<tee_vault::VaultHandle>>,
    funder_addr: String,
    pub risk: Arc<std::sync::Mutex<RiskManager>>,
    #[allow(dead_code)]
    mev_router: Option<std::sync::Mutex<MevRouter>>,
    accounted_closed_trades: Mutex<usize>,
    signing_policy: OrderSigningPolicy,
    nonce_counter: AtomicU64,
    /// Live orders submitted to the exchange that have not yet been reconciled.
    /// Fill recording is deferred until the reconciliation worker confirms the
    /// actual fill amounts via `GET /order/{id}`.
    pending_orders: Mutex<HashMap<String, PendingOrder>>,
    reconcile_interval: Duration,
    pub failsafe_metrics: std::sync::Mutex<FailsafeMetrics>,
    canary_policy: CanaryPolicy,
    canary_state: std::sync::Mutex<CanaryState>,
    /// Path to the pending-orders WAL file. Written atomically after every
    /// insert/remove so that crash recovery can reconcile against the exchange.
    wal_path: String,
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
    max_orders_per_session: usize,
    daytime_only: bool,
    start_hour_utc: u8,
    end_hour_utc: u8,
    max_reject_streak: usize,
    allowed_markets: Vec<String>,
}

#[derive(Debug, Default)]
struct CanaryState {
    accepted_orders: usize,
    reject_streak: usize,
    halted: bool,
}

impl LiveEngine {
    pub fn new(config: Arc<Config>, book_store: Arc<OrderBookStore>, activity: Option<ActivityLog>) -> Result<Self> {
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
        let risk = Arc::new(std::sync::Mutex::new(RiskManager::new(RiskConfig::from_env())));
        let signing_policy = OrderSigningPolicy {
            expiration: config.polymarket_order_expiration,
            nonce: config.polymarket_order_nonce,
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
            max_orders_per_session: config.live_canary_max_orders_per_session,
            daytime_only: config.live_canary_daytime_only,
            start_hour_utc: config.live_canary_start_hour_utc,
            end_hour_utc: config.live_canary_end_hour_utc,
            max_reject_streak: config.live_canary_max_reject_streak,
            allowed_markets: config.live_canary_allowed_markets.clone(),
        };

        if let Some(ref log) = activity {
            log_push(
                log,
                EntryKind::Engine,
                format!(
                    "LiveEngine started — live={} vault={} dry_run={}",
                    config.live_trading, vault.is_some(), !config.live_trading
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
            portfolio: Arc::new(Mutex::new(PaperPortfolio::new())),
            book_store,
            activity,
            executor,
            vault,
            funder_addr,
            risk,
            mev_router: mev,
            accounted_closed_trades: Mutex::new(0),
            signing_policy,
            nonce_counter: AtomicU64::new(config.polymarket_order_nonce),
            pending_orders: Mutex::new(HashMap::new()),
            reconcile_interval: Duration::from_secs(reconcile_interval_secs),
            failsafe_metrics: std::sync::Mutex::new(FailsafeMetrics::default()),
            canary_policy,
            canary_state: std::sync::Mutex::new(CanaryState::default()),
            wal_path: std::env::var("PENDING_ORDERS_WAL_PATH")
                .unwrap_or_else(|_| "logs/live_pending_orders_wal.json".to_string()),
        })
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
            Err(e) => { error!("WAL serialize failed: {e}"); return; }
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
            Ok(_) => return 0,  // empty file
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
            log_push(log, EntryKind::Warn,
                format!("WAL RECOVERY: {} pending orders from previous session — reconciling", wal_entries.len()));
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
        info!(recovered, remaining, "WAL startup reconciliation complete");
        recovered
    }

    pub fn spawn_reconciliation_worker(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                sleep(self.reconcile_interval).await;
                self.run_reconciliation_pass().await;
            }
        });
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
        self.risk.lock().unwrap_or_else(|e| e.into_inner()).status_line()
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
                flushed,
                "🧹 Pre-game order wipe: flushed {flushed} positions for {token_id}"
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
        self.sync_risk_closes_from_portfolio().await;
        self.run_reconciliation_pass().await;

        let intent = self.classify_signal_intent(&signal).await;
        if matches!(intent, SignalIntent::HedgeOrFlatten | SignalIntent::Ambiguous) {
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
                    format!("INTENT-SKIP {} token={} side={}", reason, signal.token_id, signal.side),
                );
            }
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            p.skipped_orders += 1;
            return;
        }

        // 1. Calculate entry_price, rn1_shares, rn1_notional_usd
        let entry_price = signal.price as f64 / 1_000.0;
        let rn1_shares = signal.size as f64 / 1_000.0;
        let rn1_notional_usd = rn1_shares * entry_price;

        // 2. Size the order — brief lock on portfolio
        let (size_usdc, current_nav, open_positions) = {
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            let size = p.calculate_size_usdc(rn1_notional_usd);
            let nav = p.nav();
            let open = p.positions.len();
            (size, nav, open)
        };

        // 3. Risk check — BEFORE doing anything real
        let size_usdc = match size_usdc {
            Some(s) => s,
            None => {
                // Skip like PaperEngine
                return;
            }
        };

        if let Err(reason) = self.check_canary_gate(&signal, size_usdc) {
            warn!(token_id = %signal.token_id, side = %signal.side, reason, "Canary gate blocked order");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("CANARY-BLOCKED: {reason}"));
            }
            let mut p = self.portfolio.lock().await;
            p.skipped_orders += 1;
            return;
        }

        if let Err(violation) = self.risk.lock().unwrap_or_else(|e| e.into_inner()).check_pre_order(
            size_usdc,
            open_positions,
            current_nav,
            STARTING_BALANCE_USDC,
        ) {
            warn!("🛑 Risk check blocked order: {violation}");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("BLOCKED: {violation}"));
            }
            return;
        }

        // 4. Fill window check — same as PaperEngine
        let filled = self
            .check_fill_window(&signal.token_id, entry_price, signal.side)
            .await;
        if !filled {
            // Abort like PaperEngine
            return;
        }

        // 5. Build and sign (or dry-run)
        let params = OrderParams {
            token_id: signal.token_id.clone(),
            side: signal.side,
            price: signal.price,
            size: size_usdc,
            maker: self.funder_addr.clone(),
        };

        let (accepted, exchange_order_id) = if self.executor.dry_run || self.vault.is_none() {
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
            (true, None)
        } else {
            let vault = self.vault.as_ref().expect("vault is Some; guarded by is_none() check above");
            let mut policy = self.signing_policy;
            policy.nonce = self.nonce_counter.fetch_add(1, Ordering::Relaxed);
            match sign_order_with_vault_policy(vault.as_ref(), &params, policy) {
                Ok(signed) => {
                    match self.executor.submit_order(&signed, TimeInForce::Gtc).await {
                        Ok(resp) => {
                            if resp.success {
                                info!(order_id = ?resp.order_id, "✅ LIVE order submitted");
                                if let Some(ref log) = self.activity {
                                    log_push(
                                        log,
                                        EntryKind::Fill,
                                        format!(
                                            "LIVE SUBMITTED {} @{:.3} ${:.2} id={:?}",
                                            signal.side, entry_price, size_usdc, resp.order_id
                                        ),
                                    );
                                }
                                (true, resp.order_id.clone())
                            } else {
                                error!(error = ?resp.error_msg, "❌ LIVE order rejected");
                                (false, None)
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "❌ LIVE submit failed");
                            (false, None)
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "❌ EIP-712 signing failed");
                    (false, None)
                }
            }
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
                    format!("LIVE REJECTED {} @{:.3} ${:.2}", signal.side, entry_price, size_usdc),
                );
            }
            return;
        }

        self.record_canary_accept();

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
        } else {
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
            self.risk.lock().unwrap_or_else(|e| e.into_inner()).record_fill(size_usdc);
        }
    }

    fn check_canary_gate(&self, signal: &RN1Signal, size_usdc: f64) -> Result<(), String> {
        let state = self.canary_state.lock().unwrap_or_else(|e| e.into_inner());
        if state.halted {
            return Err("canary_halted_after_reject_streak".to_string());
        }
        if size_usdc > self.canary_policy.max_order_usdc {
            return Err(format!(
                "size_usdc_exceeds_canary_limit {:.2}>{:.2}",
                size_usdc, self.canary_policy.max_order_usdc
            ));
        }
        if self.canary_policy.max_orders_per_session > 0
            && state.accepted_orders >= self.canary_policy.max_orders_per_session
        {
            return Err("session_order_cap_reached".to_string());
        }
        if self.canary_policy.daytime_only {
            let hour = chrono::Utc::now().hour() as u8;
            if !hour_in_window(hour, self.canary_policy.start_hour_utc, self.canary_policy.end_hour_utc) {
                return Err(format!(
                    "outside_daytime_window hour={} window={}..{}",
                    hour, self.canary_policy.start_hour_utc, self.canary_policy.end_hour_utc
                ));
            }
        }
        if !self.canary_policy.allowed_markets.is_empty()
            && !self.canary_policy.allowed_markets.iter().any(|m| m == &signal.token_id)
        {
            return Err("token_not_in_canary_allowlist".to_string());
        }
        Ok(())
    }

    fn bump_reject_streak(&self) {
        let mut state = self.canary_state.lock().unwrap_or_else(|e| e.into_inner());
        state.reject_streak += 1;
        if state.reject_streak >= self.canary_policy.max_reject_streak {
            state.halted = true;
            let _ = std::fs::write(
                "logs\\CANARY_HALTED.flag",
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
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!(
                        "CANARY HALTED: reject_streak={} threshold={} (logs\\CANARY_HALTED.flag)",
                        state.reject_streak, self.canary_policy.max_reject_streak
                    ),
                );
            }
        }
    }

    fn record_canary_accept(&self) {
        let mut state = self.canary_state.lock().unwrap_or_else(|e| e.into_inner());
        state.accepted_orders += 1;
        state.reject_streak = 0;
    }

    async fn classify_signal_intent(&self, signal: &RN1Signal) -> SignalIntent {
        let (same_side, opposite_side) = {
            let p = self.portfolio.lock().await;
            let mut same = 0usize;
            let mut opposite = 0usize;
            for pos in p.positions.iter().filter(|pos| pos.token_id == signal.token_id) {
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
        let (new_count, realized_delta) = {
            let p = self.portfolio.lock().await;
            if *accounted >= p.closed_trades.len() {
                return;
            }
            let delta = p.closed_trades[*accounted..]
                .iter()
                .map(|t| t.realized_pnl)
                .sum::<f64>();
            (p.closed_trades.len(), delta)
        };

        self.risk.lock().unwrap_or_else(|e| e.into_inner()).record_close(realized_delta);
        *accounted = new_count;
    }

    async fn run_reconciliation_pass(&self) {
        self.sync_risk_closes_from_portfolio().await;

        let pending_ids: Vec<String> =
            self.pending_orders.lock().await.keys().cloned().collect();

        let mut resolved       = 0usize;
        let mut fills_recorded = 0usize;

        for order_id in pending_ids {
            // Fetch latest status from exchange.
            let status = match self.executor.get_order_status(&order_id).await {
                Ok(s)  => s,
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
                    None          => continue, // removed by concurrent path
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
                    self.risk.lock().unwrap_or_else(|e| e.into_inner()).record_fill(actual_size_usdc);
                    fills_recorded += 1;
                    resolved       += 1;
                    self.failsafe_metrics.lock().unwrap_or_else(|e| e.into_inner()).confirmed_fills += 1;

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
                    self.failsafe_metrics.lock().unwrap_or_else(|e| e.into_inner()).no_fills += 1;
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
                    if let Some(ref log) = self.activity {
                        log_push(
                            log,
                            EntryKind::Warn,
                            format!("STALE ORDER {order_id} pending {elapsed_secs}s — operator review required"),
                        );
                    }
                    self.failsafe_metrics.lock().unwrap_or_else(|e| e.into_inner()).stale_orders += 1;
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
            info!(resolved, fills_recorded, pending, "Reconciliation pass completed");
        }
    }

    async fn check_fill_window(&self, token_id: &str, entry_price: f64, _side: OrderSide) -> bool {
        for check in 0_u8..6 {
            sleep(Duration::from_millis(500)).await;
            if let Some(current) = self.get_market_price(token_id) {
                let drift = (current - entry_price).abs() / entry_price;
                {
                    let mut metrics = self.failsafe_metrics.lock().unwrap_or_else(|e| e.into_inner());
                    metrics.check_count += 1;
                    let drift_bps = (drift * 10_000.0).round().max(0.0) as u64;
                    if drift_bps > metrics.max_observed_drift_bps {
                        metrics.max_observed_drift_bps = drift_bps;
                    }
                }
                if drift > drift_threshold() {
                    self.failsafe_metrics.lock().unwrap_or_else(|e| e.into_inner()).trigger_count += 1;
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

    fn get_market_price(&self, token_id: &str) -> Option<f64> {
        self.book_store
            .get_mid_price(token_id)
            .map(|p| p as f64 / 1_000.0)
    }

    pub fn failsafe_metrics_snapshot(&self) -> FailsafeMetricsSnapshot {
        let m = self.failsafe_metrics.lock().unwrap_or_else(|e| e.into_inner());
        let total_resolved = m.confirmed_fills + m.no_fills;
        let confirmation_rate_pct = if total_resolved > 0 {
            Some(m.confirmed_fills as f64 / total_resolved as f64 * 100.0)
        } else {
            None
        };
        FailsafeMetricsSnapshot {
            trigger_count:          m.trigger_count,
            check_count:            m.check_count,
            max_observed_drift_bps: m.max_observed_drift_bps,
            confirmed_fills:        m.confirmed_fills,
            no_fills:               m.no_fills,
            stale_orders:           m.stale_orders,
            confirmation_rate_pct,
            heartbeat_ok_count:     m.heartbeat_ok_count,
            heartbeat_fail_count:   m.heartbeat_fail_count,
        }
    }

    pub async fn pending_orders_count(&self) -> usize {
        self.pending_orders.lock().await.len()
    }

    /// Emergency stop: trips circuit breaker, cancels all open exchange orders,
    /// runs a final reconciliation pass, and writes an incident flag file.
    ///
    /// Call this when drift, auth failures, or any critical anomaly is detected.
    /// Trading will remain halted until the operator resets the circuit breaker.
    pub async fn emergency_stop(&self, reason: &str) {
        error!("🚨 EMERGENCY STOP triggered: {reason}");

        // 1. Trip circuit breaker — blocks all new orders immediately.
        self.risk.lock().unwrap_or_else(|e| e.into_inner()).trip_circuit_breaker(reason);

        // 2. Cancel all open orders on the exchange.
        match self.executor.cancel_all_orders().await {
            Ok(())  => info!("Emergency stop: exchange cancel-all succeeded"),
            Err(e)  => error!("Emergency stop: cancel_all_orders failed: {e}"),
        }

        // 3. Run reconciliation to sync any fills that arrived before cancel.
        self.run_reconciliation_pass().await;

        // 4. Write persistent incident flag for operator review.
        let pending = self.pending_orders.lock().await.len();
        let flag_content = format!(
            "reason={reason}\ntimestamp={}\npending_orders_after_cancel={pending}\n",
            chrono::Utc::now()
        );
        let _ = std::fs::write("logs\\EMERGENCY_STOP.flag", &flag_content);

        if let Some(ref log) = self.activity {
            log_push(
                log,
                EntryKind::Warn,
                format!("🚨 EMERGENCY STOP: {reason} — circuit breaker tripped, all orders cancelled"),
            );
        }

        error!(
            reason,
            pending_after_cancel = pending,
            "🚨 EMERGENCY STOP complete — trading halted. See logs/EMERGENCY_STOP.flag"
        );
    }

    /// Graceful shutdown: log and persist state.
    pub async fn graceful_shutdown(&self) {
        info!("Live engine graceful shutdown initiated");
        // Save portfolio state
        let p = self.portfolio.lock().await;
        let nav = p.nav();
        let positions = p.positions.len();
        drop(p);
        info!(nav = %format!("{:.2}", nav), positions, "Live engine shutdown — final state");
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

#[cfg(test)]
mod tests {
    use super::{classify_intent_from_counts, hour_in_window, SignalIntent};

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
        assert_eq!(classify_intent_from_counts(0, 1), SignalIntent::HedgeOrFlatten);
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
}
