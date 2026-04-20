//! Risk management module — enforces hard financial limits.
//!
//! [`RiskManager::check_pre_order`] must be called before every order
//! submission.  It returns `Ok(())` only when all safety checks pass.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::warn;

// ─── RiskConfig ───────────────────────────────────────────────────────────────

/// Static configuration for the risk manager.
#[derive(Debug, Clone)]
pub struct RiskConfig {
    /// Maximum daily loss as fraction of starting NAV (default 0.10 = 10%).
    pub max_daily_loss_pct: f64,
    /// Maximum simultaneous open positions (default 5).
    pub max_concurrent_positions: usize,
    /// Maximum USDC per single order (default $20).
    pub max_single_order_usdc: f64,
    /// Maximum orders per second (default 3, CLOB rate limit safety).
    pub max_orders_per_second: u32,
    /// Hard kill switch — when false, ALL order submission is blocked.
    pub trading_enabled: bool,
    /// Rolling VaR window duration (default 60 seconds).
    pub var_window: Duration,
    /// VaR circuit-breaker threshold — if outstanding exposure in the rolling
    /// window exceeds this fraction of portfolio NAV, trip the breaker.
    /// Default 0.05 = 5%.
    pub var_threshold_pct: f64,

    // ── Phase 3: HFT stream-aware risk fields ────────────────────────────

    /// Steady-state order submission rate (orders/second) admitted by the
    /// token-bucket gate. Default 50.0.
    pub orders_per_second: f64,
    /// Burst bucket size — max tokens refill can accumulate. Default 150.
    pub orders_burst: u32,
    /// Steady-state cancel/replace budget (operations/second). Default 30.0.
    pub cancel_replace_budget_per_sec: f64,
    /// Max concurrent pending orders per market. 0 disables the check.
    /// Default 6.
    pub per_market_max_pending: u32,
    /// Max notional exposure per market (USDC × 1_000 scaled). 0 disables.
    /// Default 500_000 (= $500).
    pub per_market_max_notional_usdc: u64,
    /// Account-wide max pending notional (USDC × 1_000 scaled). 0 disables.
    /// Default 5_000_000 (= $5000).
    pub account_max_pending_notional_usdc: u64,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_daily_loss_pct: 1.0,
            max_concurrent_positions: 20,
            max_single_order_usdc: 25.0,
            max_orders_per_second: 10,
            trading_enabled: false,
            var_window: Duration::from_secs(60),
            var_threshold_pct: 0.05,
            orders_per_second: 50.0,
            orders_burst: 150,
            cancel_replace_budget_per_sec: 30.0,
            per_market_max_pending: 6,
            per_market_max_notional_usdc: 500_000,
            account_max_pending_notional_usdc: 5_000_000,
        }
    }
}

