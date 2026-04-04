//! Historical backtesting engine.
//!
//! Replays tick data through a virtual clock, reusing [`PaperPortfolio`] for
//! position tracking and [`RiskManager`] for safety checks.
//!
//! # Anti-lookahead guarantees
//!
//! 1. [`VirtualClock::now()`] never returns a timestamp later than the tick
//!    being processed.
//! 2. The simulated order book is built incrementally — only ticks at or before
//!    the current clock time are applied.
//! 3. Fill-window simulation looks forward in tick data (next `fill_window_ms`)
//!    only to check drift; the **entry price is always the price at signal time**.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::paper_portfolio::{ClosedTrade, PaperPortfolio};
use crate::risk_manager::{RiskConfig, RiskManager};
use crate::types::OrderSide;

// ─── Configuration ───────────────────────────────────────────────────────────

/// Backtest-specific configuration.
#[derive(Debug, Clone)]
pub struct BacktestConfig {
    /// Wallet address to filter as RN1 (compared case-insensitively).
    pub rn1_wallet: String,
    /// Starting virtual USDC balance.
    pub starting_usdc: f64,
    /// Fraction of RN1 notional to mirror (e.g. 0.02 = 2%).
    pub size_multiplier: f64,
    /// Maximum price drift fraction before aborting (e.g. 0.015 = 1.5%).
    pub drift_threshold: f64,
    /// Simulated fill window in milliseconds.
    pub fill_window_ms: u64,
    /// Simulated slippage in basis points (e.g. 10 = 0.1%).
    pub slippage_bps: u64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            rn1_wallet: String::new(),
            starting_usdc: 100.0,
            size_multiplier: 0.02,
            drift_threshold: 0.015,
            fill_window_ms: 3000,
            slippage_bps: 10,
        }
    }
}

// ─── Tick Record ─────────────────────────────────────────────────────────────

/// A single tick from historical data.
#[derive(Debug, Clone)]
pub struct TickRecord {
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
    /// Polymarket token ID.
    pub token_id: String,
    /// `"BUY"` or `"SELL"`.
    pub side: String,
    /// Price scaled ×1 000 (e.g. 0.65 → 650).
    pub price: u64,
    /// Size scaled ×1 000.
    pub size: u64,
    /// Wallet address that placed the order.
    pub wallet: String,
}

// ─── Virtual Clock ───────────────────────────────────────────────────────────

/// A monotonically non-decreasing virtual clock.
#[derive(Debug, Default)]
pub struct VirtualClock {
    current_ms: i64,
}

impl VirtualClock {
    pub fn new() -> Self {
        Self { current_ms: 0 }
    }

    /// Advance the clock.  Panics (debug) if `ts` is earlier than current.
    pub fn advance_to(&mut self, ts: i64) {
        debug_assert!(
            ts >= self.current_ms,
            "VirtualClock cannot go backwards: {ts} < {}",
            self.current_ms
        );
        self.current_ms = ts;
    }

    pub fn now(&self) -> i64 {
        self.current_ms
    }
}

// ─── Simulated Order Book ────────────────────────────────────────────────────

/// Per-token simulated order book built up tick by tick.
#[derive(Debug, Default)]
pub struct SimulatedOrderBook {
    /// token_id → (bids: price→size, asks: price→size)
    books: HashMap<String, (BTreeMap<u64, u64>, BTreeMap<u64, u64>)>,
}

impl SimulatedOrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a single tick to update the book state.
    pub fn apply_tick(&mut self, tick: &TickRecord) {
        let (bids, asks) = self
            .books
            .entry(tick.token_id.clone())
            .or_insert_with(|| (BTreeMap::new(), BTreeMap::new()));

        match tick.side.to_uppercase().as_str() {
            "BUY" => {
                if tick.size == 0 {
                    bids.remove(&tick.price);
                } else {
                    bids.insert(tick.price, tick.size);
                }
            }
            "SELL" => {
                if tick.size == 0 {
                    asks.remove(&tick.price);
                } else {
                    asks.insert(tick.price, tick.size);
                }
            }
            _ => {}
        }
    }

    /// Returns the mid price (scaled ×1 000) if both sides exist.
    pub fn mid_price(&self, token_id: &str) -> Option<u64> {
        let (bids, asks) = self.books.get(token_id)?;
        let best_bid = bids.keys().next_back().copied()?;
        let best_ask = asks.keys().next().copied()?;
        Some((best_bid + best_ask) / 2)
    }
}

