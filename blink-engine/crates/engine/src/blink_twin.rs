//! Blink Twin — The Self-Improving Adversarial Digital Twin.
//!
//! A shadow engine that mirrors live signals but simulates "worst-case" execution:
//! - Added network/processing latency.
//! - Increased slippage/market impact.
//! - Aggressive drift aborts.
//!
//! The Twin continually evaluates its own performance and mutates its
//! parameters (latency, slippage, drift) to find the boundary of profitability.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::info;

use crate::activity_log::{push as log_push, ActivityLog, EntryKind};
use crate::order_book::OrderBookStore;
use crate::paper_engine::parse_autoclaim_tiers;
use crate::paper_portfolio::{
    ExecutionScorecard, PaperPortfolio, DRIFT_THRESHOLD, STARTING_BALANCE_USDC,
};
use crate::types::{OrderSide, RN1Signal};

/// Configuration for the adversarial simulation.
#[derive(Debug, Clone)]
pub struct TwinConfig {
    /// Extra latency in milliseconds to add to every signal.
    pub extra_latency_ms: u64,
    /// Penalty in basis points to add to the observed slippage.
    pub slippage_penalty_bps: f64,
    /// Multiplier for the drift threshold (e.g. 0.8 means it aborts 20% earlier).
    pub drift_multiplier: f64,
    /// Generation / Iteration number for self-improvement.
    pub generation: u32,
}

impl Default for TwinConfig {
    fn default() -> Self {
        Self {
            extra_latency_ms: 100, // Pessimistic start
            slippage_penalty_bps: 10.0,
            drift_multiplier: 0.90,
            generation: 1,
        }
    }
}

pub struct BlinkTwin {
    pub portfolio: Arc<Mutex<PaperPortfolio>>,
    book_store: Arc<OrderBookStore>,
    activity: Option<ActivityLog>,
    pub config: Arc<Mutex<TwinConfig>>,
}

#[derive(Debug, Clone, Default)]
pub struct TwinSnapshot {
    pub generation: u32,
    pub extra_latency_ms: u64,
    pub slippage_penalty_bps: f64,
    pub drift_multiplier: f64,
    pub nav: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub total_signals: usize,
    pub filled_orders: usize,
    pub aborted_orders: usize,
    pub skipped_orders: usize,
    pub open_positions: usize,
    pub closed_trades: usize,
    pub win_rate_pct: f64,
    pub equity_curve: Vec<f64>,
    pub max_drawdown_pct: f64,
    pub high_water_mark: f64,
    pub nav_return_pct: f64,
}

impl BlinkTwin {
    pub fn new(book_store: Arc<OrderBookStore>, activity: Option<ActivityLog>) -> Arc<Self> {
        let config = TwinConfig::default();

        info!(
            latency = config.extra_latency_ms,
            slippage = config.slippage_penalty_bps,
            "Blink Twin initialized (Self-Improving Shadow Mode)"
        );

        if let Some(ref log) = activity {
            log_push(
                log,
                EntryKind::Engine,
                format!(
                    "Blink Twin Gen 1 active: +{}ms lat, +{:.1}bps slip",
                    config.extra_latency_ms, config.slippage_penalty_bps
                ),
            );
        }

        let twin = Arc::new(Self {
            portfolio: Arc::new(Mutex::new(PaperPortfolio::new())),
            book_store,
            activity,
            config: Arc::new(Mutex::new(config)),
        });

        // Spawn the self-improvement loop
        Self::spawn_optimizer(Arc::clone(&twin));

        twin
    }

