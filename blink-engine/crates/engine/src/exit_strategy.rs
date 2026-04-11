//! Exit strategy module — centralizes all position exit decision logic.
//!
//! Provides [`ExitConfig`] (loaded from environment) and [`evaluate_exits`]
//! which inspects a portfolio snapshot and returns a list of positions to close
//! with their exit reasons. The actual closing is performed by the caller
//! (PaperEngine or LiveEngine) so this module stays pure and testable.

use crate::paper_portfolio::PaperPosition;
use crate::types::OrderSide;

// ─── ExitAction ──────────────────────────────────────────────────────────────

/// Describes why a position should be closed.
#[derive(Debug, Clone)]
pub enum ExitAction {
    /// Tiered take-profit: threshold % reached, close `fraction` of position.
    TakeProfit { threshold_pct: f64, fraction: f64 },
    /// Stop-loss triggered (full close).
    StopLoss { threshold_pct: f64 },
    /// Trailing stop: position was up ≥ activate%, then dropped ≥ drop% from peak.
    TrailingStop { peak_price: f64, drop_pct: f64 },
    /// Position held too long with insufficient price movement.
    StagnantExit { held_secs: u64, move_pct: f64 },
    /// Market resolved (price ≥ 0.99 or ≤ 0.01).
    Resolved { exit_price: f64, winner: bool },
    /// Market no longer has live book data and position is old enough.
    MarketNotLive { held_secs: u64 },
    /// Max hold duration exceeded (absolute time limit).
    MaxHoldExpired { held_secs: u64 },
    /// Exit a losing position when event resolution is imminent (4B).
    PreResolutionStop { remaining_secs: i64, pnl_pct: f64 },
    /// Force-close ALL positions within N seconds of event resolution (3C).
    PreEventClose { secs_left: i64 },
    /// Exit a profitable position due to adverse price momentum (4A).
    AdverseMomentum { price_change_bps: i64 },
}

impl ExitAction {
    /// Human-readable reason string for ClosedTrade records.
    pub fn reason(&self) -> String {
        match self {
            Self::TakeProfit { threshold_pct, fraction } =>
                format!("autoclaim@{threshold_pct:.0}%[{:.0}%]", fraction * 100.0),
            Self::StopLoss { threshold_pct } =>
                format!("stop_loss@-{threshold_pct:.0}%"),
            Self::TrailingStop { .. } =>
                "trailing_stop".to_string(),
            Self::StagnantExit { .. } =>
                "stagnant_exit".to_string(),
            Self::Resolved { winner, .. } =>
                if *winner { "resolved@winner".to_string() } else { "resolved@loser".to_string() },
            Self::MarketNotLive { .. } =>
                "autoclaim@market_not_live".to_string(),
            Self::MaxHoldExpired { held_secs } =>
                format!("max_hold@{}s", held_secs),
            Self::PreResolutionStop { remaining_secs, .. } =>
                format!("pre_resolution_stop@{}s_left", remaining_secs),
            Self::PreEventClose { secs_left } =>
                format!("pre_event_close@{}s_left", secs_left),
            Self::AdverseMomentum { price_change_bps } =>
                format!("adverse_momentum@{}bps", price_change_bps),
        }
    }

    /// Fraction of position to close (1.0 = full close).
    pub fn fraction(&self) -> f64 {
        match self {
            Self::TakeProfit { fraction, .. } => *fraction,
            _ => 1.0,
        }
    }

    /// Tags for the execution scorecard.
    pub fn outcome_tags(&self) -> Vec<String> {
        vec![self.reason()]
    }
}

// ─── ExitConfig ──────────────────────────────────────────────────────────────

/// All exit strategy parameters, loaded from environment variables.
#[derive(Debug, Clone)]
pub struct ExitConfig {
    // Take-profit tiers: (threshold_pct, fraction_to_close)
    pub autoclaim_tiers: Vec<(f64, f64)>,

    // Stop-loss
    pub stop_loss_pct: f64,
    pub stop_loss_small_pct: f64,
    pub stop_loss_small_notional_usdc: f64,

    // Trailing stop
    pub trailing_stop_activate_pct: f64,
    pub trailing_stop_drop_pct: f64,

    // Stagnant exit
    pub stagnant_exit_secs: u64,
    pub stagnant_threshold_pct: f64,

    // Max hold time (absolute limit, default 5 days = 432000s)
    pub max_hold_secs: u64,

    // Stale market close
    pub stale_close_secs: u64,

    // Event-aware confidence exit (4B): exit a losing position when resolution is near.
    pub event_aware_exit_secs: u64,
    pub event_aware_exit_loss_pct: f64,

    // Force-close ALL positions within this many seconds of event resolution (3C).
    pub pre_event_close_secs: u64,