impl RiskConfig {
    /// Loads risk configuration from environment variables, falling back to
    /// safe defaults for any variable that is missing or unparseable.
    ///
    /// | Variable                  | Default |
    /// |---------------------------|---------|
    /// | `MAX_DAILY_LOSS_PCT`      | 1.0 (100% — no limit in paper mode) |
    /// | `MAX_CONCURRENT_POSITIONS`| 20      |
    /// | `MAX_SINGLE_ORDER_USDC`   | 25.0    |
    /// | `MAX_ORDERS_PER_SECOND`   | 10      |
    /// | `TRADING_ENABLED`         | false   |
    pub fn from_env() -> Self {
        let max_daily_loss_pct = std::env::var("MAX_DAILY_LOSS_PCT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(1.0);

        let max_concurrent_positions = std::env::var("MAX_CONCURRENT_POSITIONS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20);

        let max_single_order_usdc = std::env::var("MAX_SINGLE_ORDER_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(25.0);

        let max_orders_per_second = std::env::var("MAX_ORDERS_PER_SECOND")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(10);

        if std::env::var("MAX_ORDERS_PER_SECOND").is_ok() {
            warn!(
                "MAX_ORDERS_PER_SECOND is deprecated — use BLINK_RISK_ORDERS_PER_SEC \
                 and BLINK_RISK_ORDERS_BURST for the Phase 3 token-bucket gate"
            );
        }

        let orders_per_second = std::env::var("BLINK_RISK_ORDERS_PER_SEC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(50.0);
        let orders_burst = std::env::var("BLINK_RISK_ORDERS_BURST")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(150);
        let cancel_replace_budget_per_sec = std::env::var("BLINK_RISK_CANCEL_REPLACE_PER_SEC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(30.0);
        let per_market_max_pending = std::env::var("BLINK_RISK_PER_MARKET_PENDING")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(6);
        let per_market_max_notional_usdc = std::env::var("BLINK_RISK_PER_MARKET_NOTIONAL_USDC")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(500_000);
        let account_max_pending_notional_usdc = std::env::var("BLINK_RISK_ACCOUNT_NOTIONAL_USDC")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5_000_000);

        // Default to false — must be explicitly opted in.
        let trading_enabled = std::env::var("TRADING_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        Self {
            max_daily_loss_pct,
            max_concurrent_positions,
            max_single_order_usdc,
            max_orders_per_second,
            trading_enabled,
            var_window: Duration::from_secs(
                std::env::var("VAR_WINDOW_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60),
            ),
            var_threshold_pct: std::env::var("VAR_THRESHOLD_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0), // 100% — effectively disabled for paper mode
            orders_per_second,
            orders_burst,
            cancel_replace_budget_per_sec,
            per_market_max_pending,
            per_market_max_notional_usdc,
            account_max_pending_notional_usdc,
        }
    }
}

// ─── RiskViolation ────────────────────────────────────────────────────────────

/// Describes why an order was rejected by the risk manager.
#[derive(Debug, Clone)]
pub enum RiskViolation {
    /// The global kill switch (`trading_enabled`) is set to false.
    KillSwitchOff,
    /// The circuit breaker was manually tripped.
    CircuitBreakerTripped { reason: String, tripped_at: Instant },
    /// Cumulative daily losses have exceeded the configured limit.
    DailyLossLimitExceeded { loss_usdc: f64, limit_usdc: f64 },
    /// Too many concurrent open positions.
    TooManyPositions { current: usize, max: usize },
    /// Single order size exceeds the configured cap.
    OrderTooLarge { size_usdc: f64, max_usdc: f64 },
    /// Order rate exceeds the per-second limit.
    RateLimitExceeded { orders_in_window: u32, max: u32 },
    /// Rolling 60-second Value-at-Risk exceeds the configured threshold.
    VarBreached {
        exposure_usdc: f64,
        threshold_usdc: f64,
        nav: f64,
    },
}

impl fmt::Display for RiskViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskViolation::KillSwitchOff => {
                write!(f, "RISK: Trading disabled (kill switch is OFF)")
            }
            RiskViolation::CircuitBreakerTripped { reason, tripped_at } => {
                let secs = tripped_at.elapsed().as_secs();
                write!(
                    f,
                    "RISK: Circuit breaker tripped {secs}s ago — reason: {reason}"
                )
            }
            RiskViolation::DailyLossLimitExceeded {
                loss_usdc,
                limit_usdc,
            } => {
                write!(
                    f,
                    "RISK: Daily loss limit exceeded (loss=${loss_usdc:.2}, limit=${limit_usdc:.2})"
                )
            }
            RiskViolation::TooManyPositions { current, max } => {
                write!(f, "RISK: Too many open positions ({current}/{max})")
            }
            RiskViolation::OrderTooLarge {
                size_usdc,
                max_usdc,
            } => {
                write!(
                    f,
                    "RISK: Order too large (${size_usdc:.2} > max ${max_usdc:.2})"
                )
            }
            RiskViolation::RateLimitExceeded {
                orders_in_window,
                max,
            } => {
                write!(
                    f,
                    "RISK: Rate limit exceeded ({orders_in_window}/{max} orders/sec)"
                )
            }
            RiskViolation::VarBreached {
                exposure_usdc,
                threshold_usdc,
                nav,
            } => {
                write!(
                    f,
                    "RISK: VaR breached — rolling exposure ${exposure_usdc:.2} > {:.1}% of NAV ${nav:.2} (threshold ${threshold_usdc:.2})",
                    (threshold_usdc / nav) * 100.0
                )
            }
        }
    }
}

impl RiskViolation {
    /// Stable analytics key used by rejection tracking and dashboards.
    pub fn analytics_key(&self) -> &'static str {
        match self {
            RiskViolation::KillSwitchOff => "risk_kill_switch_off",
            RiskViolation::CircuitBreakerTripped { .. } => "risk_circuit_breaker",
            RiskViolation::DailyLossLimitExceeded { .. } => "risk_daily_loss_limit",
            RiskViolation::TooManyPositions { .. } => "risk_too_many_positions",
            RiskViolation::OrderTooLarge { .. } => "risk_order_too_large",
            RiskViolation::RateLimitExceeded { .. } => "risk_rate_limit",
            RiskViolation::VarBreached { .. } => "risk_var_breached",
        }
    }
}

// ─── Phase 3: StreamRiskGate (lock-free token-bucket admission) ──────────────

/// Admission decision returned by [`StreamRiskGate::try_admit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitDecision {
    /// Order admitted — 1_000 rate tokens consumed.
    Admit,
    /// Bucket empty — caller should back off for `retry_in` then retry once.
    Throttle { retry_in: Duration },
    /// Hard reject — a structural limit was exceeded.
    Reject { reason: &'static str },
}

/// Per-market mutable risk state tracked by [`StreamRiskGate`].
#[derive(Debug, Default)]
pub struct MarketRiskState {
    pub pending_count: AtomicU32,
    /// Pending notional in USDC × 1_000.
    pub pending_notional_usdc: AtomicU64,
}

/// Lock-free token-bucket admission gate shared between `RiskManager` and
/// the `OrderRouter`.
///
/// **Hot path is integer-only.** All floats from [`RiskConfig`] are converted
/// to `u64` in [`StreamRiskGate::new`]; [`StreamRiskGate::try_admit`] performs
/// only integer arithmetic. Tokens are scaled by 1_000 (one order = 1_000
/// tokens) so the refill interval (10 ms) can deposit fractional-order
/// increments without loss of precision.
pub struct StreamRiskGate {
    // ── Immutable parameters (set at init) ─────────────────────────────────
    /// Full bucket capacity (orders_burst × 1_000).
    max_tokens: u64,
    /// Tokens added every 10 ms ((orders_per_second × 10.0) as u64).
    refill_per_10ms: u64,
    /// Full cancel/replace bucket capacity.
    max_cancel_tokens: u64,
    /// Cancel tokens added every 10 ms.
    cancel_refill_per_10ms: u64,
    /// Single-order USDC cap (× 1_000). 0 disables.
    max_single_order_u64: u64,
    /// Max pending orders per market. 0 disables.
    per_market_max_pending: u32,
    /// Per-market pending notional cap (× 1_000). 0 disables.
    per_market_max_notional_usdc: u64,
    /// Account-wide pending notional cap (× 1_000). 0 disables.
    account_max_pending_notional_usdc: u64,

    // ── Mutable atomic state ───────────────────────────────────────────────
    pub tokens: AtomicU64,
    pub cancel_tokens: AtomicU64,
    /// Per-market pending counters — updated by router lifecycle hooks.
    pub market_state: DashMap<String, MarketRiskState>,
}

impl StreamRiskGate {
    /// Build a new gate from a risk config. All float fields are converted to
    /// `u64` here so the hot path stays integer-only.
    pub fn new(config: &RiskConfig) -> Arc<Self> {
        let max_tokens = (config.orders_burst as u64).saturating_mul(1_000);
        let refill_per_10ms = (config.orders_per_second * 10.0).max(0.0) as u64;
        let max_cancel_tokens =
            ((config.cancel_replace_budget_per_sec * 3.0).max(0.0) as u64).saturating_mul(1_000);
        let cancel_refill_per_10ms = (config.cancel_replace_budget_per_sec * 10.0).max(0.0) as u64;
        let max_single_order_u64 = (config.max_single_order_usdc.max(0.0) * 1_000.0) as u64;

        Arc::new(Self {
            max_tokens,
            refill_per_10ms,
            max_cancel_tokens,
            cancel_refill_per_10ms,
            max_single_order_u64,
            per_market_max_pending: config.per_market_max_pending,
            per_market_max_notional_usdc: config.per_market_max_notional_usdc,
            account_max_pending_notional_usdc: config.account_max_pending_notional_usdc,
            tokens: AtomicU64::new(max_tokens),
            cancel_tokens: AtomicU64::new(max_cancel_tokens),
            market_state: DashMap::new(),
        })
    }

    /// Integer-only admission check. Must be called on the submit hot path
    /// BEFORE enqueueing to the router.
    pub fn try_admit(
        &self,
        intent: &crate::order_router::intent::OrderIntent,
    ) -> AdmitDecision {
        // Step 1: single-order size cap.
        if self.max_single_order_u64 > 0 && intent.size_u64 > self.max_single_order_u64 {
            return AdmitDecision::Reject {
                reason: "max_single_order",
            };
        }

        // Step 2: token-bucket CAS loop.
        loop {
            let current = self.tokens.load(Ordering::Acquire);
            if current < 1_000 {
                let refill = self.refill_per_10ms.max(1);
                let deficit = 1_000u64.saturating_sub(current);
                let ticks = (deficit + refill - 1) / refill;
                return AdmitDecision::Throttle {
                    retry_in: Duration::from_millis(ticks.saturating_mul(10).saturating_add(1)),
                };
            }
            if self
                .tokens
                .compare_exchange(current, current - 1_000, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }

        // Step 3: per-market pending count.
        if !intent.market_id.is_empty() && self.per_market_max_pending > 0 {
            let entry = self
                .market_state
                .entry(intent.market_id.clone())
                .or_default();
            let pending = entry.pending_count.load(Ordering::Relaxed);
            if pending >= self.per_market_max_pending {
                drop(entry);
                self.tokens.fetch_add(1_000, Ordering::Relaxed);
                return AdmitDecision::Reject {
                    reason: "pending_count",
                };
            }
        }

        // Step 4: per-market pending notional.
        if !intent.market_id.is_empty() && self.per_market_max_notional_usdc > 0 {
            let entry = self
                .market_state
                .entry(intent.market_id.clone())
                .or_default();
            let existing = entry.pending_notional_usdc.load(Ordering::Relaxed);
            if existing.saturating_add(intent.size_u64) > self.per_market_max_notional_usdc {
                drop(entry);
                self.tokens.fetch_add(1_000, Ordering::Relaxed);
                return AdmitDecision::Reject {
                    reason: "market_notional",
                };
            }
        }

        // Step 5: account-wide pending notional (sum across markets).
        if self.account_max_pending_notional_usdc > 0 {
            let mut sum: u64 = 0;
            for m in self.market_state.iter() {
                sum = sum.saturating_add(m.value().pending_notional_usdc.load(Ordering::Relaxed));
            }
            if sum.saturating_add(intent.size_u64) > self.account_max_pending_notional_usdc {
                self.tokens.fetch_add(1_000, Ordering::Relaxed);
                return AdmitDecision::Reject {
                    reason: "account_notional",
                };
            }
        }

        AdmitDecision::Admit
    }

    /// Try to admit a cancel/replace operation against the dedicated budget.
    pub fn try_admit_cancel(&self) -> AdmitDecision {
        loop {
            let current = self.cancel_tokens.load(Ordering::Acquire);
            if current < 1_000 {
                let refill = self.cancel_refill_per_10ms.max(1);
                let deficit = 1_000u64.saturating_sub(current);
                let ticks = (deficit + refill - 1) / refill;
                return AdmitDecision::Throttle {
                    retry_in: Duration::from_millis(ticks.saturating_mul(10).saturating_add(1)),
                };
            }
            if self
                .cancel_tokens
                .compare_exchange(current, current - 1_000, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return AdmitDecision::Admit;
            }
        }
    }

    /// Called by the router dispatcher the moment an order enters the store.
    pub fn on_order_created(&self, market_id: &str, size_u64: u64) {
        if market_id.is_empty() {
            return;
        }
        let entry = self.market_state.entry(market_id.to_string()).or_default();
        entry.pending_count.fetch_add(1, Ordering::Relaxed);
        entry
            .pending_notional_usdc
            .fetch_add(size_u64, Ordering::Relaxed);
    }

    /// Called by the router when an order reaches a terminal state
    /// (`Acked`, `Rejected`, `SubmitUnknown`, filled/cancelled, etc.).
    pub fn on_order_terminal(&self, market_id: &str, size_u64: u64) {
        if market_id.is_empty() {
            return;
        }
        if let Some(entry) = self.market_state.get(market_id) {
            let _ = entry
                .pending_count
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    Some(n.saturating_sub(1))
                });
            let _ = entry.pending_notional_usdc.fetch_update(
                Ordering::AcqRel,
                Ordering::Acquire,
                |n| Some(n.saturating_sub(size_u64)),
            );
        }
    }

    /// Current rate-bucket token level (× 1_000).
    pub fn available_tokens(&self) -> u64 {
        self.tokens.load(Ordering::Relaxed)
    }

    /// Current cancel-bucket token level (× 1_000).
    pub fn available_cancel_tokens(&self) -> u64 {
        self.cancel_tokens.load(Ordering::Relaxed)
    }

    /// Max pending count observed across all known markets.
    pub fn max_per_market_pending_count(&self) -> u64 {
        let mut max: u64 = 0;
        for m in self.market_state.iter() {
            let v = m.value().pending_count.load(Ordering::Relaxed) as u64;
            if v > max {
                max = v;
            }
        }
        max
    }

    /// Spawn the 10 ms refill task. Safe to call once per gate instance.
    pub fn spawn_token_refill(gate: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let refill = gate.refill_per_10ms;
            let cancel_refill = gate.cancel_refill_per_10ms;
            let max_tokens = gate.max_tokens;
            let max_cancel_tokens = gate.max_cancel_tokens;
            loop {
                interval.tick().await;
                if refill > 0 && max_tokens > 0 {
                    let _ = gate
                        .tokens
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |t| {
                            Some(t.saturating_add(refill).min(max_tokens))
                        });
                }
                if cancel_refill > 0 && max_cancel_tokens > 0 {
                    let _ = gate.cancel_tokens.fetch_update(
                        Ordering::AcqRel,
                        Ordering::Acquire,
                        |t| Some(t.saturating_add(cancel_refill).min(max_cancel_tokens)),
                    );
                }
            }
        });
    }
}

