//! Offline replay reports for quant research.
//!
//! This module deliberately consumes replay events only. It has no live order
//! submission path and should be treated as a research/reporting layer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::quant_replay::{
    sort_replay_events, FeatureConfig, FeatureStore, FillModelConfig, FillSimulator, FillStatus,
    ReplayEvent, ReplayEventKind, SimulatedOrder, SimulatedOrderKind,
};
use crate::types::OrderSide;

const SCALE: f64 = 1_000.0;
const NOTIONAL_SCALE: f64 = SCALE * SCALE;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SignalTakerReplayConfig {
    pub starting_cash_usdc: f64,
    pub max_order_usdc: f64,
    pub min_signal_notional_usdc: f64,
    pub taker_latency_ms: i64,
    pub taker_slippage_bps: u64,
}

impl Default for SignalTakerReplayConfig {
    fn default() -> Self {
        Self {
            starting_cash_usdc: 100.0,
            max_order_usdc: 2.0,
            min_signal_notional_usdc: 1.0,
            taker_latency_ms: 150,
            taker_slippage_bps: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalTakerReplayReport {
    pub starting_cash_usdc: f64,
    pub ending_cash_usdc: f64,
    pub marked_position_value_usdc: f64,
    pub final_nav_usdc: f64,
    pub return_pct: f64,
    pub events_loaded: usize,
    pub signal_count: usize,
    pub evaluated_signals: usize,
    pub skipped_low_notional: usize,
    pub skipped_unsupported_side: usize,
    pub filled_orders: usize,
    pub partial_orders: usize,
    pub no_fill_orders: usize,
    pub rejected_orders: usize,
    pub total_filled_notional_usdc: f64,
    pub open_positions: Vec<ReplayPositionMark>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayPositionMark {
    pub token_id: String,
    pub side: OrderSide,
    pub size: u64,
    pub avg_entry_price: u64,
    pub mark_price: Option<u64>,
    pub entry_notional_usdc: f64,
    pub mark_value_usdc: f64,
    pub unrealized_pnl_usdc: f64,
}

#[derive(Debug, Clone)]
struct ReplayPosition {
    side: OrderSide,
    size: u64,
    notional: u128,
}

impl ReplayPosition {
    fn add_buy(&mut self, size: u64, price: u64) {
        self.size = self.size.saturating_add(size);
        self.notional = self.notional.saturating_add(size as u128 * price as u128);
    }

    fn avg_entry_price(&self) -> u64 {
        if self.size == 0 {
            return 0;
        }
        (self.notional / self.size as u128) as u64
    }

    fn entry_notional_usdc(&self) -> f64 {
        self.notional as f64 / NOTIONAL_SCALE
    }
}

pub fn run_signal_taker_replay(
    mut events: Vec<ReplayEvent>,
    config: SignalTakerReplayConfig,
) -> SignalTakerReplayReport {
    sort_replay_events(&mut events);

    let fill_sim = FillSimulator::new(FillModelConfig {
        taker_latency_ms: config.taker_latency_ms,
        taker_slippage_bps: config.taker_slippage_bps,
        ..FillModelConfig::default()
    });

    let mut cash = config.starting_cash_usdc;
    let mut positions: HashMap<String, ReplayPosition> = HashMap::new();
    let mut marks: HashMap<String, u64> = HashMap::new();
    let mut store = FeatureStore::new(FeatureConfig::default());

    let mut signal_count = 0usize;
    let mut evaluated_signals = 0usize;
    let mut skipped_low_notional = 0usize;
    let mut skipped_unsupported_side = 0usize;
    let mut filled_orders = 0usize;
    let mut partial_orders = 0usize;
    let mut no_fill_orders = 0usize;
    let mut rejected_orders = 0usize;
    let mut total_filled_notional_usdc = 0.0;

    for event in &events {
        let step = store.apply_with_frames(event);
        if let Some(mark) = step
            .post
            .mid_price
            .or(step.post.best_bid)
            .or(step.post.best_ask)
        {
            marks.insert(step.token_id.clone(), mark);
        }

        let ReplayEventKind::Signal {
            token_id,
            side,
            price,
            size,
            ..
        } = &event.kind
        else {
            continue;
        };

        signal_count += 1;
        let signal_notional = scaled_notional_usdc(*price, *size);
        if signal_notional < config.min_signal_notional_usdc {
            skipped_low_notional += 1;
            continue;
        }
        if *side != OrderSide::Buy {
            skipped_unsupported_side += 1;
            continue;
        }
        if cash <= 0.0 {
            break;
        }

        let order_notional = config
            .max_order_usdc
            .min(signal_notional)
            .min(cash)
            .max(0.0);
        let order_size = size_from_notional(order_notional, *price);
        if order_size == 0 {
            skipped_low_notional += 1;
            continue;
        }

        evaluated_signals += 1;
        let order = SimulatedOrder {
            submitted_at_ms: event.timestamp_ms,
            token_id: token_id.clone(),
            side: *side,
            limit_price: *price,
            size: order_size,
            kind: SimulatedOrderKind::Taker,
        };

        let outcome = fill_sim.simulate(&order, &events);
        match outcome.status {
            FillStatus::Filled | FillStatus::Partial => {
                if outcome.status == FillStatus::Partial {
                    partial_orders += 1;
                } else {
                    filled_orders += 1;
                }

                let avg_price = outcome.avg_price.unwrap_or(*price);
                let filled_notional = scaled_notional_usdc(avg_price, outcome.filled_size);
                cash -= filled_notional;
                total_filled_notional_usdc += filled_notional;
                positions
                    .entry(token_id.clone())
                    .or_insert_with(|| ReplayPosition {
                        side: *side,
                        size: 0,
                        notional: 0,
                    })
                    .add_buy(outcome.filled_size, avg_price);
            }
            FillStatus::NoFill => no_fill_orders += 1,
            FillStatus::Rejected => rejected_orders += 1,
        }
    }

    let mut open_positions: Vec<_> = positions
        .into_iter()
        .filter_map(|(token_id, position)| {
            if position.size == 0 {
                return None;
            }
            let mark_price = marks.get(&token_id).copied();
            let mark_value_usdc = mark_price
                .map(|price| scaled_notional_usdc(price, position.size))
                .unwrap_or(0.0);
            let entry_notional_usdc = position.entry_notional_usdc();
            Some(ReplayPositionMark {
                token_id,
                side: position.side,
                size: position.size,
                avg_entry_price: position.avg_entry_price(),
                mark_price,
                entry_notional_usdc,
                mark_value_usdc,
                unrealized_pnl_usdc: mark_value_usdc - entry_notional_usdc,
            })
        })
        .collect();
    open_positions.sort_by(|a, b| b.mark_value_usdc.total_cmp(&a.mark_value_usdc));

    let marked_position_value_usdc = open_positions
        .iter()
        .map(|position| position.mark_value_usdc)
        .sum::<f64>();
    let final_nav_usdc = cash + marked_position_value_usdc;
    let return_pct = if config.starting_cash_usdc > 0.0 {
        ((final_nav_usdc / config.starting_cash_usdc) - 1.0) * 100.0
    } else {
        0.0
    };

    SignalTakerReplayReport {
        starting_cash_usdc: config.starting_cash_usdc,
        ending_cash_usdc: cash,
        marked_position_value_usdc,
        final_nav_usdc,
        return_pct,
        events_loaded: events.len(),
        signal_count,
        evaluated_signals,
        skipped_low_notional,
        skipped_unsupported_side,
        filled_orders,
        partial_orders,
        no_fill_orders,
        rejected_orders,
        total_filled_notional_usdc,
        open_positions,
    }
}

fn scaled_notional_usdc(price: u64, size: u64) -> f64 {
    price as f64 * size as f64 / NOTIONAL_SCALE
}

fn size_from_notional(notional_usdc: f64, price: u64) -> u64 {
    if notional_usdc <= 0.0 || price == 0 {
        return 0;
    }
    ((notional_usdc * NOTIONAL_SCALE) / price as f64).floor() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quant_replay::{ReplayEventKind, ReplayLevel};

    fn level(price: u64, size: u64) -> ReplayLevel {
        ReplayLevel { price, size }
    }

    fn book(ts: i64, seq: u64, ask: u64) -> ReplayEvent {
        ReplayEvent {
            timestamp_ms: ts,
            seq,
            kind: ReplayEventKind::BookSnapshot {
                token_id: "tok".to_string(),
                bids: vec![level(490, 100_000)],
                asks: vec![level(ask, 100_000)],
            },
        }
    }

    fn signal(ts: i64, seq: u64, price: u64, size: u64) -> ReplayEvent {
        ReplayEvent {
            timestamp_ms: ts,
            seq,
            kind: ReplayEventKind::Signal {
                token_id: "tok".to_string(),
                side: OrderSide::Buy,
                price,
                size,
                wallet: "0xwallet".to_string(),
            },
        }
    }

    #[test]
    fn signal_taker_report_fills_and_marks_position() {
        let events = vec![book(1_000, 1, 500), signal(1_100, 2, 500, 10_000)];
        let report = run_signal_taker_replay(
            events,
            SignalTakerReplayConfig {
                starting_cash_usdc: 100.0,
                max_order_usdc: 2.0,
                min_signal_notional_usdc: 1.0,
                taker_latency_ms: 0,
                taker_slippage_bps: 0,
            },
        );

        assert_eq!(report.signal_count, 1);
        assert_eq!(report.filled_orders, 1);
        assert_eq!(report.ending_cash_usdc, 98.0);
        assert_eq!(report.open_positions.len(), 1);
        assert_eq!(report.open_positions[0].avg_entry_price, 500);
    }

    #[test]
    fn signal_taker_report_counts_no_fill_without_book() {
        let events = vec![signal(1_100, 2, 500, 10_000)];
        let report = run_signal_taker_replay(
            events,
            SignalTakerReplayConfig {
                taker_latency_ms: 0,
                ..SignalTakerReplayConfig::default()
            },
        );

        assert_eq!(report.signal_count, 1);
        assert_eq!(report.no_fill_orders, 1);
        assert_eq!(report.final_nav_usdc, 100.0);
    }
}