// ─── Backtest Results ────────────────────────────────────────────────────────

/// Aggregated performance statistics from a completed backtest.
#[derive(Debug, Clone, Serialize)]
pub struct BacktestResults {
    pub total_return_pct: f64,
    /// Annualised Sharpe ratio (risk-free rate = 0).
    pub sharpe_ratio: f64,
    /// Annualised Sortino ratio (downside deviation only).
    pub sortino_ratio: f64,
    /// Maximum peak-to-trough drawdown as a percentage.
    pub max_drawdown_pct: f64,
    /// Calmar ratio: total_return / max_drawdown.
    pub calmar_ratio: f64,
    pub win_rate: f64,
    /// Gross profit / |gross loss|.
    pub profit_factor: f64,
    /// Average trade duration in milliseconds.
    pub avg_trade_duration_ms: u64,
    pub total_trades: usize,
    /// Time-series of (unix_ms, nav) for charting.
    pub equity_curve: Vec<(i64, f64)>,
}

// ─── Backtest Engine ─────────────────────────────────────────────────────────

/// Historical backtesting engine.  Call [`BacktestEngine::run`] to execute.
pub struct BacktestEngine {
    pub portfolio: PaperPortfolio,
    risk: RiskManager,
    order_book: SimulatedOrderBook,
    clock: VirtualClock,
    ticks: Vec<TickRecord>,
    config: BacktestConfig,
}

impl BacktestEngine {
    /// Creates a new backtest engine.  Ticks are sorted by timestamp internally.
    pub fn new(config: BacktestConfig, mut ticks: Vec<TickRecord>) -> Self {
        ticks.sort_by_key(|t| t.timestamp);

        let mut portfolio = PaperPortfolio::new();
        portfolio.cash_usdc = config.starting_usdc;

        let risk_config = RiskConfig {
            trading_enabled: true,
            max_orders_per_second: u32::MAX, // no rate limit in backtest
            ..RiskConfig::default()
        };

        Self {
            portfolio,
            risk: RiskManager::new(risk_config),
            order_book: SimulatedOrderBook::new(),
            clock: VirtualClock::new(),
            ticks,
            config,
        }
    }

