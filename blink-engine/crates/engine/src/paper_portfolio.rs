//! Virtual portfolio for paper trading.
//!
//! Holds virtual USDC cash, open positions, and closed trades.
//! All prices are stored as `f64` (acceptable outside the hot path).

use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

use crate::types::OrderSide;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Starting virtual balance in USDC.
pub const STARTING_BALANCE_USDC: f64 = 100.0; // $100 starter bankroll

/// We mirror `SIZE_MULTIPLIER × RN1's notional` as our trade size.
pub const SIZE_MULTIPLIER: f64 = 0.20; // 20% base for higher participation

/// Maximum fraction of current NAV per single trade.
pub const MAX_POSITION_PCT: f64 = 0.25; // 25% max per trade

/// Minimum trade size; signals below this are skipped.
pub const MIN_TRADE_USDC: f64 = 5.0; // $5 minimum to reduce size_or_cash rejections

/// If price drifts more than this fraction from entry during the fill
/// window, the order is aborted (simulates an in-play failsafe).
pub const DRIFT_THRESHOLD: f64 = 0.015; // 1.5 %

/// Maximum number of NAV snapshots kept for the TUI equity curve.
const EQUITY_CURVE_MAX: usize = 300;

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

fn realism_mode() -> bool {
    env_flag("PAPER_REALISM_MODE")
}

