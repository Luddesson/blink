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
use crate::paper_portfolio::{DRIFT_THRESHOLD, PaperPortfolio, STARTING_BALANCE_USDC};
use crate::risk_manager::{RiskConfig, RiskManager};
use crate::types::{OrderSide, RN1Signal, TimeInForce};

pub struct LiveEngine {
    pub portfolio: Arc<Mutex<PaperPortfolio>>,
    book_store: Arc<OrderBookStore>,
    activity: Option<ActivityLog>,
    executor: OrderExecutor,
    vault: Option<Arc<tee_vault::VaultHandle>>,
    funder_addr: String,
    pub risk: Arc<std::sync::Mutex<RiskManager>>,
    #[allow(dead_code)]
    mev_router: Option<std::sync::Mutex<MevRouter>>,
    accounted_closed_trades: Mutex<usize>,
    signing_policy: OrderSigningPolicy,
    nonce_counter: AtomicU64,
    pending_orders: Mutex<HashMap<String, PendingOrderInfo>>,
    reconcile_interval: Duration,
    failsafe_metrics: std::sync::Mutex<FailsafeMetrics>,
    canary_policy: CanaryPolicy,
    canary_state: std::sync::Mutex<CanaryState>,
}

#[derive(Debug, Clone)]
struct PendingOrderInfo {
    token_id: String,
    side: OrderSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalIntent {
    NewExposure,
    AddExposure,
    HedgeOrFlatten,
    Ambiguous,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FailsafeMetricsSnapshot {
    pub trigger_count: u64,
    pub check_count: u64,
    pub max_observed_drift_bps: u64,
}

#[derive(Debug, Default)]
struct FailsafeMetrics {
    trigger_count: u64,
    check_count: u64,
    max_observed_drift_bps: u64,
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
    pub fn new(config: Arc<Config>, book_store: Arc<OrderBookStore>, activity: Option<ActivityLog>) -> Self {
        let executor = OrderExecutor::from_config(&config);

        // Initialize the TEE vault for key isolation.
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

        Self {
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
        }
    }

    pub fn spawn_reconciliation_worker(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                sleep(self.reconcile_interval).await;
                self.run_reconciliation_pass().await;
            }
        });
    }

    pub fn risk_status(&self) -> String {
        self.risk.lock().unwrap().status_line()
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

        if let Err(violation) = self.risk.lock().unwrap().check_pre_order(
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
            let vault = self.vault.as_ref().unwrap();
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

        // 6. Record virtual fill only when order path was accepted.
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

        if let Some(order_id) = exchange_order_id {
            self.pending_orders.lock().await.insert(
                order_id,
                PendingOrderInfo {
                    token_id: signal.token_id.clone(),
                    side: signal.side,
                },
            );
        }

        // Update risk manager
        self.risk.lock().unwrap().record_fill(size_usdc);
    }

    fn check_canary_gate(&self, signal: &RN1Signal, size_usdc: f64) -> Result<(), String> {
        let state = self.canary_state.lock().unwrap();
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
        let mut state = self.canary_state.lock().unwrap();
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
        let mut state = self.canary_state.lock().unwrap();
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

        self.risk.lock().unwrap().record_close(realized_delta);
        *accounted = new_count;
    }

    async fn run_reconciliation_pass(&self) {
        self.sync_risk_closes_from_portfolio().await;

        let pending_ids: Vec<String> = self.pending_orders.lock().await.keys().cloned().collect();
        let mut resolved = 0usize;
        for order_id in pending_ids {
            let status = match self.executor.get_order_status(&order_id).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(order_id = %order_id, error = %e, "Reconciliation status fetch failed");
                    continue;
                }
            };
            let state = status.status.to_ascii_lowercase();
            let terminal = matches!(
                state.as_str(),
                "matched" | "filled" | "cancelled" | "canceled" | "rejected" | "expired"
            );
            if terminal {
                if let Some(meta) = self.pending_orders.lock().await.remove(&order_id) {
                    resolved += 1;
                    info!(
                        order_id = %order_id,
                        token_id = %meta.token_id,
                        side = %meta.side,
                        status = %state,
                        "Reconciliation resolved live order"
                    );
                }
            }
        }
        if resolved > 0 {
            let pending = self.pending_orders.lock().await.len();
            info!(resolved, pending, "Reconciliation pass completed");
        }
    }

    async fn check_fill_window(&self, token_id: &str, entry_price: f64, _side: OrderSide) -> bool {
        for check in 0_u8..6 {
            sleep(Duration::from_millis(500)).await;
            if let Some(current) = self.get_market_price(token_id) {
                let drift = (current - entry_price).abs() / entry_price;
                {
                    let mut metrics = self.failsafe_metrics.lock().unwrap();
                    metrics.check_count += 1;
                    let drift_bps = (drift * 10_000.0).round().max(0.0) as u64;
                    if drift_bps > metrics.max_observed_drift_bps {
                        metrics.max_observed_drift_bps = drift_bps;
                    }
                }
                if drift > DRIFT_THRESHOLD {
                    self.failsafe_metrics.lock().unwrap().trigger_count += 1;
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
        let m = self.failsafe_metrics.lock().unwrap();
        FailsafeMetricsSnapshot {
            trigger_count: m.trigger_count,
            check_count: m.check_count,
            max_observed_drift_bps: m.max_observed_drift_bps,
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