    // Adverse momentum exit (4A): exit profitable position if price moved this many bps against us.
    pub momentum_exit_threshold_bps: u64,
    /// How often (secs) the momentum reference price is refreshed by autoclaim.
    pub momentum_check_interval_secs: u64,
}

impl ExitConfig {
    /// Load from environment variables with sensible defaults.
    pub fn from_env() -> Self {
        Self {
            autoclaim_tiers: parse_autoclaim_tiers_from_env(),
            stop_loss_pct: env_f64("STOP_LOSS_PCT", 25.0).clamp(1.0, 99.0),
            stop_loss_small_pct: env_f64("STOP_LOSS_SMALL_PCT", 20.0).clamp(1.0, 99.0),
            stop_loss_small_notional_usdc: env_f64("STOP_LOSS_SMALL_NOTIONAL_USDC", 6.0),
            trailing_stop_activate_pct: env_f64("TRAILING_STOP_ACTIVATE_PCT", 15.0),
            trailing_stop_drop_pct: env_f64("TRAILING_STOP_DROP_PCT", 10.0),
            stagnant_exit_secs: env_u64("STAGNANT_EXIT_SECS", 1800),
            stagnant_threshold_pct: env_f64("STAGNANT_THRESHOLD_PCT", 5.0),
            max_hold_secs: env_u64("MAX_HOLD_SECS", 432_000), // 5 days
            stale_close_secs: env_u64("STALE_CLOSE_SECS", 300),
            event_aware_exit_secs: env_u64("EVENT_AWARE_EXIT_SECS", 3600),
            event_aware_exit_loss_pct: env_f64("EVENT_AWARE_EXIT_LOSS_PCT", 5.0),
            pre_event_close_secs: env_u64("PRE_EVENT_CLOSE_SECS", 60),
            momentum_exit_threshold_bps: env_u64("MOMENTUM_EXIT_THRESHOLD_BPS", 300),
            momentum_check_interval_secs: env_u64("MOMENTUM_CHECK_INTERVAL_SECS", 60),
        }
    }
}

impl Default for ExitConfig {
    fn default() -> Self {
        Self {
            autoclaim_tiers: vec![(40.0, 0.30), (70.0, 0.30), (100.0, 1.0)],
            stop_loss_pct: 25.0,
            stop_loss_small_pct: 20.0,
            stop_loss_small_notional_usdc: 6.0,
            trailing_stop_activate_pct: 15.0,
            trailing_stop_drop_pct: 10.0,
            stagnant_exit_secs: 1800,
            stagnant_threshold_pct: 5.0,
            max_hold_secs: 432_000,
            stale_close_secs: 300,
            event_aware_exit_secs: 3600,
            event_aware_exit_loss_pct: 5.0,
            pre_event_close_secs: 60,
            momentum_exit_threshold_bps: 300,
            momentum_check_interval_secs: 60,
        }
    }
}

// ─── Evaluation ──────────────────────────────────────────────────────────────

/// Result of evaluating one position: its index and the exit action to take.
#[derive(Debug, Clone)]
pub struct ExitDecision {
    pub position_idx: usize,
    pub action: ExitAction,
}