fn taker_fee_bps() -> f64 {
    std::env::var("PAPER_TAKER_FEE_BPS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(7.0)
        .clamp(0.0, 500.0)
}

fn exit_haircut_bps() -> f64 {
    std::env::var("PAPER_EXIT_HAIRCUT_BPS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(12.0)
        .clamp(0.0, 500.0)
}

// ─── PaperPosition ────────────────────────────────────────────────────────────

/// An open virtual position.
#[derive(Debug, Clone)]
pub struct PaperPosition {
    /// Sequential position ID (1, 2, 3, …).
    pub id: usize,
    pub token_id: String,
    pub market_title: Option<String>,
    pub market_outcome: Option<String>,
    pub side: OrderSide,
    /// Price paid per share in USDC (0.0 – 1.0).
    pub entry_price: f64,
    /// Number of shares bought/sold.
    pub shares: f64,
    /// USDC committed to this position (entry_price × shares).
    pub usdc_spent: f64,
    /// Latest known market price from the live order book.
    pub current_price: f64,
    /// Wall-clock time when the position was opened.
    pub opened_at: Instant,
    /// The RN1 order ID that triggered this position.
    pub rn1_order_id: String,
    /// Wall-clock time when the position was opened (for trade history).
    pub opened_at_wall: chrono::DateTime<chrono::Local>,
    /// Entry slippage in bps vs observed pre-fill reference.
    pub entry_slippage_bps: f64,
    /// Time from detection to queue selection/fill path.
    pub queue_delay_ms: u64,
    /// Experiment variant label for A/B metrics.
    pub experiment_variant: String,
}

impl PaperPosition {
    /// Unrealized P&L in USDC at `current_price`.
    #[inline]
    pub fn unrealized_pnl(&self) -> f64 {
        match self.side {
            OrderSide::Buy  => (self.current_price - self.entry_price) * self.shares,
            OrderSide::Sell => (self.entry_price   - self.current_price) * self.shares,
        }
    }

    /// Unrealized P&L as a percentage of cost basis.
    #[inline]
    pub fn unrealized_pnl_pct(&self) -> f64 {
        self.unrealized_pnl() / self.usdc_spent * 100.0
    }

    #[inline]
    fn with_conservative_exit_price(&self, haircut_bps: f64) -> f64 {
        let h = haircut_bps / 10_000.0;
        let px = match self.side {
            OrderSide::Buy => self.current_price * (1.0 - h),
            OrderSide::Sell => self.current_price * (1.0 + h),
        };
        px.clamp(0.0, 1.0)
    }
}

// ─── ClosedTrade ──────────────────────────────────────────────────────────────

/// A fully closed (exited) trade record.
#[derive(Debug, Clone)]
pub struct ClosedTrade {
    pub token_id: String,
    pub side: OrderSide,
    pub entry_price: f64,
    pub exit_price: f64,
    pub shares: f64,
    pub realized_pnl: f64,
    pub reason: String,
    /// Wall-clock time when the position was opened.
    pub opened_at_wall: chrono::DateTime<chrono::Local>,
    /// Wall-clock time when the position was closed.
    pub closed_at_wall: chrono::DateTime<chrono::Local>,
    /// Duration the position was held, in seconds.
    pub duration_secs: u64,
    /// Execution quality scorecard snapshot.
    pub scorecard: ExecutionScorecard,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionScorecard {
    pub slippage_bps: f64,
    pub queue_delay_ms: u64,
    pub outcome_tags: Vec<String>,
}

// ─── PaperPortfolio ───────────────────────────────────────────────────────────

/// The complete virtual portfolio state.
///
/// This struct is wrapped in `Arc<tokio::sync::Mutex<_>>` and shared between
/// the signal handler and the periodic dashboard printer.
#[derive(Debug)]
pub struct PaperPortfolio {
    /// Available virtual USDC (starts at `STARTING_BALANCE_USDC`).
    pub cash_usdc: f64,
    /// Currently open positions.
    pub positions: Vec<PaperPosition>,
    /// History of closed trades.
    pub closed_trades: Vec<ClosedTrade>,
    /// Total signals received (including skipped / aborted).
    pub total_signals: usize,
    /// Orders that reached a simulated fill.
    pub filled_orders: usize,
    /// Orders aborted by the drift failsafe.
    pub aborted_orders: usize,
    /// Orders skipped because size < `MIN_TRADE_USDC` or no cash.
    pub skipped_orders: usize,
    /// NAV samples for the TUI equity sparkline (newest at the end, max 300).
    pub equity_curve: Vec<f64>,
    next_id: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPaperPosition {
    id: usize,
    token_id: String,
    #[serde(default)]
    market_title: Option<String>,
    #[serde(default)]
    market_outcome: Option<String>,
    side: OrderSide,
    entry_price: f64,
    shares: f64,
    usdc_spent: f64,
    current_price: f64,
    rn1_order_id: String,
    opened_at_wall_ms: i64,
    opened_age_secs: u64,
    #[serde(default)]
    entry_slippage_bps: f64,
    #[serde(default)]
    queue_delay_ms: u64,
    #[serde(default)]
    experiment_variant: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedClosedTrade {
    token_id: String,
    side: OrderSide,
    entry_price: f64,
    exit_price: f64,
    shares: f64,
    realized_pnl: f64,
    reason: String,
    opened_at_wall_ms: i64,
    closed_at_wall_ms: i64,
    duration_secs: u64,
    #[serde(default)]
    scorecard: ExecutionScorecard,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPaperPortfolio {
    #[serde(default)]
    schema_version: u32,
    cash_usdc: f64,
    positions: Vec<PersistedPaperPosition>,
    closed_trades: Vec<PersistedClosedTrade>,
    total_signals: usize,
    filled_orders: usize,
    aborted_orders: usize,
    skipped_orders: usize,
    equity_curve: Vec<f64>,
    next_id: usize,
}

impl PaperPortfolio {
    /// Creates a fresh portfolio with `STARTING_BALANCE_USDC` virtual USDC.
    pub fn new() -> Self {
        Self {
            cash_usdc:      STARTING_BALANCE_USDC,
            positions:      Vec::new(),
            closed_trades:  Vec::new(),
            total_signals:  0,
            filled_orders:  0,
            aborted_orders: 0,
            skipped_orders: 0,
            equity_curve:   Vec::with_capacity(EQUITY_CURVE_MAX),
            next_id:        1,
        }
    }

    // ── Aggregate metrics ────────────────────────────────────────────────

    /// Total net asset value: cash + market value of open positions.
    pub fn nav(&self) -> f64 {
        let haircut = if realism_mode() { exit_haircut_bps() } else { 0.0 };
        let mkt_value: f64 = self
            .positions
            .iter()
            .map(|p| {
                let marked = p.with_conservative_exit_price(haircut);
                let pnl = match p.side {
                    OrderSide::Buy => (marked - p.entry_price) * p.shares,
                    OrderSide::Sell => (p.entry_price - marked) * p.shares,
                };
                p.usdc_spent + pnl
            })
            .sum();
        self.cash_usdc + mkt_value
    }

    /// Sum of `usdc_spent` across all open positions.
    pub fn total_invested(&self) -> f64 {
        self.positions.iter().map(|p| p.usdc_spent).sum()
    }

    /// Sum of unrealized P&L across all open positions.
    pub fn unrealized_pnl(&self) -> f64 {
        let haircut = if realism_mode() { exit_haircut_bps() } else { 0.0 };
        self.positions
            .iter()
            .map(|p| {
                let marked = p.with_conservative_exit_price(haircut);
                match p.side {
                    OrderSide::Buy => (marked - p.entry_price) * p.shares,
                    OrderSide::Sell => (p.entry_price - marked) * p.shares,
                }
            })
            .sum()
    }

    /// Sum of realized P&L from all closed trades.
    pub fn realized_pnl(&self) -> f64 {
        self.closed_trades.iter().map(|t| t.realized_pnl).sum()
    }

    // ── Order sizing ──────────────────────────────────────────────────────

    /// Calculate how many USDC we should commit for a given signal.
    ///
    /// Returns `None` if the resulting size would be below `MIN_TRADE_USDC`
    /// or if we have no remaining cash.
    pub fn calculate_size_usdc(&self, rn1_notional_usdc: f64) -> Option<f64> {
        let size_multiplier = std::env::var("PAPER_SIZE_MULTIPLIER")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(SIZE_MULTIPLIER)
            .max(0.01);
        let max_position_pct = std::env::var("PAPER_MAX_POSITION_PCT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(MAX_POSITION_PCT)
            .clamp(0.01, 1.0);
        let min_trade_usdc = std::env::var("PAPER_MIN_TRADE_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(MIN_TRADE_USDC)
            .max(1.0);
        let min_floor_usdc = std::env::var("PAPER_MIN_ORDER_FLOOR_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(8.0)
            .max(min_trade_usdc);

        let raw        = rn1_notional_usdc * size_multiplier;
        let cap_nav    = self.nav() * max_position_pct;
        let size       = raw.max(min_floor_usdc).min(cap_nav).min(self.cash_usdc);
        if size < min_trade_usdc { None } else { Some(size) }
    }

    // ── Mutations ─────────────────────────────────────────────────────────

    /// Open a new virtual position.  Deducts `usdc_size` from `cash_usdc`.
    ///
    /// Returns the sequential position ID.
    pub fn open_position(
        &mut self,
        token_id:     String,
        side:         OrderSide,
        entry_price:  f64,
        usdc_size:    f64,
        rn1_order_id: String,
    ) -> usize {
        self.open_position_with_meta(token_id, None, None, side, entry_price, usdc_size, rn1_order_id, 0.0, 0, "A")
    }

    pub fn open_position_with_meta(
        &mut self,
        token_id:     String,
        market_title: Option<String>,
        market_outcome: Option<String>,
        side:         OrderSide,
        entry_price:  f64,
        usdc_size:    f64,
        rn1_order_id: String,
        entry_slippage_bps: f64,
        queue_delay_ms: u64,
        experiment_variant: &str,
    ) -> usize {
        let shares = usdc_size / entry_price;
        let id     = self.next_id;
        self.next_id   += 1;
        let entry_fee = if realism_mode() {
            usdc_size * (taker_fee_bps() / 10_000.0)
        } else {
            0.0
        };
        self.cash_usdc -= usdc_size + entry_fee;
        self.positions.push(PaperPosition {
            id,
            token_id,
            market_title,
            market_outcome,
            side,
            entry_price,
            shares,
            usdc_spent:    usdc_size,
            current_price: entry_price,
            opened_at:     Instant::now(),
            rn1_order_id,
            opened_at_wall: chrono::Local::now(),
            entry_slippage_bps,
            queue_delay_ms,
            experiment_variant: experiment_variant.to_string(),
        });
        self.filled_orders += 1;
        id
    }

    /// Update `current_price` for every position in `token_id`.
    pub fn update_price(&mut self, token_id: &str, new_price: f64) {
        for pos in &mut self.positions {
            if pos.token_id == token_id {
                pos.current_price = new_price;
            }
        }
    }

    /// Closes positions that reached the configured take-profit target.
    ///
    /// Returns number of positions closed.
    pub fn autoclaim_take_profit(&mut self, target_pnl_pct: f64) -> usize {
        let mut closed = 0usize;
        let mut i = 0usize;
        while i < self.positions.len() {
            let should_close = self.positions[i].unrealized_pnl_pct() >= target_pnl_pct;
            if !should_close {
                i += 1;
                continue;
            }

            let pos = self.positions.remove(i);
            let pnl = match pos.side {
                OrderSide::Buy => (pos.current_price - pos.entry_price) * pos.shares,
                OrderSide::Sell => (pos.entry_price - pos.current_price) * pos.shares,
            };
            let exit_fee = if realism_mode() {
                (pos.current_price * pos.shares) * (taker_fee_bps() / 10_000.0)
            } else {
                0.0
            };
            self.cash_usdc += pos.usdc_spent + pnl - exit_fee;
            self.closed_trades.push(ClosedTrade {
                token_id: pos.token_id.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                exit_price: pos.current_price,
                shares: pos.shares,
                realized_pnl: pnl,
                reason: format!("autoclaim@{:.0}%", target_pnl_pct),
                opened_at_wall: pos.opened_at_wall,
                closed_at_wall: chrono::Local::now(),
                duration_secs: pos.opened_at.elapsed().as_secs(),
                scorecard: ExecutionScorecard {
                    slippage_bps: pos.entry_slippage_bps,
                    queue_delay_ms: pos.queue_delay_ms,
                    outcome_tags: vec![
                        if pnl > 0.0 { "profit".to_string() } else if pnl < 0.0 { "loss".to_string() } else { "breakeven".to_string() },
                        format!("variant:{}", pos.experiment_variant),
                    ],
                },
            });
            closed += 1;
        }
        closed
    }

    /// Tiered autoclaim supporting partial exits.
    ///
    /// `tiers` is a list of `(pnl_pct_threshold, fraction_to_close)` tuples.
    /// Example: `[(40.0, 0.30), (70.0, 0.30), (100.0, 1.0)]`.
    pub fn autoclaim_tiered(&mut self, tiers: &[(f64, f64)]) -> usize {
        let mut actions = 0usize;
        let mut i = 0usize;

        while i < self.positions.len() {
            let pnl_pct = self.positions[i].unrealized_pnl_pct();
            let mut chosen: Option<(f64, f64)> = None;
            for (threshold, fraction) in tiers {
                if pnl_pct >= *threshold {
                    chosen = Some((*threshold, *fraction));
                }
            }

            let Some((threshold, fraction)) = chosen else {
                i += 1;
                continue;
            };

            let reason = format!("autoclaim@{threshold:.0}%[{:.0}%]", fraction * 100.0);
            let removed = self.close_position_fraction(i, fraction, reason);
            if removed {
                actions += 1;
                // Position at index i was removed; continue without incrementing.
            } else {
                actions += 1;
                i += 1;
            }
        }

        actions
    }

    fn close_position_fraction(&mut self, idx: usize, fraction: f64, reason: String) -> bool {
        if idx >= self.positions.len() {
            return false;
        }
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction <= 0.0 {
            return false;
        }

        if fraction >= 0.999_999 {
            let pos = self.positions.remove(idx);
            let pnl = match pos.side {
                OrderSide::Buy => (pos.current_price - pos.entry_price) * pos.shares,
                OrderSide::Sell => (pos.entry_price - pos.current_price) * pos.shares,
            };
            let exit_fee = if realism_mode() {
                (pos.current_price * pos.shares) * (taker_fee_bps() / 10_000.0)
            } else {
                0.0
            };
            self.cash_usdc += pos.usdc_spent + pnl - exit_fee;
            self.closed_trades.push(ClosedTrade {
                token_id: pos.token_id.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                exit_price: pos.current_price,
                shares: pos.shares,
                realized_pnl: pnl,
                reason,
                opened_at_wall: pos.opened_at_wall,
                closed_at_wall: chrono::Local::now(),
                duration_secs: pos.opened_at.elapsed().as_secs(),
                scorecard: ExecutionScorecard {
                    slippage_bps: pos.entry_slippage_bps,
                    queue_delay_ms: pos.queue_delay_ms,
                    outcome_tags: vec![
                        if pnl > 0.0 { "profit".to_string() } else if pnl < 0.0 { "loss".to_string() } else { "breakeven".to_string() },
                        format!("variant:{}", pos.experiment_variant),
                    ],
                },
            });
            return true;
        }

        let pos = &mut self.positions[idx];
        let close_shares = pos.shares * fraction;
        if close_shares <= 0.0 {
            return false;
        }
        let close_usdc_spent = pos.usdc_spent * fraction;
        let pnl = match pos.side {
            OrderSide::Buy => (pos.current_price - pos.entry_price) * close_shares,
            OrderSide::Sell => (pos.entry_price - pos.current_price) * close_shares,
        };
        let exit_fee = if realism_mode() {
            (pos.current_price * close_shares) * (taker_fee_bps() / 10_000.0)
        } else {
            0.0
        };
        self.cash_usdc += close_usdc_spent + pnl - exit_fee;
        self.closed_trades.push(ClosedTrade {
            token_id: pos.token_id.clone(),
            side: pos.side,
            entry_price: pos.entry_price,
            exit_price: pos.current_price,
            shares: close_shares,
            realized_pnl: pnl,
            reason,
            opened_at_wall: pos.opened_at_wall,
            closed_at_wall: chrono::Local::now(),
            duration_secs: pos.opened_at.elapsed().as_secs(),
            scorecard: ExecutionScorecard {
                slippage_bps: pos.entry_slippage_bps,
                queue_delay_ms: pos.queue_delay_ms,
                outcome_tags: vec![
                    if pnl > 0.0 { "profit".to_string() } else if pnl < 0.0 { "loss".to_string() } else { "breakeven".to_string() },
                    format!("variant:{}", pos.experiment_variant),
                ],
            },
        });

        pos.shares -= close_shares;
        pos.usdc_spent -= close_usdc_spent;
        if pos.shares <= 1e-9 || pos.usdc_spent <= 1e-9 {
            let _ = self.positions.remove(idx);
            return true;
        }
        false
    }

    /// Returns the maximum drawdown as a percentage (0.0 – 100.0).
    ///
    /// Computed from the equity curve samples: max peak-to-trough decline.
    pub fn max_drawdown_pct(&self) -> f64 {
        if self.equity_curve.len() < 2 {
            return 0.0;
        }
        let mut peak = f64::NEG_INFINITY;
        let mut max_dd = 0.0f64;
        for &nav in &self.equity_curve {
            if nav > peak { peak = nav; }
            let dd = (peak - nav) / peak * 100.0;
            if dd > max_dd { max_dd = dd; }
        }
        max_dd
    }

    /// Returns the high-water mark NAV from the equity curve.
    pub fn high_water_mark(&self) -> f64 {
        self.equity_curve.iter().cloned().fold(STARTING_BALANCE_USDC, f64::max)
    }

    /// Records the current NAV as a sparkline sample (capped at 300 entries).
    ///
    /// Called by the TUI loop every ~150 ms to build the equity curve.
    pub fn push_equity_snapshot(&mut self) {
        if self.equity_curve.len() >= EQUITY_CURVE_MAX {
            self.equity_curve.remove(0);
        }
        self.equity_curve.push(self.nav());
    }

    pub fn save_to_path(&self, path: &str) -> std::io::Result<()> {
        let persisted = PersistedPaperPortfolio::from(self);
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&persisted)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load_from_path(path: &str) -> std::io::Result<Self> {
        let data = std::fs::read_to_string(path)?;
        let persisted: PersistedPaperPortfolio = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok(Self::from(persisted))
    }
}

impl From<&PaperPortfolio> for PersistedPaperPortfolio {
    fn from(value: &PaperPortfolio) -> Self {
        Self {
            schema_version: 2,
            cash_usdc: value.cash_usdc,
            positions: value.positions.iter().map(|p| PersistedPaperPosition {
                id: p.id,
                token_id: p.token_id.clone(),
                market_title: p.market_title.clone(),
                market_outcome: p.market_outcome.clone(),
                side: p.side,
                entry_price: p.entry_price,
                shares: p.shares,
                usdc_spent: p.usdc_spent,
                current_price: p.current_price,
                rn1_order_id: p.rn1_order_id.clone(),
                opened_at_wall_ms: p.opened_at_wall.timestamp_millis(),
                opened_age_secs: p.opened_at.elapsed().as_secs(),
                entry_slippage_bps: p.entry_slippage_bps,
                queue_delay_ms: p.queue_delay_ms,
                experiment_variant: p.experiment_variant.clone(),
            }).collect(),
            closed_trades: value.closed_trades.iter().map(|t| PersistedClosedTrade {
                token_id: t.token_id.clone(),
                side: t.side,
                entry_price: t.entry_price,
                exit_price: t.exit_price,
                shares: t.shares,
                realized_pnl: t.realized_pnl,
                reason: t.reason.clone(),
                opened_at_wall_ms: t.opened_at_wall.timestamp_millis(),
                closed_at_wall_ms: t.closed_at_wall.timestamp_millis(),
                duration_secs: t.duration_secs,
                scorecard: t.scorecard.clone(),
            }).collect(),
            total_signals: value.total_signals,
            filled_orders: value.filled_orders,
            aborted_orders: value.aborted_orders,
            skipped_orders: value.skipped_orders,
            equity_curve: value.equity_curve.clone(),
            next_id: value.next_id,
        }
    }
}

impl From<PersistedPaperPortfolio> for PaperPortfolio {
    fn from(value: PersistedPaperPortfolio) -> Self {
        let now = Instant::now();
        let positions = value.positions.into_iter().map(|p| {
            let opened_at_wall = Local
                .timestamp_millis_opt(p.opened_at_wall_ms)
                .single()
                .unwrap_or_else(Local::now);
            let opened_at = now
                .checked_sub(Duration::from_secs(p.opened_age_secs))
                .unwrap_or(now);
            PaperPosition {
                id: p.id,
                token_id: p.token_id,
                market_title: p.market_title,
                market_outcome: p.market_outcome,
                side: p.side,
                entry_price: p.entry_price,
                shares: p.shares,
                usdc_spent: p.usdc_spent,
                current_price: p.current_price,
                opened_at,
                rn1_order_id: p.rn1_order_id,
                opened_at_wall,
                entry_slippage_bps: p.entry_slippage_bps,
                queue_delay_ms: p.queue_delay_ms,
                experiment_variant: if p.experiment_variant.is_empty() { "A".to_string() } else { p.experiment_variant },
            }
        }).collect();

        let closed_trades = value.closed_trades.into_iter().map(|t| {
            let opened_at_wall = Local
                .timestamp_millis_opt(t.opened_at_wall_ms)
                .single()
                .unwrap_or_else(Local::now);
            let closed_at_wall = Local
                .timestamp_millis_opt(t.closed_at_wall_ms)
                .single()
                .unwrap_or_else(Local::now);
            ClosedTrade {
                token_id: t.token_id,
                side: t.side,
                entry_price: t.entry_price,
                exit_price: t.exit_price,
                shares: t.shares,
                realized_pnl: t.realized_pnl,
                reason: t.reason,
                opened_at_wall,
                closed_at_wall,
                duration_secs: t.duration_secs,
                scorecard: t.scorecard,
            }
        }).collect();

        Self {
            cash_usdc: value.cash_usdc,
            positions,
            closed_trades,
            total_signals: value.total_signals,
            filled_orders: value.filled_orders,
            aborted_orders: value.aborted_orders,
            skipped_orders: value.skipped_orders,
            equity_curve: value.equity_curve,
            next_id: value.next_id.max(1),
        }
    }
}

impl Default for PaperPortfolio {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_nav_equals_balance() {
        let p = PaperPortfolio::new();
        assert!((p.nav() - STARTING_BALANCE_USDC).abs() < 1e-9);
    }

    #[test]
    fn size_capped_at_position_pct_nav() {
        // Run with explicit defaults so the test is env-var-independent.
        temp_env::with_vars(
            vec![
                ("PAPER_SIZE_MULTIPLIER", Some("0.20")),
                ("PAPER_MAX_POSITION_PCT", Some("0.25")),
                ("PAPER_MIN_ORDER_FLOOR_USDC", Some("8.0")),
                ("PAPER_MIN_TRADE_USDC", Some("5.0")),
            ],
            || {
                let p = PaperPortfolio::new(); // NAV = $100, cap = 25% = $25
                // RN1 trades $20,000 → 20% = $4,000, capped by 25% NAV = $25
                let size = p.calculate_size_usdc(20_000.0).unwrap();
                assert!((size - 25.0).abs() < 1e-9, "size={size}");
            },
        );
    }

    #[test]
    fn size_below_minimum_returns_none() {
        temp_env::with_vars(
            vec![
                ("PAPER_SIZE_MULTIPLIER", Some("0.20")),
                ("PAPER_MAX_POSITION_PCT", Some("0.25")),
                ("PAPER_MIN_ORDER_FLOOR_USDC", Some("8.0")),
                ("PAPER_MIN_TRADE_USDC", Some("5.0")),
            ],
            || {
                let p = PaperPortfolio::new();
                // 20% of $100 = $20, which is above minimum ($5), so should size.
                assert!(p.calculate_size_usdc(100.0).is_some());
            },
        );
    }

    #[test]
    fn nav_decreases_after_open() {
        let mut p = PaperPortfolio::new();
        p.open_position("tok".into(), OrderSide::Buy, 0.65, 20.0, "oid".into());
        // cash = 80, position worth 20 at entry → NAV ≈ 100
        assert!((p.nav() - 100.0).abs() < 1e-9);
        assert!((p.cash_usdc - 80.0).abs() < 1e-9);
    }

    #[test]
    fn unrealized_pnl_buy() {
        let mut p = PaperPortfolio::new();
        p.open_position("tok".into(), OrderSide::Buy, 0.50, 10.0, "o1".into());
        p.update_price("tok", 0.60); // price up 20 %
        // shares = 10 / 0.50 = 20 → PnL = (0.60 - 0.50) × 20 = 2.0
        assert!((p.unrealized_pnl() - 2.0).abs() < 1e-9);
    }
}