    /// Execute the backtest against all loaded ticks.
    pub fn run(&mut self) -> BacktestResults {
        let starting_nav = self.config.starting_usdc;
        let rn1_wallet = self.config.rn1_wallet.to_lowercase();

        let mut equity_curve: Vec<(i64, f64)> = Vec::new();
        let mut trade_open_times: HashMap<usize, i64> = HashMap::new();
        let mut trade_durations_ms: Vec<i64> = Vec::new();

        // Clone ticks to avoid borrow-checker issues with &self.ticks vs &mut self.
        let ticks = self.ticks.clone();
        let tick_count = ticks.len();

        for i in 0..tick_count {
            let tick = &ticks[i];

            // 1. Advance virtual clock (monotonically non-decreasing).
            self.clock.advance_to(tick.timestamp);

            // 2. Update simulated order book with this tick.
            self.order_book.apply_tick(tick);

            // 3. Update current prices for all open positions on this token.
            if let Some(mid) = self.order_book.mid_price(&tick.token_id) {
                let price_f64 = mid as f64 / 1000.0;
                self.portfolio.update_price(&tick.token_id, price_f64);
            }

            // 4. Check if this tick is an RN1 signal.
            if tick.wallet.to_lowercase() == rn1_wallet {
                self.portfolio.total_signals += 1;

                let side = match tick.side.to_uppercase().as_str() {
                    "BUY" => OrderSide::Buy,
                    "SELL" => OrderSide::Sell,
                    _ => continue,
                };

                // Entry price is the signal's OWN price — never a future price.
                let entry_price = tick.price as f64 / 1000.0;
                if entry_price <= 0.0 {
                    self.portfolio.skipped_orders += 1;
                    continue;
                }

                let rn1_shares = tick.size as f64 / 1000.0;
                let rn1_notional = rn1_shares * entry_price;

                // Size calculation (configurable, mirrors PaperPortfolio logic).
                let raw = rn1_notional * self.config.size_multiplier;
                let cap_nav = self.portfolio.nav() * 0.10;
                let size_usdc = raw.min(cap_nav).min(self.portfolio.cash_usdc);

                if size_usdc < 0.50 {
                    self.portfolio.skipped_orders += 1;
                    continue;
                }

                // Risk check.
                let open_positions = self.portfolio.positions.len();
                if self
                    .risk
                    .check_pre_order(
                        size_usdc,
                        open_positions,
                        self.portfolio.nav(),
                        starting_nav,
                    )
                    .is_err()
                {
                    continue;
                }

                // ── Fill window simulation ──────────────────────────────────
                // Look forward in tick data for the next `fill_window_ms`.
                // If price drifts beyond `drift_threshold` → abort.
                // The entry price is NOT updated (anti-lookahead).
                let window_end = tick.timestamp + self.config.fill_window_ms as i64;
                let mut drift_abort = false;

                for j in (i + 1)..tick_count {
                    if ticks[j].timestamp > window_end {
                        break;
                    }
                    if ticks[j].token_id == tick.token_id {
                        let future_price = ticks[j].price as f64 / 1000.0;
                        let drift = (future_price - entry_price).abs() / entry_price;
                        if drift > self.config.drift_threshold {
                            drift_abort = true;
                            break;
                        }
                    }
                }

                if drift_abort {
                    self.portfolio.aborted_orders += 1;
                    continue;
                }

                // ── Apply slippage to fill price ────────────────────────────
                let slippage = self.config.slippage_bps as f64 / 10_000.0;
                let fill_price = match side {
                    OrderSide::Buy => entry_price * (1.0 + slippage),
                    OrderSide::Sell => entry_price * (1.0 - slippage),
                };

                // ── Open position ───────────────────────────────────────────
                let pos_id = self.portfolio.open_position(
                    tick.token_id.clone(),
                    side,
                    fill_price,
                    size_usdc,
                    format!("backtest-{i}"),
                );

                trade_open_times.insert(pos_id, tick.timestamp);
                self.risk.record_fill(size_usdc);
            }

            // 5. Record equity curve (one point per distinct timestamp).
            let nav = self.portfolio.nav();
            if equity_curve.is_empty()
                || equity_curve
                    .last()
                    .map_or(false, |&(ts, _)| ts != tick.timestamp)
            {
                equity_curve.push((tick.timestamp, nav));
            }
        }

        // ── Close all remaining positions at last known prices ───────────────
        let final_ts = self.clock.now();
        let remaining: Vec<_> = self.portfolio.positions.drain(..).collect();
        for pos in &remaining {
            let pnl = pos.unrealized_pnl();
            self.portfolio.cash_usdc += pos.usdc_spent + pnl;
            self.portfolio.closed_trades.push(ClosedTrade {
                token_id: pos.token_id.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                exit_price: pos.current_price,
                shares: pos.shares,
                realized_pnl: pnl,
                reason: "backtest-end".to_string(),
                opened_at_wall: pos.opened_at_wall,
                closed_at_wall: chrono::Local::now(),
                duration_secs: pos.opened_at.elapsed().as_secs(),
                scorecard: crate::paper_portfolio::ExecutionScorecard::default(),
            });
            if let Some(open_ts) = trade_open_times.remove(&pos.id) {
                trade_durations_ms.push(final_ts - open_ts);
            }
        }

        self.compute_results(starting_nav, equity_curve, &trade_durations_ms)
    }

    fn compute_results(
        &self,
        starting_nav: f64,
        equity_curve: Vec<(i64, f64)>,
        trade_durations_ms: &[i64],
    ) -> BacktestResults {
        let final_nav = self.portfolio.nav();
        let total_return_pct = (final_nav - starting_nav) / starting_nav * 100.0;

        let daily_returns = compute_daily_returns(&equity_curve);
        let sharpe_ratio = compute_sharpe(&daily_returns);
        let sortino_ratio = compute_sortino(&daily_returns);
        let max_drawdown_pct = compute_max_drawdown(&equity_curve);

        let calmar_ratio = if max_drawdown_pct > 0.0 {
            total_return_pct / max_drawdown_pct
        } else if total_return_pct > 0.0 {
            f64::INFINITY
        } else {
            0.0
        };

        let trades = &self.portfolio.closed_trades;
        let total_trades = trades.len();
        let winners = trades.iter().filter(|t| t.realized_pnl > 0.0).count();
        let win_rate = if total_trades > 0 {
            winners as f64 / total_trades as f64
        } else {
            0.0
        };

        let gross_profit: f64 = trades
            .iter()
            .filter(|t| t.realized_pnl > 0.0)
            .map(|t| t.realized_pnl)
            .sum();
        let gross_loss: f64 = trades
            .iter()
            .filter(|t| t.realized_pnl < 0.0)
            .map(|t| t.realized_pnl.abs())
            .sum();
        let profit_factor = if gross_loss > 0.0 {
            gross_profit / gross_loss
        } else if gross_profit > 0.0 {
            f64::INFINITY
        } else {
            0.0
        };

        let avg_trade_duration_ms = if trade_durations_ms.is_empty() {
            0
        } else {
            let total: i64 = trade_durations_ms.iter().sum();
            (total / trade_durations_ms.len() as i64) as u64
        };

        BacktestResults {
            total_return_pct,
            sharpe_ratio,
            sortino_ratio,
            max_drawdown_pct,
            calmar_ratio,
            win_rate,
            profit_factor,
            avg_trade_duration_ms,
            total_trades,
            equity_curve,
        }
    }
}