    /// Background task that evaluates Twin's performance and mutates parameters.
    fn spawn_optimizer(twin: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                // Evaluate every 60 seconds
                sleep(Duration::from_secs(60)).await;
                twin.run_autoclaim().await;

                let (win_rate, trades_count, pnl) = {
                    let p = twin.portfolio.lock().await;
                    if p.closed_trades.is_empty() {
                        continue;
                    }
                    let wins = p
                        .closed_trades
                        .iter()
                        .filter(|t| t.realized_pnl > 0.0)
                        .count();
                    let win_rate = wins as f64 / p.closed_trades.len() as f64;
                    let trades = p.closed_trades.len();
                    (win_rate, trades, p.realized_pnl())
                };

                // Only optimize if we have enough new data
                if trades_count < 5 {
                    continue;
                }

                let mut cfg = twin.config.lock().await;
                cfg.generation += 1;

                // Simple Evolutionary Logic:
                // If the twin is making money despite the penalties, make the penalties harder
                // to find the exact breaking point. If it's losing too much, ease up slightly
                // to find the realistic baseline.
                if pnl > 0.0 && win_rate > 0.50 {
                    // Too profitable -> make it harder
                    cfg.extra_latency_ms += 20;
                    cfg.slippage_penalty_bps += 2.0;
                    cfg.drift_multiplier *= 0.95; // stricter
                } else if pnl < -10.0 {
                    // Losing heavily -> make it slightly easier
                    cfg.extra_latency_ms = cfg.extra_latency_ms.saturating_sub(10);
                    cfg.slippage_penalty_bps = (cfg.slippage_penalty_bps - 1.0).max(0.0);
                    cfg.drift_multiplier = (cfg.drift_multiplier * 1.05).min(1.0);
                }

                let msg = format!(
                    "Twin Gen {} Evolved: PnL {:.2}, Win {:.1}%. New Params: +{}ms lat, +{:.1}bps slip, {:.2} drift",
                    cfg.generation, pnl, win_rate * 100.0, cfg.extra_latency_ms, cfg.slippage_penalty_bps, cfg.drift_multiplier
                );

                if let Some(ref log) = twin.activity {
                    log_push(log, EntryKind::Engine, msg.clone());
                }
                info!("{}", msg);

                // Auto-claim profit to reset the evaluation somewhat (optional, relies on PaperPortfolio)
                let mut p = twin.portfolio.lock().await;
                p.autoclaim_take_profit(100.0); // just to keep positions rotating if highly profitable
                p.push_equity_snapshot();
            }
        });
    }

    /// Process a signal through the adversarial lens.
    pub async fn handle_signal(&self, signal: RN1Signal) {
        self.run_autoclaim().await;
        let current_cfg = self.config.lock().await.clone();

        // 1. Simulate extra adversarial latency
        if current_cfg.extra_latency_ms > 0 {
            sleep(Duration::from_millis(current_cfg.extra_latency_ms)).await;
        }

        // 2. Pricing logic
        let entry_price = signal.price as f64 / 1_000.0;
        let rn1_shares = signal.size as f64 / 1_000.0;
        let rn1_notional_usd = rn1_shares * entry_price;

        // 3. Sizing (independent virtual portfolio)
        let size_usdc = {
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            p.calculate_size_usdc(rn1_notional_usd)
        };

        let size_usdc = match size_usdc {
            Some(s) => s,
            None => return, // Skipped due to size/cash
        };

        // 4. Adversarial Fill Window Check
        let twin_drift_threshold = DRIFT_THRESHOLD * current_cfg.drift_multiplier;

        let filled = self
            .check_fill_window_adversarial(&signal.token_id, entry_price, twin_drift_threshold)
            .await;

        if !filled {
            let mut p = self.portfolio.lock().await;
            p.aborted_orders += 1;
            p.push_equity_snapshot();
            return; // Silent abort to not spam logs
        }

        // 5. Apply Slippage Penalty
        let penalty_factor = 1.0 + (current_cfg.slippage_penalty_bps / 10_000.0);
        let pessimistic_price = match signal.side {
            OrderSide::Buy => entry_price * penalty_factor,
            OrderSide::Sell => entry_price / penalty_factor,
        };

        // 6. Record virtual fill
        {
            let mut p = self.portfolio.lock().await;
            p.open_position_with_meta(
                signal.token_id.clone(),
                signal.market_title.clone(),
                signal.market_outcome.clone(),
                signal.side,
                pessimistic_price,
                size_usdc,
                signal.order_id.clone(),
                current_cfg.slippage_penalty_bps,
                current_cfg.extra_latency_ms,
                &format!("TWIN_G{}", current_cfg.generation),
            );
            p.push_equity_snapshot();
        }
    }

    async fn check_fill_window_adversarial(
        &self,
        token_id: &str,
        entry_price: f64,
        threshold: f64,
    ) -> bool {
        // Simulating a fast fill window for the twin (aggressive check)
        for _ in 0..4 {
            sleep(Duration::from_millis(500)).await;
            if let Some(current) = self.get_market_price(token_id) {
                let drift = (current - entry_price).abs() / entry_price;
                if drift > threshold {
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

    async fn run_autoclaim(&self) {
        let enabled = std::env::var("AUTOCLAIM_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);
        if !enabled {
            return;
        }

        let tiers = std::env::var("TWIN_AUTOCLAIM_TIERS")
            .ok()
            .map(parse_tiers_from_raw)
            .unwrap_or_else(parse_autoclaim_tiers);

        let mut p = self.portfolio.lock().await;
        if p.positions.is_empty() {
            return;
        }

        let token_prices: Vec<(String, f64)> = p
            .positions
            .iter()
            .filter_map(|pos| {
                self.get_market_price(&pos.token_id)
                    .map(|pr| (pos.token_id.clone(), pr))
            })
            .collect();
        for (token_id, price) in token_prices {
            p.update_price(&token_id, price);
        }

        let stale_indexes: Vec<usize> = p
            .positions
            .iter()
            .enumerate()
            .filter_map(|(idx, pos)| {
                if self.get_market_price(&pos.token_id).is_none() {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();
        for idx in stale_indexes.into_iter().rev() {
            let pos = p.positions.remove(idx);
            let pnl = match pos.side {
                OrderSide::Buy => (pos.current_price - pos.entry_price) * pos.shares,
                OrderSide::Sell => (pos.entry_price - pos.current_price) * pos.shares,
            };
            p.cash_usdc += pos.usdc_spent + pnl;
            p.closed_trades.push(crate::paper_portfolio::ClosedTrade {
                token_id: pos.token_id.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                exit_price: pos.current_price,
                shares: pos.shares,
                realized_pnl: pnl,
                reason: "twin_autoclaim@market_not_live".to_string(),
                opened_at_wall: pos.opened_at_wall,
                closed_at_wall: chrono::Local::now(),
                duration_secs: pos.opened_at.elapsed().as_secs(),
                scorecard: ExecutionScorecard {
                    slippage_bps: pos.entry_slippage_bps,
                    queue_delay_ms: pos.queue_delay_ms,
                    outcome_tags: vec![
                        "market_not_live".to_string(),
                        format!("variant:{}", pos.experiment_variant),
                    ],
                },
            });
        }

        let closed = p.autoclaim_tiered(&tiers);
        p.push_equity_snapshot();
        if closed > 0 {
            let msg = format!("Blink Twin autoclaim: {} tiered close action(s)", closed);
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Engine, msg.clone());
            }
            info!("{msg}");
        }
    }

    pub async fn snapshot(&self) -> TwinSnapshot {
        let cfg = self.config.lock().await.clone();
        let p = self.portfolio.lock().await;
        let wins = p
            .closed_trades
            .iter()
            .filter(|t| t.realized_pnl > 0.0)
            .count();
        let win_rate_pct = if p.closed_trades.is_empty() {
            0.0
        } else {
            (wins as f64 / p.closed_trades.len() as f64) * 100.0
        };

        let mut equity_curve = p.equity_curve.clone();
        if equity_curve.is_empty() {
            equity_curve.push(p.nav());
        }
        let start_nav = equity_curve
            .first()
            .copied()
            .unwrap_or(STARTING_BALANCE_USDC);
        let nav_return_pct = if start_nav > 0.0 {
            ((p.nav() - start_nav) / start_nav) * 100.0
        } else {
            0.0
        };

        TwinSnapshot {
            generation: cfg.generation,
            extra_latency_ms: cfg.extra_latency_ms,
            slippage_penalty_bps: cfg.slippage_penalty_bps,
            drift_multiplier: cfg.drift_multiplier,
            nav: p.nav(),
            realized_pnl: p.realized_pnl(),
            unrealized_pnl: p.unrealized_pnl(),
            total_signals: p.total_signals,
            filled_orders: p.filled_orders,
            aborted_orders: p.aborted_orders,
            skipped_orders: p.skipped_orders,
            open_positions: p.positions.len(),
            closed_trades: p.closed_trades.len(),
            win_rate_pct,
            equity_curve,
            max_drawdown_pct: p.max_drawdown_pct(),
            high_water_mark: p.high_water_mark(),
            nav_return_pct,
        }
    }
}

fn parse_tiers_from_raw(raw: String) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = raw
        .split(',')
        .filter_map(|item| {
            let mut parts = item.split(':');
            let a = parts.next()?.trim().parse::<f64>().ok()?;
            let b = parts.next()?.trim().parse::<f64>().ok()?;
            Some((a, b.clamp(0.0, 1.0)))
        })
        .collect();
    out.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    if out.is_empty() {
        out.push((100.0, 1.0));
    }
    out
}