/// Evaluates all open positions against the exit config and returns decisions.
///
/// `has_live_price` is a closure that returns `true` if the WS order book has a
/// fresh price for the given token_id. Pass `|_| true` to skip stale-market checks.
///
/// Decisions are returned in priority order (resolved > stop-loss > trailing >
/// take-profit > stagnant > max-hold > stale). Only the highest-priority action
/// per position is returned.
pub fn evaluate_exits<F>(
    positions: &[PaperPosition],
    config: &ExitConfig,
    has_live_price: F,
) -> Vec<ExitDecision>
where
    F: Fn(&str) -> bool,
{
    let mut decisions = Vec::new();

    for (idx, pos) in positions.iter().enumerate() {
        let held_secs = pos.opened_at.elapsed().as_secs();
        let pnl_pct = pos.unrealized_pnl_pct();

        // 1. Resolved market (highest priority)
        if pos.current_price >= 0.99 || pos.current_price <= 0.01 {
            decisions.push(ExitDecision {
                position_idx: idx,
                action: ExitAction::Resolved {
                    exit_price: pos.current_price.clamp(0.0, 1.0),
                    winner: pos.current_price >= 0.99,
                },
            });
            continue;
        }

        // 1.5. Time-decay force-close (3C): close ALL positions within N secs of event end.
        if config.pre_event_close_secs > 0 {
            if let Some(end_ts) = pos.event_end_time {
                let now_ts = chrono::Utc::now().timestamp();
                let remaining = end_ts - now_ts;
                if remaining > 0 && remaining <= config.pre_event_close_secs as i64 {
                    decisions.push(ExitDecision {
                        position_idx: idx,
                        action: ExitAction::PreEventClose { secs_left: remaining },
                    });
                    continue;
                }
            }
        }

        // 2. Stop-loss
        let sl_threshold = if pos.usdc_spent < config.stop_loss_small_notional_usdc {
            config.stop_loss_small_pct
        } else {
            config.stop_loss_pct
        };
        if pnl_pct <= -sl_threshold {
            decisions.push(ExitDecision {
                position_idx: idx,
                action: ExitAction::StopLoss { threshold_pct: sl_threshold },
            });
            continue;
        }

        // 2.5. Pre-resolution stop (4B): exit a losing position before event resolves.
        if config.event_aware_exit_secs > 0 {
            if let Some(end_ts) = pos.event_end_time {
                let now_ts = chrono::Utc::now().timestamp();
                let remaining = end_ts - now_ts;
                if remaining > 0 && remaining < config.event_aware_exit_secs as i64
                    && pnl_pct <= -config.event_aware_exit_loss_pct
                {
                    decisions.push(ExitDecision {
                        position_idx: idx,
                        action: ExitAction::PreResolutionStop { remaining_secs: remaining, pnl_pct },
                    });
                    continue;
                }
            }
        }

        // 2.7. Adverse momentum exit (4A): exit profitable position if price moved adversely.
        if config.momentum_exit_threshold_bps > 0 && pnl_pct > 0.0 {
            let now_ts = chrono::Utc::now().timestamp();
            if now_ts - pos.momentum_ref_ts >= config.momentum_check_interval_secs as i64 {
                let price_change_bps = ((pos.current_price - pos.momentum_ref_price)
                    / pos.momentum_ref_price.max(0.001)
                    * 10_000.0) as i64;
                let adverse = match pos.side {
                    OrderSide::Buy => price_change_bps < -(config.momentum_exit_threshold_bps as i64),
                    OrderSide::Sell => price_change_bps > (config.momentum_exit_threshold_bps as i64),
                };
                if adverse {
                    decisions.push(ExitDecision {
                        position_idx: idx,
                        action: ExitAction::AdverseMomentum { price_change_bps: price_change_bps.abs() },
                    });
                    continue;
                }
            }
        }

        // 3. Trailing stop
        if pos.side == OrderSide::Buy {
            let gain_from_entry = (pos.peak_price - pos.entry_price) / pos.entry_price * 100.0;
            if gain_from_entry >= config.trailing_stop_activate_pct {
                let drop_from_peak = (pos.peak_price - pos.current_price) / pos.peak_price * 100.0;
                if drop_from_peak >= config.trailing_stop_drop_pct {
                    decisions.push(ExitDecision {
                        position_idx: idx,
                        action: ExitAction::TrailingStop {
                            peak_price: pos.peak_price,
                            drop_pct: drop_from_peak,
                        },
                    });
                    continue;
                }
            }
        }

        // 4. Take-profit (tiered — picks highest matching tier)
        let mut best_tier: Option<(f64, f64)> = None;
        for &(threshold, fraction) in &config.autoclaim_tiers {
            if pnl_pct >= threshold {
                best_tier = Some((threshold, fraction));
            }
        }
        if let Some((threshold_pct, fraction)) = best_tier {
            decisions.push(ExitDecision {
                position_idx: idx,
                action: ExitAction::TakeProfit { threshold_pct, fraction },
            });
            continue;
        }

        // 5. Max hold time (absolute limit)
        if config.max_hold_secs > 0 && held_secs >= config.max_hold_secs {
            decisions.push(ExitDecision {
                position_idx: idx,
                action: ExitAction::MaxHoldExpired { held_secs },
            });
            continue;
        }

        // 6. Stagnant exit (held long, barely moved)
        if held_secs >= config.stagnant_exit_secs {
            let move_pct = ((pos.current_price - pos.entry_price) / pos.entry_price * 100.0).abs();
            if move_pct < config.stagnant_threshold_pct {
                decisions.push(ExitDecision {
                    position_idx: idx,
                    action: ExitAction::StagnantExit { held_secs, move_pct },
                });
                continue;
            }
        }

        // 7. Market not live (stale data)
        if config.stale_close_secs > 0 && held_secs >= config.stale_close_secs {
            let has_fresh = has_live_price(&pos.token_id)
                || (pos.current_price - pos.entry_price).abs() > 0.001;
            if !has_fresh {
                decisions.push(ExitDecision {
                    position_idx: idx,
                    action: ExitAction::MarketNotLive { held_secs },
                });
            }
        }
    }

    decisions
}

// ─── Conviction-Based Dynamic Sizing ─────────────────────────────────────────