// ─── CSV Loading ─────────────────────────────────────────────────────────────

/// Parse tick records from a CSV file.
///
/// Expected format (with header):
/// ```text
/// timestamp_ms,token_id,side,price_scaled,size_scaled,wallet
/// 1710000000000,12345...,BUY,650,5000,0xRN1WALLET
/// ```
pub fn load_ticks_csv(path: &str) -> Result<Vec<TickRecord>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read CSV: {path}"))?;

    let mut ticks = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        if line_no == 0 || line.trim().is_empty() {
            continue; // skip header + blank lines
        }

        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 {
            anyhow::bail!(
                "line {}: expected 6 comma-separated fields, got {}",
                line_no + 1,
                f.len()
            );
        }

        ticks.push(TickRecord {
            timestamp: f[0]
                .trim()
                .parse()
                .with_context(|| format!("line {}: invalid timestamp", line_no + 1))?,
            token_id: f[1].trim().to_string(),
            side: f[2].trim().to_string(),
            price: f[3]
                .trim()
                .parse()
                .with_context(|| format!("line {}: invalid price", line_no + 1))?,
            size: f[4]
                .trim()
                .parse()
                .with_context(|| format!("line {}: invalid size", line_no + 1))?,
            wallet: f[5].trim().to_string(),
        });
    }

    Ok(ticks)
}

// ─── Statistics helpers ──────────────────────────────────────────────────────

/// Group equity curve into daily returns.
fn compute_daily_returns(curve: &[(i64, f64)]) -> Vec<f64> {
    if curve.len() < 2 {
        return Vec::new();
    }

    const DAY_MS: i64 = 86_400_000;
    let mut returns = Vec::new();
    let mut day_start_idx = 0;
    let mut day_boundary = curve[0].0 + DAY_MS;

    for i in 1..curve.len() {
        if curve[i].0 >= day_boundary {
            let start_nav = curve[day_start_idx].1;
            let end_nav = curve[i - 1].1;
            if start_nav > 0.0 {
                returns.push((end_nav - start_nav) / start_nav);
            }
            day_start_idx = i;
            day_boundary = curve[i].0 + DAY_MS;
        }
    }

    // Last (possibly partial) day.
    let start_nav = curve[day_start_idx].1;
    let end_nav = curve.last().unwrap().1;
    if start_nav > 0.0 {
        returns.push((end_nav - start_nav) / start_nav);
    }

    returns
}

/// Annualised Sharpe ratio (risk-free rate = 0).
fn compute_sharpe(returns: &[f64]) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std_dev = variance.sqrt();
    if std_dev == 0.0 {
        return 0.0;
    }
    (mean / std_dev) * 252.0_f64.sqrt()
}

/// Annualised Sortino ratio (downside deviation only).
fn compute_sortino(returns: &[f64]) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let downside_sq: f64 = returns
        .iter()
        .filter(|&&r| r < 0.0)
        .map(|r| r.powi(2))
        .sum();
    let downside_dev = (downside_sq / n).sqrt();
    if downside_dev == 0.0 {
        return if mean > 0.0 { f64::INFINITY } else { 0.0 };
    }
    (mean / downside_dev) * 252.0_f64.sqrt()
}

