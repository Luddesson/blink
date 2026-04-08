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
pub const SIZE_MULTIPLIER: f64 = 0.05; // 5% of RN1 notional

/// Maximum fraction of current NAV per single trade.
pub const MAX_POSITION_PCT: f64 = 0.25; // 25% max per trade → up to 4 concurrent positions

/// Minimum trade size; signals below this are skipped.
pub const MIN_TRADE_USDC: f64 = 2.0; // $2 minimum

/// If price drifts more than this fraction from entry during the fill
/// window, the order is aborted (simulates an in-play failsafe).
pub const DRIFT_THRESHOLD: f64 = 0.015; // 1.5 %

/// Maximum number of NAV snapshots kept for the equity curve (10s sampling → ~28h).
const EQUITY_CURVE_MAX: usize = 10_080;

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

fn realism_mode() -> bool {
    env_flag("PAPER_REALISM_MODE")
}

/// Detect Polymarket fee category from market title.
/// Returns (category_name, fee_rate) per Polymarket's current 0.4% flat taker fee.
pub fn detect_fee_category(title: &str) -> (&'static str, f64) {
    let t = title.to_lowercase();
    // Geopolitics — 0% fee (Polymarket promo category, still 0)
    if t.contains("geopolit") || t.contains("sanction") || t.contains("nato")
        || t.contains("war ") || t.contains("military") || t.contains("treaty")
        || t.contains("united nations") || t.contains("diplomacy")
    {
        return ("geopolitics", 0.00);
    }
    // Sports
    if t.contains("win on 2") || t.contains("o/u ") || t.contains("over/under")
        || t.contains(" vs ") || t.contains(" vs. ") || t.contains("afc")
        || t.contains(" fc ") || t.contains("nba") || t.contains("nfl")
        || t.contains("mlb") || t.contains("nhl") || t.contains("soccer")
        || t.contains("tennis") || t.contains("golf") || t.contains("boxing")
        || t.contains("ufc") || t.contains("mma") || t.contains("f1 ")
        || t.contains("formula") || t.contains("grand prix")
        || t.contains("serie a") || t.contains("la liga") || t.contains("bundesliga")
        || t.contains("premier league") || t.contains("ligue 1")
        || t.contains("campinas") || t.contains("linz") || t.contains("open:")
        || t.contains("championship") || t.contains("cup ")
    {
        return ("sports", 0.0001);
    }
    // Politics
    if t.contains("president") || t.contains("election") || t.contains("congress")
        || t.contains("senate") || t.contains("governor") || t.contains("democrat")
        || t.contains("republican") || t.contains("trump") || t.contains("biden")
        || t.contains("poll") || t.contains("vote") || t.contains("party")
        || t.contains("legislation") || t.contains("bill ")
    {
        return ("politics", 0.0001);
    }
    // Crypto
    if t.contains("bitcoin") || t.contains("btc") || t.contains("ethereum")
        || t.contains("eth ") || t.contains("crypto") || t.contains("solana")
        || t.contains("token") || t.contains("defi") || t.contains("nft")
    {
        return ("crypto", 0.0001);
    }
    // Default / Other — 0.01%
    ("other", 0.0001)
}