// ─── RiskManager ─────────────────────────────────────────────────────────────

/// Runtime risk manager — holds mutable state for daily P&L and rate limiting.
pub struct RiskManager {
    config: RiskConfig,
    /// USDC P&L accumulated today (negative = loss).
    daily_pnl: f64,
    /// Timestamps of recent order submissions (for rate limiting).
    recent_orders: VecDeque<Instant>,
    /// Circuit breaker tripped at this time (`None` = not tripped).
    circuit_breaker_tripped_at: Option<Instant>,
    /// Human-readable reason the circuit breaker was tripped.
    circuit_breaker_reason: String,
    /// Rolling exposure entries for VaR calculation.
    rolling_exposure: VecDeque<ExposureEntry>,
    /// Phase 3: shared lock-free token-bucket admission gate. Also cloned
    /// into the `OrderRouter` at startup so the submit hot path can call
    /// [`StreamRiskGate::try_admit`] without acquiring a lock.
    pub gate: Arc<StreamRiskGate>,
}

/// A single exposure entry in the rolling VaR window.
#[derive(Debug, Clone, Copy)]
struct ExposureEntry {
    /// When the exposure was recorded.
    timestamp: Instant,
    /// USDC amount of the order.
    amount_usdc: f64,
}

impl RiskManager {
    /// Creates a new `RiskManager` with the given configuration.
    pub fn new(config: RiskConfig) -> Self {
        let gate = StreamRiskGate::new(&config);
        Self {
            config,
            daily_pnl: 0.0,
            recent_orders: VecDeque::new(),
            circuit_breaker_tripped_at: None,
            circuit_breaker_reason: String::new(),
            rolling_exposure: VecDeque::new(),
            gate,
        }
    }