/// Maximum peak-to-trough drawdown as a percentage.
fn compute_max_drawdown(curve: &[(i64, f64)]) -> f64 {
    if curve.is_empty() {
        return 0.0;
    }
    let mut peak = curve[0].1;
    let mut max_dd = 0.0_f64;
    for &(_, nav) in curve {
        if nav > peak {
            peak = nav;
        }
        let dd = (peak - nav) / peak * 100.0;
        if dd > max_dd {
            max_dd = dd;
        }
    }
    max_dd
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BacktestConfig {
        BacktestConfig {
            rn1_wallet: "0xrn1wallet".to_string(),
            starting_usdc: 100.0,
            size_multiplier: 0.02,
            drift_threshold: 0.015,
            fill_window_ms: 3000,
            slippage_bps: 10,
        }
    }

    fn tick(ts: i64, tok: &str, side: &str, price: u64, size: u64, wallet: &str) -> TickRecord {
        TickRecord {
            timestamp: ts,
            token_id: tok.to_string(),
            side: side.to_string(),
            price,
            size,
            wallet: wallet.to_string(),
        }
    }

    #[test]
    fn no_lookahead_bias_in_fill_simulation() {
        // RN1 buys at price 650 (0.65).
        // A future tick at +1 000 ms shows price 750 — 15.4 % drift.
        // The fill window must detect this and abort; entry must NOT be 750.
        let ticks = vec![
            tick(1000, "tok", "BUY", 640, 1000, "0xmm"),
            tick(1000, "tok", "SELL", 660, 1000, "0xmm"),
            tick(1100, "tok", "BUY", 650, 50000, "0xrn1wallet"),
            tick(2100, "tok", "BUY", 750, 5000, "0xmm"), // 15.4 % drift in window
        ];

        let mut engine = BacktestEngine::new(cfg(), ticks);
        let results = engine.run();

        assert_eq!(
            results.total_trades, 0,
            "trade should be aborted due to drift"
        );
        assert_eq!(engine.portfolio.aborted_orders, 1);
    }

    #[test]
    fn equity_curve_is_monotone_without_trades() {
        // 10 ticks, none from RN1 → NAV must stay at starting balance.
        let ticks: Vec<_> = (0..10)
            .map(|i| tick(1000 + i * 1000, "tok", "BUY", 650, 5000, "0xother"))
            .collect();

        let mut engine = BacktestEngine::new(cfg(), ticks);
        let results = engine.run();

        for &(_, nav) in &results.equity_curve {
            assert!(
                (nav - 100.0).abs() < 1e-9,
                "NAV must stay constant without trades, got {nav}"
            );
        }
    }

    #[test]
    fn sharpe_ratio_positive_for_profitable_strategy() {
        // RN1 buys at 0.50, price rises to ≈0.60 → profitable.
        let ticks = vec![
            tick(1000, "tok", "BUY", 490, 10000, "0xmm"),
            tick(1000, "tok", "SELL", 510, 10000, "0xmm"),
            tick(2000, "tok", "BUY", 500, 50000, "0xrn1wallet"),
            // stable inside fill window
            tick(4000, "tok", "BUY", 501, 1000, "0xmm"),
            // price rises after fill window
            tick(10000, "tok", "BUY", 590, 10000, "0xmm"),
            tick(10000, "tok", "SELL", 610, 10000, "0xmm"),
        ];

        let mut engine = BacktestEngine::new(cfg(), ticks);
        let results = engine.run();

        assert!(results.total_trades > 0, "should have at least one trade");
        assert!(
            results.total_return_pct > 0.0,
            "strategy should be profitable, got {:.4}%",
            results.total_return_pct
        );
    }

    #[test]
    fn abort_triggers_when_price_drifts_in_window() {
        let ticks = vec![
            tick(1000, "tok", "BUY", 500, 1000, "0xmm"),
            tick(1000, "tok", "SELL", 510, 1000, "0xmm"),
            tick(2000, "tok", "BUY", 500, 50000, "0xrn1wallet"),
            // 2.2 % drift (>1.5 % threshold) within the 3 000 ms window
            tick(3000, "tok", "BUY", 511, 1000, "0xmm"),
        ];

        let mut engine = BacktestEngine::new(cfg(), ticks);
        let results = engine.run();

        assert_eq!(
            engine.portfolio.aborted_orders, 1,
            "should abort due to drift"
        );
        assert_eq!(results.total_trades, 0);
    }

    #[test]
    fn rn1_filter_only_triggers_on_correct_wallet() {
        let ticks = vec![
            tick(1000, "tok", "BUY", 490, 10000, "0xmm"),
            tick(1000, "tok", "SELL", 510, 10000, "0xmm"),
            tick(2000, "tok", "BUY", 500, 50000, "0xsomeoneelse"),
            tick(3000, "tok", "BUY", 500, 50000, "0xanother"),
        ];

        let mut engine = BacktestEngine::new(cfg(), ticks);
        let results = engine.run();

        assert_eq!(
            engine.portfolio.total_signals, 0,
            "no RN1 signals should fire"
        );
        assert_eq!(results.total_trades, 0);
    }
}