/// Polymarket taker fee: flat 0.01% of notional (`shares × price × rate`).
/// Per Polymarket 2025 fee schedule: 1 basis point (0.0001) of contract premium.
pub fn polymarket_taker_fee(shares: f64, price: f64) -> f64 {
    let rate = std::env::var("POLYMARKET_FEE_RATE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0001)
        .clamp(0.0, 0.20);
    let fee = shares * price * rate;  // notional × rate (flat, not variance)
    (fee * 100_000.0).round() / 100_000.0
}

/// Category-aware taker fee: flat rate on notional.
pub fn polymarket_taker_fee_with_rate(shares: f64, price: f64, fee_rate: f64) -> f64 {
    let fee = shares * price * fee_rate;  // notional × rate (flat, not variance)
    (fee * 100_000.0).round() / 100_000.0
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
    /// Entry fee paid (USDC) at open — tracked per-position for accounting.
    pub entry_fee_paid_usdc: f64,
    /// Latest known market price from the live order book.
    pub current_price: f64,
    /// Highest price seen since entry (for trailing stop).
    pub peak_price: f64,
    /// Fee category detected from market title (e.g. "sports", "geopolitics").
    pub fee_category: String,
    /// Fee rate for this position's category.
    pub fee_rate: f64,
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
    /// Unix timestamp — game/event kickoff time (from Gamma API).
    pub event_start_time: Option<i64>,
    /// Unix timestamp — market resolution deadline (from Gamma API).
    pub event_end_time: Option<i64>,
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
    #[allow(dead_code)]
    pub market_title: Option<String>,
    pub side: OrderSide,
    pub entry_price: f64,
    pub exit_price: f64,
    pub shares: f64,
    pub realized_pnl: f64,
    /// Total fees (entry + exit) attributed to this closed portion, in USDC.
    pub fees_paid_usdc: f64,
    pub reason: String,
    /// Wall-clock time when the position was opened.
    pub opened_at_wall: chrono::DateTime<chrono::Local>,
    /// Wall-clock time when the position was closed.
    pub closed_at_wall: chrono::DateTime<chrono::Local>,
    /// Duration the position was held, in seconds.
    pub duration_secs: u64,
    /// Execution quality scorecard snapshot.
    pub scorecard: ExecutionScorecard,
    /// Unix timestamp — game/event kickoff time (from Gamma API).
    pub event_start_time: Option<i64>,
    /// Unix timestamp — market resolution deadline (from Gamma API).
    pub event_end_time: Option<i64>,
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
    /// Cumulative fees paid (entry + exit) in USDC.
    pub total_fees_paid_usdc: f64,
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
    /// NAV samples for the equity curve (10s sampling, newest at end, max ~28h).
    pub equity_curve: Vec<f64>,
    /// Unix-ms timestamp for each equity_curve sample.
    pub equity_timestamps: Vec<i64>,
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
    entry_fee_paid_usdc: f64,
    #[serde(default)]
    entry_slippage_bps: f64,
    #[serde(default)]
    queue_delay_ms: u64,
    #[serde(default)]
    experiment_variant: String,
    #[serde(default)]
    fee_category: String,
    #[serde(default)]
    fee_rate: f64,
    #[serde(default)]
    event_start_time: Option<i64>,
    #[serde(default)]
    event_end_time: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedClosedTrade {
    token_id: String,
    #[serde(default)]
    market_title: Option<String>,
    side: OrderSide,
    entry_price: f64,
    exit_price: f64,
    shares: f64,
    realized_pnl: f64,
    #[serde(default)]
    fees_paid_usdc: f64,
    reason: String,
    opened_at_wall_ms: i64,
    closed_at_wall_ms: i64,
    duration_secs: u64,
    #[serde(default)]
    scorecard: ExecutionScorecard,
    #[serde(default)]
    event_start_time: Option<i64>,
    #[serde(default)]
    event_end_time: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPaperPortfolio {
    #[serde(default)]
    schema_version: u32,
    cash_usdc: f64,
    total_fees_paid_usdc: f64,
    positions: Vec<PersistedPaperPosition>,
    closed_trades: Vec<PersistedClosedTrade>,
    total_signals: usize,
    filled_orders: usize,
    aborted_orders: usize,
    skipped_orders: usize,
    equity_curve: Vec<f64>,
    #[serde(default)]
    equity_timestamps: Vec<i64>,
    next_id: usize,
}

impl PaperPortfolio {
    /// Creates a fresh portfolio with `STARTING_BALANCE_USDC` virtual USDC.
    pub fn new() -> Self {
        Self {
            cash_usdc:      STARTING_BALANCE_USDC,
            total_fees_paid_usdc: 0.0,
            positions:      Vec::new(),
            closed_trades:  Vec::new(),
            total_signals:  0,
            filled_orders:  0,
            aborted_orders: 0,
            skipped_orders: 0,
            equity_curve:       Vec::with_capacity(EQUITY_CURVE_MAX),
            equity_timestamps:  Vec::with_capacity(EQUITY_CURVE_MAX),
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
        self.calculate_size_usdc_with_conviction(rn1_notional_usdc, None)
    }

    /// Conviction-aware sizing: if a `conviction_multiplier` is provided it
    /// replaces the flat `SIZE_MULTIPLIER`. Use [`exit_strategy::conviction_multiplier`]
    /// to compute the multiplier from signal metadata + [`FilterConfig`].
    pub fn calculate_size_usdc_with_conviction(
        &self,
        rn1_notional_usdc: f64,
        conviction_mult: Option<f64>,
    ) -> Option<f64> {
        let size_multiplier = match conviction_mult {
            Some(m) => m,
            None => std::env::var("PAPER_SIZE_MULTIPLIER")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(SIZE_MULTIPLIER)
                .max(0.01),
        };
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
            .unwrap_or(5.0)
            .max(min_trade_usdc);

        let raw        = rn1_notional_usdc * size_multiplier;
        let cap_nav    = self.nav() * max_position_pct;
        // No cash reserve — always deploy all available cash to maximise exposure
        let cash_reserve_pct: f64 = std::env::var("CASH_RESERVE_PCT")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let available_cash = (self.cash_usdc - self.nav() * cash_reserve_pct).max(0.0);
        let size       = raw.max(min_floor_usdc).min(cap_nav).min(available_cash);
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
        self.open_position_with_meta(token_id, None, None, side, entry_price, usdc_size, rn1_order_id, 0.0, 0, "A", None, None)
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
        event_start_time: Option<i64>,
        event_end_time: Option<i64>,
    ) -> usize {
        let shares = usdc_size / entry_price;
        let id     = self.next_id;
        self.next_id   += 1;
        let (fee_cat, fee_rate) = detect_fee_category(
            market_title.as_deref().unwrap_or(""),
        );
        let entry_fee = polymarket_taker_fee_with_rate(shares, entry_price, fee_rate);
        self.cash_usdc -= usdc_size + entry_fee;
        // Track entry fee in the global fees counter and per-position for accounting.
        self.total_fees_paid_usdc += entry_fee;
        self.positions.push(PaperPosition {
            id,
            token_id,
            market_title,
            market_outcome,
            side,
            entry_price,
            shares,
            usdc_spent:    usdc_size,
            entry_fee_paid_usdc: entry_fee,
            current_price: entry_price,
            peak_price: entry_price,
            fee_category: fee_cat.to_string(),
            fee_rate,
            opened_at:     Instant::now(),
            rn1_order_id,
            opened_at_wall: chrono::Local::now(),
            entry_slippage_bps,
            queue_delay_ms,
            experiment_variant: experiment_variant.to_string(),
            event_start_time,
            event_end_time,
        });
        self.filled_orders += 1;
        id
    }

    /// Update `current_price` for every position in `token_id`.
    pub fn update_price(&mut self, token_id: &str, new_price: f64) {
        for pos in &mut self.positions {
            if pos.token_id == token_id {
                pos.current_price = new_price;
                if new_price > pos.peak_price {
                    pos.peak_price = new_price;
                }
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
            let exit_fee = polymarket_taker_fee_with_rate(pos.shares, pos.current_price, pos.fee_rate);
            let entry_fee_portion = pos.entry_fee_paid_usdc;
            self.total_fees_paid_usdc += exit_fee;
            self.cash_usdc += pos.usdc_spent + pnl - exit_fee;
            self.closed_trades.push(ClosedTrade {
                token_id: pos.token_id.clone(),
                market_title: pos.market_title.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                exit_price: pos.current_price,
                shares: pos.shares,
                realized_pnl: pnl,
                fees_paid_usdc: entry_fee_portion + exit_fee,
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
                event_start_time: pos.event_start_time,
                event_end_time: pos.event_end_time,
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

    /// Closes any open position whose unrealized P&L has fallen below
    /// `-loss_threshold_pct` percent (e.g. pass `35.0` to cut at -35%).
    ///
    /// Returns the number of positions closed.
    pub fn stop_loss_check(&mut self, loss_threshold_pct: f64) -> usize {
        self.stop_loss_check_tiered(loss_threshold_pct, None, None)
    }

    /// Tiered stop-loss: apply `small_threshold_pct` (tighter) for positions
    /// whose cost basis is below `small_notional_usdc`. Falls back to
    /// `loss_threshold_pct` for larger positions.
    pub fn stop_loss_check_tiered(
        &mut self,
        loss_threshold_pct: f64,
        small_threshold_pct: Option<f64>,
        small_notional_usdc: Option<f64>,
    ) -> usize {
        let mut actions = 0usize;
        let mut i = 0usize;
        while i < self.positions.len() {
            let threshold = match (small_threshold_pct, small_notional_usdc) {
                (Some(tight_pct), Some(notional_cutoff))
                    if self.positions[i].usdc_spent < notional_cutoff =>
                {
                    tight_pct.abs()
                }
                _ => loss_threshold_pct.abs(),
            };
            if self.positions[i].unrealized_pnl_pct() <= -threshold {
                let reason = format!("stop_loss@-{:.0}%", threshold);
                let removed = self.close_position_fraction(i, 1.0, reason);
                actions += 1;
                if !removed {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        actions
    }


    pub fn close_position_fraction(&mut self, idx: usize, fraction: f64, reason: String) -> bool {
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
            let exit_fee = polymarket_taker_fee_with_rate(pos.shares, pos.current_price, pos.fee_rate);
            let entry_fee_portion = pos.entry_fee_paid_usdc;
            // Record exit fee in global counter and settle cash.
            self.total_fees_paid_usdc += exit_fee;
            self.cash_usdc += pos.usdc_spent + pnl - exit_fee;
            self.closed_trades.push(ClosedTrade {
                token_id: pos.token_id.clone(),
                market_title: pos.market_title.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                exit_price: pos.current_price,
                shares: pos.shares,
                realized_pnl: pnl,
                fees_paid_usdc: entry_fee_portion + exit_fee,
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
                event_start_time: pos.event_start_time,
                event_end_time: pos.event_end_time,
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
        let exit_fee = polymarket_taker_fee_with_rate(close_shares, pos.current_price, pos.fee_rate);
        let entry_fee_portion = pos.entry_fee_paid_usdc * fraction;
        // Record exit fee and attribute entry fee portion to this closed slice.
        self.total_fees_paid_usdc += exit_fee;
        self.cash_usdc += close_usdc_spent + pnl - exit_fee;
        self.closed_trades.push(ClosedTrade {
            token_id: pos.token_id.clone(),
            market_title: pos.market_title.clone(),
            side: pos.side,
            entry_price: pos.entry_price,
            exit_price: pos.current_price,
            shares: close_shares,
            realized_pnl: pnl,
            fees_paid_usdc: entry_fee_portion + exit_fee,
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
            event_start_time: pos.event_start_time,
            event_end_time: pos.event_end_time,
        });

        pos.shares -= close_shares;
        pos.usdc_spent -= close_usdc_spent;
        pos.entry_fee_paid_usdc -= entry_fee_portion;
        if pos.shares <= 0.01 || pos.usdc_spent <= 0.001 {
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
    /// Called by the TUI loop every ~150 ms to build the equity curve.
    pub fn push_equity_snapshot(&mut self) {
        let new_nav = self.nav();
        // Only record a new point if NAV has actually changed — avoids 90%+ duplicate entries.
        if let Some(&last) = self.equity_curve.last() {
            if (new_nav - last).abs() < 0.001 {
                return;
            }
        }
        if self.equity_curve.len() >= EQUITY_CURVE_MAX {
            self.equity_curve.remove(0);
            if !self.equity_timestamps.is_empty() {
                self.equity_timestamps.remove(0);
            }
        }
        self.equity_curve.push(new_nav);
        self.equity_timestamps.push(chrono::Utc::now().timestamp_millis());
    }

    pub fn save_to_path(&self, path: &str) -> std::io::Result<()> {
        let persisted = PersistedPaperPortfolio::from(self);
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&persisted)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        // Atomic write: write to .tmp then rename to prevent corruption on hard kill
        let tmp_path = format!("{path}.tmp");
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, path)?;
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
            total_fees_paid_usdc: value.total_fees_paid_usdc,
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
                entry_fee_paid_usdc: p.entry_fee_paid_usdc,
                entry_slippage_bps: p.entry_slippage_bps,
                queue_delay_ms: p.queue_delay_ms,
                experiment_variant: p.experiment_variant.clone(),
                fee_category: p.fee_category.clone(),
                fee_rate: p.fee_rate,
                event_start_time: p.event_start_time,
                event_end_time: p.event_end_time,
            }).collect(),
            closed_trades: value.closed_trades.iter().map(|t| PersistedClosedTrade {
                token_id: t.token_id.clone(),
                market_title: t.market_title.clone(),
                side: t.side,
                entry_price: t.entry_price,
                exit_price: t.exit_price,
                shares: t.shares,
                realized_pnl: t.realized_pnl,
                fees_paid_usdc: t.fees_paid_usdc,
                reason: t.reason.clone(),
                opened_at_wall_ms: t.opened_at_wall.timestamp_millis(),
                closed_at_wall_ms: t.closed_at_wall.timestamp_millis(),
                duration_secs: t.duration_secs,
                scorecard: t.scorecard.clone(),
                event_start_time: t.event_start_time,
                event_end_time: t.event_end_time,
            }).collect(),
            total_signals: value.total_signals,
            filled_orders: value.filled_orders,
            aborted_orders: value.aborted_orders,
            skipped_orders: value.skipped_orders,
            equity_curve: value.equity_curve.clone(),
            equity_timestamps: value.equity_timestamps.clone(),
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
                peak_price: p.current_price.max(p.entry_price),
                entry_fee_paid_usdc: p.entry_fee_paid_usdc,
                opened_at,
                rn1_order_id: p.rn1_order_id,
                opened_at_wall,
                entry_slippage_bps: p.entry_slippage_bps,
                queue_delay_ms: p.queue_delay_ms,
                experiment_variant: if p.experiment_variant.is_empty() { "A".to_string() } else { p.experiment_variant },
                fee_rate: if p.fee_rate == 0.0 && p.fee_category.is_empty() { 0.0001 } else { p.fee_rate },
                fee_category: if p.fee_category.is_empty() { "other".to_string() } else { p.fee_category },
                event_start_time: p.event_start_time,
                event_end_time: p.event_end_time,
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
                market_title: t.market_title,
                side: t.side,
                entry_price: t.entry_price,
                exit_price: t.exit_price,
                shares: t.shares,
                realized_pnl: t.realized_pnl,
                fees_paid_usdc: t.fees_paid_usdc,
                reason: t.reason,
                opened_at_wall,
                closed_at_wall,
                duration_secs: t.duration_secs,
                scorecard: t.scorecard,
                event_start_time: t.event_start_time,
                event_end_time: t.event_end_time,
            }
        }).collect();

        Self {
            cash_usdc: value.cash_usdc,
            total_fees_paid_usdc: value.total_fees_paid_usdc,
            positions,
            closed_trades,
            total_signals: value.total_signals,
            filled_orders: value.filled_orders,
            aborted_orders: value.aborted_orders,
            skipped_orders: value.skipped_orders,
            equity_curve: value.equity_curve,
            equity_timestamps: value.equity_timestamps,
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
    fn size_capped_at_10_pct_nav() {
        let p = PaperPortfolio::new(); // NAV = 100, cap = 12% = 12
        // RN1 trades $20,000 → 5% = $1,000, capped at $12
        let size = p.calculate_size_usdc(20_000.0).unwrap();
        let max_position = p.nav() * MAX_POSITION_PCT;
        assert!((size - max_position).abs() < 1e-9, "size={size} expected cap={max_position}");
    }

    #[test]
    fn size_below_minimum_returns_none() {
        let p = PaperPortfolio::new();
        // 5% of $100 = $5, which meets the floor, so should size.
        assert!(p.calculate_size_usdc(100.0).is_some());
    }

    #[test]
    fn nav_decreases_after_open() {
        let mut p = PaperPortfolio::new();
        p.open_position("tok".into(), OrderSide::Buy, 0.65, 20.0, "oid".into());
        // cash = 100 - 20 - entry_fee; position worth 20 at entry → NAV ≈ 100 - entry_fee
        // Entry fee = shares * rate * p * (1-p) where shares = 20/0.65, rate = 0.004
        let shares = 20.0 / 0.65;
        let entry_fee = polymarket_taker_fee(shares, 0.65);
        assert!((p.nav() - (100.0 - entry_fee)).abs() < 0.01,
            "nav={} expected={}", p.nav(), 100.0 - entry_fee);
        assert!((p.cash_usdc - (80.0 - entry_fee)).abs() < 0.01,
            "cash={} expected={}", p.cash_usdc, 80.0 - entry_fee);
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