    // ── Core pre-order check ─────────────────────────────────────────────

    /// Returns `Ok(())` if all risk checks pass; otherwise returns the first
    /// `Err(RiskViolation)` encountered.
    ///
    /// On success, the current timestamp is pushed into the rate-limit buffer.
    pub fn check_pre_order(
        &mut self,
        size_usdc: f64,
        open_positions: usize,
        _current_nav: f64,
        starting_nav: f64,
    ) -> Result<(), RiskViolation> {
        // 1. Kill switch
        if !self.config.trading_enabled {
            return Err(RiskViolation::KillSwitchOff);
        }

        // 2. Circuit breaker — auto-reset if the VaR-based trip has expired.
        if let Some(tripped_at) = self.circuit_breaker_tripped_at {
            if self.circuit_breaker_reason.starts_with("VaR breached") {
                let now = Instant::now();
                self.evict_expired_exposure(now);
                let rolling_sum: f64 = self.rolling_exposure.iter().map(|e| e.amount_usdc).sum();
                // Use current NAV so threshold scales with growth/losses.
                let effective_nav = if _current_nav > 0.0 {
                    _current_nav
                } else {
                    starting_nav
                };
                let threshold_usdc = effective_nav * self.config.var_threshold_pct;
                if rolling_sum + size_usdc <= threshold_usdc {
                    // Exposure decayed below threshold — auto-reset.
                    self.circuit_breaker_tripped_at = None;
                    self.circuit_breaker_reason.clear();
                    tracing::info!(
                        rolling_usdc = %format!("{rolling_sum:.2}"),
                        threshold_usdc = %format!("{threshold_usdc:.2}"),
                        "🟢 VaR circuit breaker auto-reset — exposure decayed"
                    );
                    // Fall through to normal checks below.
                } else {
                    return Err(RiskViolation::CircuitBreakerTripped {
                        reason: self.circuit_breaker_reason.clone(),
                        tripped_at,
                    });
                }
            } else {
                return Err(RiskViolation::CircuitBreakerTripped {
                    reason: self.circuit_breaker_reason.clone(),
                    tripped_at,
                });
            }
        }

        // 3. Daily loss limit
        let limit_usdc = starting_nav * self.config.max_daily_loss_pct;
        if self.daily_pnl < -limit_usdc {
            // Auto-trip the circuit breaker to prevent further orders.
            self.circuit_breaker_tripped_at = Some(Instant::now());
            self.circuit_breaker_reason =
                format!("Daily loss limit exceeded (${:.2})", -self.daily_pnl);
            return Err(RiskViolation::DailyLossLimitExceeded {
                loss_usdc: -self.daily_pnl,
                limit_usdc,
            });
        }

        // 4. Concurrent positions (0 = unlimited)
        if self.config.max_concurrent_positions > 0
            && open_positions >= self.config.max_concurrent_positions
        {
            return Err(RiskViolation::TooManyPositions {
                current: open_positions,
                max: self.config.max_concurrent_positions,
            });
        }

        // 5. Single-order size cap
        if size_usdc > self.config.max_single_order_usdc {
            return Err(RiskViolation::OrderTooLarge {
                size_usdc,
                max_usdc: self.config.max_single_order_usdc,
            });
        }

        // 6. Rate limit — evict timestamps older than 1 second.
        let now = Instant::now();
        let window = Duration::from_secs(1);
        while self
            .recent_orders
            .front()
            .map(|t| now.duration_since(*t) >= window)
            .unwrap_or(false)
        {
            self.recent_orders.pop_front();
        }
        let orders_in_window = self.recent_orders.len() as u32;
        if orders_in_window >= self.config.max_orders_per_second {
            return Err(RiskViolation::RateLimitExceeded {
                orders_in_window,
                max: self.config.max_orders_per_second,
            });
        }

        // 7. Dynamic VaR — rolling exposure check.
        self.evict_expired_exposure(now);
        let rolling_sum: f64 = self.rolling_exposure.iter().map(|e| e.amount_usdc).sum();
        let pending_exposure = rolling_sum + size_usdc;
        // Use current NAV so threshold scales with growth/losses, not fixed at start.
        let effective_nav = if _current_nav > 0.0 {
            _current_nav
        } else {
            starting_nav
        };
        let threshold_usdc = effective_nav * self.config.var_threshold_pct;
        if pending_exposure > threshold_usdc {
            // Auto-trip circuit breaker on VaR breach.
            self.circuit_breaker_tripped_at = Some(now);
            self.circuit_breaker_reason = format!(
                "VaR breached: rolling ${:.2} > {:.1}% of NAV ${:.2}",
                pending_exposure,
                self.config.var_threshold_pct * 100.0,
                effective_nav
            );
            return Err(RiskViolation::VarBreached {
                exposure_usdc: pending_exposure,
                threshold_usdc,
                nav: effective_nav,
            });
        }

        // 8. All checks passed — record this order's timestamp.
        self.recent_orders.push_back(now);
        Ok(())
    }

