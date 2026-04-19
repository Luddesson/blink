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
    /// Time-based stop: underwater position exceeded `time_stop_secs`.
    ///
    /// Cuts losing positions before they drift into a full -50% stop-loss.
    /// Apr 5-8 data showed stop-loss trades averaged 930s of hold — a 300s
    /// time stop would have caught 62 of 89 losers and saved ~$25 USDC.
    TimeStop { held_secs: u64, pnl_pct: f64 },
    /// Wide-spread health exit: book is effectively illiquid.
    ///
    /// Triggered when the best bid/ask spread is beyond a configured bps
    /// threshold — breaks the market_not_live trap where a market looks
    /// "alive" (prices present) but is actually uninvestable.
    WideSpread { spread_bps: u64 },
    /// Exit a losing position when event resolution is imminent (4B).
    PreResolutionStop { remaining_secs: i64, pnl_pct: f64 },
    /// Force-close ALL positions within N seconds of event resolution (3C).
    PreEventClose { secs_left: i64 },
    /// Exit a profitable position due to adverse price momentum (4A).
    /// Closes `fraction` of the position (default 0.5 = scale out, not dump).
    AdverseMomentum { price_change_bps: i64, fraction: f64 },
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
            Self::TimeStop { held_secs, .. } =>
                format!("time_stop@{}s", held_secs),
            Self::WideSpread { spread_bps } =>
                format!("wide_spread@{}bps", spread_bps),
            Self::PreResolutionStop { remaining_secs, .. } =>
                format!("pre_resolution_stop@{}s_left", remaining_secs),
            Self::PreEventClose { secs_left } =>
                format!("pre_event_close@{}s_left", secs_left),
            Self::AdverseMomentum { price_change_bps, fraction } =>
                format!("adverse_momentum@{}bps[{:.0}%]", price_change_bps, fraction * 100.0),
        }
    }

    /// Fraction of position to close (1.0 = full close).
    pub fn fraction(&self) -> f64 {
        match self {
            Self::TakeProfit { fraction, .. } => *fraction,
            Self::AdverseMomentum { fraction, .. } => *fraction,
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

    // Max hold time (absolute limit, default 6h = 21600s)
    pub max_hold_secs: u64,

    // Stale market close
    pub stale_close_secs: u64,

    /// Time stop for underwater positions: exit a losing position after this
    /// many seconds regardless of drawdown. Fires BEFORE `stop_loss_pct`.
    /// 0 disables.
    pub time_stop_secs: u64,

    /// Spread-based health exit: close when best bid/ask spread exceeds this
    /// many basis points (1% = 100 bps). Requires the caller to pass spread
    /// data; otherwise inert. 0 disables.
    pub wide_spread_bps_exit: u64,

    // Event-aware confidence exit (4B): exit a losing position when resolution is near.
    pub event_aware_exit_secs: u64,
    pub event_aware_exit_loss_pct: f64,

    // Force-close ALL positions within this many seconds of event resolution (3C).
    pub pre_event_close_secs: u64,

    // Adverse momentum exit (4A): exit profitable position if price moved this many bps against us.
    pub momentum_exit_threshold_bps: u64,
    /// Fraction of position to close on adverse momentum (default 0.5 = scale out).
    pub momentum_exit_fraction: f64,
    /// How often (secs) the momentum reference price is refreshed by autoclaim.
    pub momentum_check_interval_secs: u64,
    /// Grace period (secs): suppress adverse momentum exits for newly opened positions.
    pub momentum_grace_secs: u64,
}

impl ExitConfig {
    /// Load from environment variables with sensible defaults.
    pub fn from_env() -> Self {
        Self {
            autoclaim_tiers: parse_autoclaim_tiers_from_env(),
            // Data-driven: -50% stop reduces bleed by 70% vs -25%
            stop_loss_pct: env_f64("STOP_LOSS_PCT", 50.0).clamp(1.0, 99.0),
            stop_loss_small_pct: env_f64("STOP_LOSS_SMALL_PCT", 50.0).clamp(1.0, 99.0),
            stop_loss_small_notional_usdc: env_f64("STOP_LOSS_SMALL_NOTIONAL_USDC", 8.0),
            trailing_stop_activate_pct: env_f64("TRAILING_STOP_ACTIVATE_PCT", 25.0),
            trailing_stop_drop_pct: env_f64("TRAILING_STOP_DROP_PCT", 15.0),
            stagnant_exit_secs: env_u64("STAGNANT_EXIT_SECS", 7200),
            stagnant_threshold_pct: env_f64("STAGNANT_THRESHOLD_PCT", 5.0),
            max_hold_secs: env_u64("MAX_HOLD_SECS", 21_600), // 6h — 24h+ trades have 17% WR
            stale_close_secs: env_u64("STALE_CLOSE_SECS", 60),
            // Data-driven (Apr 5-8): stop-loss losers averaged 930s held. A 300s
            // time stop on underwater positions would've caught 62/89 losers
            // and saved ~$25 USDC versus waiting for a -50% drawdown.
            time_stop_secs: env_u64("TIME_STOP_SECS", 300),
            wide_spread_bps_exit: env_u64("WIDE_SPREAD_BPS_EXIT", 500),
            event_aware_exit_secs: env_u64("EVENT_AWARE_EXIT_SECS", 3600),
            event_aware_exit_loss_pct: env_f64("EVENT_AWARE_EXIT_LOSS_PCT", 5.0),
            pre_event_close_secs: env_u64("PRE_EVENT_CLOSE_SECS", 60),
            momentum_exit_threshold_bps: env_u64("MOMENTUM_EXIT_THRESHOLD_BPS", 300),
            momentum_exit_fraction: env_f64("MOMENTUM_EXIT_FRACTION", 0.3).clamp(0.1, 1.0),
            momentum_check_interval_secs: env_u64("MOMENTUM_CHECK_INTERVAL_SECS", 60),
            momentum_grace_secs: env_u64("MOMENTUM_GRACE_SECS", 60),
        }
    }
}

impl Default for ExitConfig {
    fn default() -> Self {
        Self {
            // Data-driven: -50% stop, 6h max hold
            autoclaim_tiers: vec![(100.0, 0.25), (200.0, 0.50), (300.0, 1.0)],
            stop_loss_pct: 50.0,
            stop_loss_small_pct: 50.0,
            stop_loss_small_notional_usdc: 8.0,
            trailing_stop_activate_pct: 25.0,
            trailing_stop_drop_pct: 15.0,
            stagnant_exit_secs: 7200,
            stagnant_threshold_pct: 5.0,
            max_hold_secs: 21_600,
            stale_close_secs: 60,
            time_stop_secs: 300,
            wide_spread_bps_exit: 500,
            event_aware_exit_secs: 3600,
            event_aware_exit_loss_pct: 5.0,
            pre_event_close_secs: 60,
            momentum_exit_threshold_bps: 300,
            momentum_exit_fraction: 0.3,
            momentum_check_interval_secs: 60,
            momentum_grace_secs: 60,
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
        // Use wall-clock time for held_secs — Instant::elapsed() silently wraps to 0
        // when positions are older than system uptime (e.g. after a Windows reboot),
        // causing MaxHoldExpired / StagnantExit to never fire after a restart.
        let held_secs = (chrono::Local::now() - pos.opened_at_wall)
            .num_seconds()
            .max(0) as u64;
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

        // 1.7. Time stop: underwater position held past `time_stop_secs`.
        //      Fires BEFORE the stop-loss percentage check so losers exit at a
        //      bounded time cost rather than bleeding to -50%.
        if config.time_stop_secs > 0
            && held_secs >= config.time_stop_secs
            && pnl_pct < 0.0
        {
            decisions.push(ExitDecision {
                position_idx: idx,
                action: ExitAction::TimeStop { held_secs, pnl_pct },
            });
            continue;
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
        //      Grace period: skip if position is younger than momentum_grace_secs.
        if config.momentum_exit_threshold_bps > 0 && pnl_pct > 0.0
            && held_secs >= config.momentum_grace_secs
        {
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
                        action: ExitAction::AdverseMomentum {
                            price_change_bps: price_change_bps.abs(),
                            fraction: config.momentum_exit_fraction,
                        },
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

        // 4. Take-profit (tiered — picks highest matching tier NOT already claimed)
        let mut best_tier: Option<(f64, f64)> = None;
        for &(threshold, fraction) in &config.autoclaim_tiers {
            if pnl_pct >= threshold && threshold > pos.last_claimed_tier_pct {
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

        // 7. Market not live (stale data) — close if no fresh order book price
        //    exists AND position has been held long enough.
        if config.stale_close_secs > 0 && held_secs >= config.stale_close_secs {
            if !has_live_price(&pos.token_id) {
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

    // Phase 6: RN1 bet-size proportional conviction (logarithmic).
    // Bigger RN1 bets signal higher conviction. Scale smoothly from 1× at $10
    // to ~2× at $100k using log2. This replaces the binary whale threshold.
    if rn1_bet_usdc > 10.0 {
        let log_boost = (rn1_bet_usdc / 10.0).log2() / 14.0; // log2(100k/10)≈13.3 → ~1.0
        mult += config.whale_bonus_multiplier * log_boost.clamp(0.0, 1.0);
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
        .unwrap_or_else(|_| "100:0.25,200:0.50,300:1.0".to_string());
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
        // Very small bet ($5), unknown category, low liquidity → only log-scale boost
        // $5k > $10, so log_boost = log2(500)/14 ≈ 0.64 → partial whale bonus
        let mult = conviction_multiplier(5_000.0, "entertainment", None, 50_000.0, &config);
        assert!(mult >= config.base_multiplier, "should be at or above base");
        assert!(mult < config.base_multiplier + config.whale_bonus_multiplier + 0.001,
            "should not have full whale bonus");
    }

    #[test]
    fn conviction_all_bonuses() {
        let config = FilterConfig::default();
        // Whale bet + sports + NFL + high liquidity → all bonuses
        // $60k → log_boost = log2(6000)/14 ≈ 0.89 → near-full whale bonus
        let mult = conviction_multiplier(60_000.0, "sports", Some("NFL"), 250_000.0, &config);
        // Should be close to max but not necessarily exact due to log scaling
        assert!(mult > config.base_multiplier + config.sports_bonus,
            "should include sports bonus + substantial whale bonus");
        assert!(mult <= config.max_multiplier + f64::EPSILON,
            "must not exceed max");
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
        let mut pos = make_position(0.50, 0.20, 5.0); // $5 position, -60% loss (exceeds -50% stop)
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

    #[test]
    fn exit_time_stop_fires_before_stop_loss() {
        // Underwater position (-10%) held past time_stop_secs — TimeStop cuts it
        // before the -50% stop-loss would trigger.
        let mut pos = make_position(0.50, 0.45, 20.0);
        pos.opened_at_wall = chrono::Local::now() - chrono::Duration::seconds(600);
        let mut config = ExitConfig::default();
        config.time_stop_secs = 300;
        let decisions = evaluate_exits(&[pos], &config, |_| true);
        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0].action, ExitAction::TimeStop { .. }),
            "expected TimeStop, got {:?}", decisions[0].action);
    }

    #[test]
    fn exit_time_stop_skips_profitable_position() {
        // Profitable position held past time_stop_secs — TimeStop must NOT fire.
        let mut pos = make_position(0.50, 0.55, 20.0);
        pos.opened_at_wall = chrono::Local::now() - chrono::Duration::seconds(600);
        let mut config = ExitConfig::default();
        config.time_stop_secs = 300;
        let decisions = evaluate_exits(&[pos], &config, |_| true);
        assert!(decisions.is_empty(), "healthy position should not time-stop");
    }

    #[test]
    fn exit_time_stop_disabled_when_zero() {
        let mut pos = make_position(0.50, 0.45, 20.0);
        pos.opened_at_wall = chrono::Local::now() - chrono::Duration::seconds(3600);
        let mut config = ExitConfig::default();
        config.time_stop_secs = 0;
        let decisions = evaluate_exits(&[pos], &config, |_| true);
        // -10% drawdown, no time stop, no other trigger → no action
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
            last_claimed_tier_pct: 0.0,
            signal_source: "rn1".to_string(),
            analysis_id: None,
        }
    }
}