/// Computes a conviction multiplier based on signal characteristics and FilterConfig.
///
/// Returns a value between `config.base_multiplier` and `config.max_multiplier`.
/// The caller should multiply `rn1_notional_usd * multiplier` to get position size.
pub fn conviction_multiplier(
    rn1_bet_usdc: f64,
    category: &str,
    sport: Option<&str>,
    liquidity: f64,
    config: &crate::types::FilterConfig,
) -> f64 {
    let mut mult = config.base_multiplier;

    // Whale bonus: RN1 bet exceeds threshold → high conviction
    if rn1_bet_usdc >= config.whale_bet_threshold_usdc {
        mult += config.whale_bonus_multiplier;
    }

    // High liquidity bonus: market is very liquid → lower slippage risk
    if liquidity >= config.high_liquidity_threshold_usdc {
        mult += config.high_liquidity_bonus;
    }

    // Sports category bonus
    if category.eq_ignore_ascii_case("sports") {
        mult += config.sports_bonus;
    }

    // Preferred sport bonus
    if let Some(s) = sport {
        if config.allowed_sports.iter().any(|allowed| allowed.eq_ignore_ascii_case(s)) {
            mult += config.preferred_sport_bonus;
        }
    }

    mult.clamp(config.base_multiplier, config.max_multiplier)
}

// ─── Env helpers ─────────────────────────────────────────────────────────────

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_autoclaim_tiers_from_env() -> Vec<(f64, f64)> {
    let raw = std::env::var("AUTOCLAIM_TIERS")
        .unwrap_or_else(|_| "40:0.30,70:0.30,100:1.0".to_string());
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

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FilterConfig;

    #[test]
    fn conviction_base_only() {
        let config = FilterConfig::default();
        // Small bet, unknown category, low liquidity → base multiplier only
        let mult = conviction_multiplier(5_000.0, "entertainment", None, 50_000.0, &config);
        assert!((mult - config.base_multiplier).abs() < f64::EPSILON);
    }

    #[test]
    fn conviction_all_bonuses() {
        let config = FilterConfig::default();
        // Whale bet + sports + NFL + high liquidity → all bonuses
        let mult = conviction_multiplier(60_000.0, "sports", Some("NFL"), 250_000.0, &config);
        let expected = config.base_multiplier
            + config.whale_bonus_multiplier
            + config.high_liquidity_bonus
            + config.sports_bonus
            + config.preferred_sport_bonus;
        assert!((mult - expected.min(config.max_multiplier)).abs() < f64::EPSILON);
    }

    #[test]
    fn conviction_clamped_to_max() {
        let mut config = FilterConfig::default();
        config.max_multiplier = 0.08; // lower cap
        let mult = conviction_multiplier(60_000.0, "sports", Some("NFL"), 250_000.0, &config);
        assert!(mult <= config.max_multiplier + f64::EPSILON);
    }

    #[test]
    fn exit_resolved_winner() {
        let positions = vec![make_position(0.35, 0.99, 100.0)];
        let config = ExitConfig::default();
        let decisions = evaluate_exits(&positions, &config, |_| true);
        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0].action, ExitAction::Resolved { winner: true, .. }));
    }

    #[test]
    fn exit_stop_loss_small() {
        let mut pos = make_position(0.50, 0.30, 5.0); // $5 position, -40% loss
        let config = ExitConfig::default();
        let decisions = evaluate_exits(&[pos], &config, |_| true);
        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0].action, ExitAction::StopLoss { .. }));
    }

    #[test]
    fn exit_no_action_healthy_position() {
        let pos = make_position(0.50, 0.52, 20.0); // small gain, healthy
        let config = ExitConfig::default();
        let decisions = evaluate_exits(&[pos], &config, |_| true);
        assert!(decisions.is_empty());
    }

    fn make_position(entry: f64, current: f64, usdc_spent: f64) -> PaperPosition {
        PaperPosition {
            id: 1,
            token_id: "test_token".to_string(),
            market_title: None,
            market_outcome: None,
            side: OrderSide::Buy,
            entry_price: entry,
            shares: usdc_spent / entry,
            usdc_spent,
            entry_fee_paid_usdc: 0.0,
            current_price: current,
            peak_price: current.max(entry),
            fee_category: "sports".to_string(),
            fee_rate: 0.03,
            opened_at: std::time::Instant::now(),
            rn1_order_id: "test_order".to_string(),
            opened_at_wall: chrono::Local::now(),
            entry_slippage_bps: 0.0,
            queue_delay_ms: 0,
            experiment_variant: "A".to_string(),
            event_start_time: None,
            event_end_time: None,
            momentum_ref_price: entry,
            momentum_ref_ts: 0,
        }
    }
}