    // ── P&L tracking ─────────────────────────────────────────────────────

    /// Called after an order fills — records exposure for VaR tracking.
    /// Does NOT affect daily P&L; use [`record_close`] with realized P&L
    /// when a position is closed.
    pub fn record_fill(&mut self, usdc_spent: f64) {
        self.rolling_exposure.push_back(ExposureEntry {
            timestamp: Instant::now(),
            amount_usdc: usdc_spent,
        });
    }

    /// Called when a position closes — adds realized P&L to the daily tracker.
    pub fn record_close(&mut self, realized_pnl: f64) {
        self.daily_pnl += realized_pnl;
    }

    // ── VaR helpers ──────────────────────────────────────────────────────

    /// Evicts exposure entries older than the rolling window.
    fn evict_expired_exposure(&mut self, now: Instant) {
        while self
            .rolling_exposure
            .front()
            .map(|e| now.duration_since(e.timestamp) >= self.config.var_window)
            .unwrap_or(false)
        {
            self.rolling_exposure.pop_front();
        }
    }

    /// Returns the current rolling exposure sum (USDC).
    pub fn rolling_exposure_usdc(&mut self) -> f64 {
        self.evict_expired_exposure(Instant::now());
        self.rolling_exposure.iter().map(|e| e.amount_usdc).sum()
    }

    // ── Circuit breaker ───────────────────────────────────────────────────

    /// Trips the circuit breaker, blocking all future orders until reset.
    pub fn trip_circuit_breaker(&mut self, reason: &str) {
        self.circuit_breaker_tripped_at = Some(Instant::now());
        self.circuit_breaker_reason = reason.to_string();
    }

    // ── Daily reset ───────────────────────────────────────────────────────

    /// Resets daily P&L tracking and clears the rate-limit buffer.
    /// Call this at midnight.
    pub fn reset_daily(&mut self) {
        self.daily_pnl = 0.0;
        self.recent_orders.clear();
        self.rolling_exposure.clear();
        // NOTE: deliberately does NOT clear a tripped circuit breaker —
        // that requires an explicit operator action via `trip_circuit_breaker`
        // reset by reinitialising the manager.
    }

    // ── Status helpers ────────────────────────────────────────────────────

    /// Returns `true` if trading is currently blocked for any reason.
    pub fn is_blocked(&self) -> bool {
        !self.config.trading_enabled || self.circuit_breaker_tripped_at.is_some()
    }

    /// One-line human-readable status string, suitable for TUI display.
    pub fn status_line(&self) -> String {
        if !self.config.trading_enabled {
            return "⛔ KILL SWITCH OFF".to_string();
        }
        if let Some(tripped_at) = self.circuit_breaker_tripped_at {
            let secs = tripped_at.elapsed().as_secs();
            return format!(
                "🔴 CIRCUIT BREAKER [{secs}s] — {}",
                self.circuit_breaker_reason
            );
        }
        let pnl_sign = if self.daily_pnl >= 0.0 { "+" } else { "" };
        let exposure: f64 = self.rolling_exposure.iter().map(|e| e.amount_usdc).sum();
        format!(
            "✅ OK | Daily P&L: {pnl_sign}{:.2} USDC | VaR exposure: ${:.2}",
            self.daily_pnl, exposure
        )
    }

    /// Returns a reference to the current configuration.
    pub fn config(&self) -> &RiskConfig {
        &self.config
    }

    /// Returns a mutable reference to the configuration for runtime editing.
    pub fn config_mut(&mut self) -> &mut RiskConfig {
        &mut self.config
    }

    /// Resets (clears) the circuit breaker, allowing trading to resume.
    pub fn reset_circuit_breaker(&mut self) {
        self.circuit_breaker_tripped_at = None;
        self.circuit_breaker_reason.clear();
    }

    /// Returns the reason the circuit breaker was tripped, or empty string if not tripped.
    pub fn circuit_breaker_reason(&self) -> &str {
        &self.circuit_breaker_reason
    }

    /// Returns `true` if the circuit breaker is currently tripped.
    pub fn is_circuit_breaker_tripped(&self) -> bool {
        self.circuit_breaker_tripped_at.is_some()
    }

    /// Returns the current daily P&L (negative = loss).
    pub fn daily_pnl(&self) -> f64 {
        self.daily_pnl
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_rm() -> RiskManager {
        RiskManager::new(RiskConfig {
            trading_enabled: true,
            max_single_order_usdc: 20.0,
            var_threshold_pct: 1.0, // permissive for unit tests
            ..RiskConfig::default()
        })
    }

    #[test]
    fn ok_when_all_checks_pass() {
        let mut rm = default_rm();
        assert!(rm.check_pre_order(10.0, 2, 100.0, 100.0).is_ok());
    }

    #[test]
    fn kill_switch_off_blocks() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: false,
            ..RiskConfig::default()
        });
        let err = rm.check_pre_order(10.0, 0, 100.0, 100.0).unwrap_err();
        assert!(matches!(err, RiskViolation::KillSwitchOff));
    }

    #[test]
    fn circuit_breaker_blocks_after_trip() {
        let mut rm = default_rm();
        rm.trip_circuit_breaker("manual test");
        let err = rm.check_pre_order(10.0, 0, 100.0, 100.0).unwrap_err();
        assert!(matches!(err, RiskViolation::CircuitBreakerTripped { .. }));
    }

    #[test]
    fn daily_loss_limit_blocks_and_trips_breaker() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            max_daily_loss_pct: 0.10, // 10% for this test
            max_single_order_usdc: 20.0,
            var_threshold_pct: 1.0,
            ..RiskConfig::default()
        });
        // Simulate $11 loss on a $100 NAV with 10% limit → limit = $10.
        rm.daily_pnl = -11.0;
        let err = rm.check_pre_order(1.0, 0, 89.0, 100.0).unwrap_err();
        assert!(
            matches!(err, RiskViolation::DailyLossLimitExceeded { .. }),
            "expected DailyLossLimitExceeded, got {err:?}"
        );
        // Circuit breaker should now be tripped as a side-effect.
        assert!(rm.circuit_breaker_tripped_at.is_some());
    }

    #[test]
    fn too_many_positions_blocks() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            max_concurrent_positions: 5,
            max_single_order_usdc: 20.0,
            var_threshold_pct: 1.0,
            ..RiskConfig::default()
        });
        let err = rm.check_pre_order(5.0, 5, 100.0, 100.0).unwrap_err();
        assert!(matches!(
            err,
            RiskViolation::TooManyPositions { current: 5, max: 5 }
        ));
    }

    #[test]
    fn order_too_large_blocks() {
        let mut rm = default_rm(); // max = $20
        let err = rm.check_pre_order(25.0, 0, 100.0, 100.0).unwrap_err();
        assert!(matches!(err, RiskViolation::OrderTooLarge { .. }));
    }

    #[test]
    fn analytics_keys_are_stable() {
        let cases = [
            (RiskViolation::KillSwitchOff, "risk_kill_switch_off"),
            (
                RiskViolation::CircuitBreakerTripped {
                    reason: "manual".to_string(),
                    tripped_at: Instant::now(),
                },
                "risk_circuit_breaker",
            ),
            (
                RiskViolation::DailyLossLimitExceeded {
                    loss_usdc: 12.0,
                    limit_usdc: 10.0,
                },
                "risk_daily_loss_limit",
            ),
            (
                RiskViolation::TooManyPositions { current: 5, max: 4 },
                "risk_too_many_positions",
            ),
            (
                RiskViolation::OrderTooLarge {
                    size_usdc: 25.0,
                    max_usdc: 20.0,
                },
                "risk_order_too_large",
            ),
            (
                RiskViolation::RateLimitExceeded {
                    orders_in_window: 3,
                    max: 2,
                },
                "risk_rate_limit",
            ),
            (
                RiskViolation::VarBreached {
                    exposure_usdc: 6.0,
                    threshold_usdc: 5.0,
                    nav: 100.0,
                },
                "risk_var_breached",
            ),
        ];

        for (violation, expected) in cases {
            assert_eq!(violation.analytics_key(), expected);
        }
    }

    #[test]
    fn rate_limit_blocks_after_max_orders() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            max_orders_per_second: 2,
            ..RiskConfig::default()
        });
        assert!(rm.check_pre_order(1.0, 0, 100.0, 100.0).is_ok());
        assert!(rm.check_pre_order(1.0, 0, 100.0, 100.0).is_ok());
        let err = rm.check_pre_order(1.0, 0, 100.0, 100.0).unwrap_err();
        assert!(matches!(err, RiskViolation::RateLimitExceeded { .. }));
    }

    #[test]
    fn record_fill_and_close_update_daily_pnl() {
        let mut rm = default_rm();
        rm.record_fill(10.0); // spent $10 → VaR tracked, daily_pnl unchanged
        assert!((rm.daily_pnl()).abs() < 1e-9);
        rm.record_close(-6.0); // realized -$6 loss → daily_pnl = -6
        assert!((rm.daily_pnl() - (-6.0)).abs() < 1e-9);
    }

    #[test]
    fn reset_daily_clears_pnl_and_rate_buffer() {
        let mut rm = default_rm();
        rm.record_fill(5.0);
        rm.record_close(-5.0); // simulate realized loss
        assert!(rm.check_pre_order(1.0, 0, 100.0, 100.0).is_ok());
        rm.reset_daily();
        assert!((rm.daily_pnl()).abs() < 1e-9);
        // Should be able to submit max_orders_per_second orders again.
        assert!(rm.check_pre_order(1.0, 0, 100.0, 100.0).is_ok());
    }

    #[test]
    fn is_blocked_reflects_state() {
        let mut rm = default_rm();
        assert!(!rm.is_blocked());
        rm.trip_circuit_breaker("test");
        assert!(rm.is_blocked());
    }

    #[test]
    fn display_formats_are_human_readable() {
        // Just exercise the Display impl — no panic is sufficient.
        let violations = [
            RiskViolation::KillSwitchOff,
            RiskViolation::CircuitBreakerTripped {
                reason: "loss".into(),
                tripped_at: Instant::now(),
            },
            RiskViolation::DailyLossLimitExceeded {
                loss_usdc: 12.0,
                limit_usdc: 10.0,
            },
            RiskViolation::TooManyPositions { current: 5, max: 5 },
            RiskViolation::OrderTooLarge {
                size_usdc: 25.0,
                max_usdc: 20.0,
            },
            RiskViolation::RateLimitExceeded {
                orders_in_window: 3,
                max: 3,
            },
            RiskViolation::VarBreached {
                exposure_usdc: 6.0,
                threshold_usdc: 5.0,
                nav: 100.0,
            },
        ];
        for v in &violations {
            let s = v.to_string();
            assert!(!s.is_empty(), "Display should not be empty for {v:?}");
        }
    }

    #[test]
    fn var_breaches_when_exposure_exceeds_threshold() {
        // NAV=100, threshold=5% → limit=$5
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            var_threshold_pct: 0.05,
            max_orders_per_second: 100,
            ..RiskConfig::default()
        });
        // Fill $4.5 → under threshold
        assert!(rm.check_pre_order(4.5, 0, 100.0, 100.0).is_ok());
        rm.record_fill(4.5);
        // Next $1.0 → total $5.5 > $5.0 threshold
        let err = rm.check_pre_order(1.0, 0, 100.0, 100.0).unwrap_err();
        assert!(
            matches!(err, RiskViolation::VarBreached { .. }),
            "expected VarBreached, got {err:?}"
        );
    }

    #[test]
    fn var_clears_after_window_expires() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            var_threshold_pct: 0.05,
            var_window: Duration::from_millis(50), // tiny window for test
            max_orders_per_second: 100,
            ..RiskConfig::default()
        });
        assert!(rm.check_pre_order(4.0, 0, 100.0, 100.0).is_ok());
        rm.record_fill(4.0);

        // Wait for the window to expire
        std::thread::sleep(Duration::from_millis(60));

        // Exposure should have expired; new order under threshold passes
        let exposure = rm.rolling_exposure_usdc();
        assert!(
            exposure < 0.01,
            "exposure should be ~0 after window, got {exposure}"
        );
    }

    #[test]
    fn rolling_exposure_tracks_fills() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            max_orders_per_second: 100,
            ..RiskConfig::default()
        });
        rm.record_fill(3.0);
        rm.record_fill(2.0);
        let exposure = rm.rolling_exposure_usdc();
        assert!((exposure - 5.0).abs() < 1e-9);
    }

    #[test]
    fn var_circuit_breaker_auto_resets_after_window() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            var_threshold_pct: 0.05,
            var_window: Duration::from_millis(50),
            max_orders_per_second: 100,
            ..RiskConfig::default()
        });
        // Fill $4.5 → under threshold
        assert!(rm.check_pre_order(4.5, 0, 100.0, 100.0).is_ok());
        rm.record_fill(4.5);
        // Next $1.0 → total $5.5 > $5.0 → trips breaker
        let err = rm.check_pre_order(1.0, 0, 100.0, 100.0).unwrap_err();
        assert!(matches!(err, RiskViolation::VarBreached { .. }));
        assert!(rm.is_circuit_breaker_tripped());

        // Immediately: breaker is still tripped (exposure not decayed)
        assert!(rm.check_pre_order(1.0, 0, 100.0, 100.0).is_err());

        // Wait for the window to expire → exposure decays to 0
        std::thread::sleep(Duration::from_millis(60));

        // Now the breaker should auto-reset (rolling exposure decayed)
        assert!(rm.check_pre_order(1.0, 0, 100.0, 100.0).is_ok());
        assert!(!rm.is_circuit_breaker_tripped());
    }

    /// P0-5 invariant: daily_pnl equals the sum of all record_close calls.
    #[test]
    fn daily_pnl_equals_sum_of_closes() {
        let mut rm = default_rm();

        // Simulate several fills and closes with varying P&L.
        rm.record_fill(10.0);
        rm.record_close(2.5); // +$2.50
        rm.record_fill(15.0);
        rm.record_close(-3.0); // -$3.00
        rm.record_fill(5.0);
        rm.record_close(0.75); // +$0.75

        // daily_pnl must equal sum of closes, NOT affected by fills.
        let expected = 2.5 + (-3.0) + 0.75; // 0.25
        assert!(
            (rm.daily_pnl() - expected).abs() < 1e-9,
            "daily_pnl={} expected={}",
            rm.daily_pnl(),
            expected
        );
    }

    /// P0-1: record_fill only affects rolling exposure, never daily P&L.
    #[test]
    fn record_fill_does_not_change_daily_pnl() {
        let mut rm = default_rm();
        assert!((rm.daily_pnl() - 0.0).abs() < 1e-9);
        rm.record_fill(100.0);
        rm.record_fill(200.0);
        assert!(
            (rm.daily_pnl() - 0.0).abs() < 1e-9,
            "record_fill should not change daily_pnl, got {}",
            rm.daily_pnl()
        );
    }

    /// Rejected orders must not affect risk state (no fill, no exposure).
    #[test]
    fn rejected_order_leaves_state_unchanged() {
        let mut rm = RiskManager::new(RiskConfig {
            trading_enabled: true,
            max_single_order_usdc: 10.0,
            max_orders_per_second: 100,
            var_threshold_pct: 1.0,
            ..RiskConfig::default()
        });

        let pnl_before = rm.daily_pnl();
        let exposure_before = rm.rolling_exposure_usdc();

        // This order is too large — should be rejected.
        let result = rm.check_pre_order(50.0, 0, 100.0, 100.0);
        assert!(matches!(result, Err(RiskViolation::OrderTooLarge { .. })));

        // Risk state must be unchanged after rejection.
        assert!(
            (rm.daily_pnl() - pnl_before).abs() < 1e-9,
            "daily_pnl changed after rejected order"
        );
        assert!(
            (rm.rolling_exposure_usdc() - exposure_before).abs() < 1e-9,
            "exposure changed after rejected order"
        );
    }
}

// ─── Property-based risk manager verification (proptest, 10 000 iterations) ──

#[cfg(test)]
mod proptest_risk_verification {
    use super::*;
    use proptest::prelude::*;

    const PROPTEST_CASES: u32 = 10_000;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(PROPTEST_CASES))]

        /// Invariant: after the circuit breaker trips due to daily loss,
        /// the accumulated loss never exceeds `max_daily_loss_pct × starting_nav + epsilon`.
        /// The "epsilon" accounts for the single fill that triggered the breach.
        #[test]
        fn daily_loss_never_exceeds_limit(
            fills in proptest::collection::vec(0.01f64..50.0f64, 1..100usize),
            starting_nav in 100.0f64..10_000.0f64,
            max_loss_pct in 0.01f64..0.50f64,
        ) {
            let config = RiskConfig {
                max_daily_loss_pct: max_loss_pct,
                max_concurrent_positions: 100,
                max_single_order_usdc: 10_000.0,
                max_orders_per_second: 10_000,
                trading_enabled: true,
                var_threshold_pct: 1.0, // disable VaR for this test
                ..RiskConfig::default()
            };
            let mut rm = RiskManager::new(config);

            let limit_usdc = starting_nav * max_loss_pct;
            let mut breaker_tripped = false;

            for fill_usdc in &fills {
                // Try to place order.
                let result = rm.check_pre_order(*fill_usdc, 0, starting_nav, starting_nav);

                if breaker_tripped {
                    // Once tripped, ALL subsequent orders must be blocked.
                    prop_assert!(result.is_err(), "orders must be blocked after circuit breaker");
                    continue;
                }

                match result {
                    Ok(()) => {
                        rm.record_fill(*fill_usdc);
                        // Simulate position closed at total loss so daily P&L
                        // tracks realized losses (record_fill no longer moves
                        // daily_pnl).
                        rm.record_close(-*fill_usdc);
                    }
                    Err(RiskViolation::DailyLossLimitExceeded { .. }) |
                    Err(RiskViolation::CircuitBreakerTripped { .. }) => {
                        breaker_tripped = true;
                        // Verify the loss is within limit + one maximum fill.
                        let actual_loss = -rm.daily_pnl();
                        prop_assert!(
                            actual_loss <= limit_usdc + 50.0 + 1e-6,
                            "loss ${:.2} exceeded limit ${:.2} + max single fill $50",
                            actual_loss, limit_usdc
                        );
                    }
                    Err(RiskViolation::RateLimitExceeded { .. }) => {
                        // Rate limiting is not our concern here.
                    }
                    Err(RiskViolation::VarBreached { .. }) => {
                        breaker_tripped = true;
                    }
                    Err(other) => {
                        prop_assert!(false, "unexpected violation: {:?}", other);
                    }
                }
            }
        }

        /// Invariant: `check_pre_order` ALWAYS rejects orders larger than
        /// `max_single_order_usdc`.
        #[test]
        fn order_size_never_exceeds_max(
            order_size in 0.01f64..100.0f64,
            max_order in 0.01f64..100.0f64,
        ) {
            let config = RiskConfig {
                max_single_order_usdc: max_order,
                max_concurrent_positions: 100,
                max_orders_per_second: 10_000,
                trading_enabled: true,
                ..RiskConfig::default()
            };
            let mut rm = RiskManager::new(config);

            let result = rm.check_pre_order(order_size, 0, 10_000.0, 10_000.0);

            if order_size > max_order {
                prop_assert!(
                    matches!(result, Err(RiskViolation::OrderTooLarge { .. })),
                    "order ${:.2} should be rejected (max ${:.2})",
                    order_size, max_order
                );
            }
        }

        /// Invariant: concurrent positions are ALWAYS capped.
        #[test]
        fn positions_never_exceed_max(
            open_positions in 0usize..20,
            max_positions in 1usize..10,
        ) {
            let config = RiskConfig {
                max_concurrent_positions: max_positions,
                max_orders_per_second: 10_000,
                trading_enabled: true,
                ..RiskConfig::default()
            };
            let mut rm = RiskManager::new(config);

            let result = rm.check_pre_order(1.0, open_positions, 10_000.0, 10_000.0);

            if open_positions >= max_positions {
                prop_assert!(
                    matches!(result, Err(RiskViolation::TooManyPositions { .. })),
                    "should reject at {}/{} positions",
                    open_positions, max_positions
                );
            }
        }

        /// Invariant: rolling exposure never exceeds VaR threshold without
        /// triggering a breach.
        #[test]
        fn var_always_triggers_when_exceeded(
            fills in proptest::collection::vec(0.01f64..5.0f64, 1..20usize),
            nav in 50.0f64..500.0f64,
            threshold_pct in 0.01f64..0.20f64,
        ) {
            let config = RiskConfig {
                var_threshold_pct: threshold_pct,
                max_concurrent_positions: 100,
                max_single_order_usdc: 10_000.0,
                max_orders_per_second: 10_000,
                max_daily_loss_pct: 1.0, // disable daily loss for this test
                trading_enabled: true,
                ..RiskConfig::default()
            };
            let mut rm = RiskManager::new(config);

            let threshold_usdc = nav * threshold_pct;
            let mut total_exposure = 0.0;

            for fill in &fills {
                let result = rm.check_pre_order(*fill, 0, nav, nav);
                match result {
                    Ok(()) => {
                        rm.record_fill(*fill);
                        total_exposure += fill;
                        // Exposure should be at or below threshold.
                        prop_assert!(
                            total_exposure <= threshold_usdc + 1e-6,
                            "exposure ${:.2} passed but threshold is ${:.2}",
                            total_exposure, threshold_usdc
                        );
                    }
                    Err(RiskViolation::VarBreached { exposure_usdc, .. }) => {
                        // VaR correctly rejected — pending exposure exceeds threshold.
                        prop_assert!(
                            exposure_usdc > threshold_usdc - 1e-6,
                            "VaR rejected at ${:.2} but threshold is ${:.2}",
                            exposure_usdc, threshold_usdc
                        );
                    }
                    Err(RiskViolation::CircuitBreakerTripped { .. }) => {
                        // Auto-reset may or may not fire depending on rolling_sum;
                        // either way the order was blocked — valid.
                    }
                    Err(_) => {}
                }
            }
        }
    }
}
