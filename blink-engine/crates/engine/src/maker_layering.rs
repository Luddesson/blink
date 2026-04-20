//! Multi-level maker quote management.
//!
//! Maintains a ladder of resting maker orders per market so the bot can
//! capture fills at several price points simultaneously. This module is
//! only wired into the hot path when the `maker-layering` cargo feature is
//! enabled (which, in turn, is only activated under the
//! `ExecutionProfile::HftMaker` selection once that profile lands on the
//! branch). With the feature off, the module compiles as dead code and is
//! inert.
//!
//! # Design notes
//!
//! * All price/notional arithmetic uses `u64` integer math.
//!   - Prices are in milli-units (×1_000 — same convention as the rest of
//!     the crate — see `types::parse_price`).
//!   - Notional is in USDC-mil (×1_000).
//! * Fill-feedback (phase 3) is a hard prerequisite: `on_fill` / `on_cancel`
//!   let the engine retire layers as they're (partially) filled or pulled,
//!   so we don't accidentally double-post.
//! * The engine never bypasses `StreamRiskGate`. Callers must `try_admit`
//!   each generated intent and skip on `Reject`/`Throttle`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use dashmap::DashMap;

use crate::order_router::OrderIntent;
use crate::strategy::StrategyMode;
use crate::types::{OrderSide, TimeInForce};

// ─── Config ──────────────────────────────────────────────────────────────────

/// Layering parameters.
///
/// Defaults: 3 levels, 20 bps step, ≤ $25 per level, 5 s max age.
#[derive(Debug, Clone, Copy)]
pub struct LayerConfig {
    /// Number of price levels to maintain per (market, side).
    pub levels: u8,
    /// Spacing between levels in basis points (1 bp = 0.01 %).
    pub step_bps: u16,
    /// Per-level notional budget in USDC-mil (e.g., 25_000 = $25.000).
    pub per_level_notional_usdc_mil: u64,
    /// Max wall-clock age of a layer before it is considered stale and
    /// eligible for re-price eviction.
    pub max_age_ms: u64,
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self {
            levels: 3,
            step_bps: 20,
            per_level_notional_usdc_mil: 25_000, // $25.000
            max_age_ms: 5_000,
        }
    }
}

// ─── State ───────────────────────────────────────────────────────────────────

/// A single resting order tracked by the layering engine.
#[derive(Debug, Clone, Copy)]
pub struct LayerOrder {
    pub intent_id: u64,
    /// Zero-based level index (0 = closest to mid).
    pub level: u8,
    /// Signed offset from mid in basis points. Negative for bids, positive
    /// for asks (stored as `i32` to keep the struct `Copy`).
    pub price_bps_from_mid: i32,
    /// When the order was planned (not when it was acked — we don't need
    /// that resolution here).
    pub placed_at: Instant,
    /// Side associated with the layer (needed so `plan_layers` can be called
    /// for bids and asks independently on the same market).
    pub side: OrderSide,
    /// Filled quantity in USDC-mil (mirrors `PendingOrder::fill_notional_usdc_mil`).
    pub filled_usdc_mil: u64,
}

/// Per-market ladder state.
#[derive(Debug)]
pub struct MarketLayerState {
    pub orders: Vec<LayerOrder>,
    pub last_refresh: Instant,
}

impl MarketLayerState {
    fn new() -> Self {
        Self {
            orders: Vec::new(),
            last_refresh: Instant::now(),
        }
    }
}

// ─── Engine ──────────────────────────────────────────────────────────────────

/// Stateful planner for multi-level maker quotes.
#[derive(Debug)]
pub struct MakerLayerEngine {
    pub config: LayerConfig,
    pub per_market: DashMap<String, MarketLayerState>,
    /// Monotonic counter used to mint `intent_id`s for generated layer
    /// intents. Kept inside the engine so tests are deterministic.
    next_intent_id: AtomicU64,
}

impl Default for MakerLayerEngine {
    fn default() -> Self {
        Self::new(LayerConfig::default())
    }
}

/// Reason a layer is flagged for cancellation by `reprice_stale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepriceReason {
    /// Exceeded `max_age_ms`.
    StaleAge,
    /// Mid moved further than the configured drift threshold.
    MidDrift,
}

impl MakerLayerEngine {
    pub fn new(config: LayerConfig) -> Self {
        Self {
            config,
            per_market: DashMap::new(),
            // Start high enough not to collide with live signal-generated IDs
            // in the unlikely case the two spaces are ever merged.
            next_intent_id: AtomicU64::new(1 << 48),
        }
    }

    fn mint_intent_id(&self) -> u64 {
        self.next_intent_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Plan any missing layers for `(market_id, side)`.
    ///
    /// Returns a freshly built batch of `OrderIntent`s — one per missing
    /// level, respecting the remaining budget. Already-live layers are
    /// preserved. The caller is responsible for submitting each intent via
    /// `OrderRouter::submit` after passing `StreamRiskGate::try_admit`.
    pub fn plan_layers(
        &self,
        market_id: &str,
        token_id: &str,
        side: OrderSide,
        mid_price_u64_milli: u64,
        mut budget_remaining_usdc_mil: u64,
    ) -> Vec<OrderIntent> {
        if mid_price_u64_milli == 0 || self.config.levels == 0 {
            return Vec::new();
        }
        let now = Instant::now();
        let mut state = self
            .per_market
            .entry(market_id.to_string())
            .or_insert_with(MarketLayerState::new);
        state.last_refresh = now;

        // Levels currently live for this side.
        let mut have_level = [false; 256];
        for o in state.orders.iter().filter(|o| o.side == side) {
            have_level[o.level as usize] = true;
        }

        let mut out = Vec::with_capacity(self.config.levels as usize);
        for level in 0..self.config.levels {
            if have_level[level as usize] {
                continue;
            }
            if budget_remaining_usdc_mil < self.config.per_level_notional_usdc_mil {
                break;
            }

            // bps offset for this level (level 0 starts one `step_bps` away
            // from mid so we don't cross the touch).
            let bps = (level as u32 + 1) * self.config.step_bps as u32;
            let offset_milli =
                (mid_price_u64_milli.saturating_mul(bps as u64)) / 10_000u64;
            let price_u64 = match side {
                OrderSide::Buy => mid_price_u64_milli.saturating_sub(offset_milli),
                OrderSide::Sell => mid_price_u64_milli.saturating_add(offset_milli),
            };
            if price_u64 == 0 {
                continue;
            }

            let size_u64 = self.config.per_level_notional_usdc_mil;
            budget_remaining_usdc_mil =
                budget_remaining_usdc_mil.saturating_sub(size_u64);

            let intent_id = self.mint_intent_id();
            let price_bps_from_mid = match side {
                OrderSide::Buy => -(bps as i32),
                OrderSide::Sell => bps as i32,
            };

            state.orders.push(LayerOrder {
                intent_id,
                level,
                price_bps_from_mid,
                placed_at: now,
                side,
                filled_usdc_mil: 0,
            });

            out.push(OrderIntent {
                intent_id,
                market_id: market_id.to_string(),
                token_id: token_id.to_string(),
                side,
                price_u64,
                size_u64,
                tif: TimeInForce::Gtc,
                strategy_mode: StrategyMode::Mirror,
                requested_at: now,
                signed_payload: None,
            });
        }

        out
    }

    /// Return layers that should be cancelled because they are stale or
    /// because mid drifted beyond `drift_bps_threshold` away from their
    /// placement price. The cancelled entries are also removed from the
    /// tracked ladder.
    pub fn reprice_stale(
        &self,
        market_id: &str,
        current_mid_milli: u64,
        drift_bps_threshold: u16,
    ) -> Vec<(u64, RepriceReason)> {
        let Some(mut state) = self.per_market.get_mut(market_id) else {
            return Vec::new();
        };
        let now = Instant::now();
        let max_age_ms = self.config.max_age_ms;
        let mut evicted: Vec<(u64, RepriceReason)> = Vec::new();

        state.orders.retain(|o| {
            let age_ms = now.saturating_duration_since(o.placed_at).as_millis() as u64;
            if age_ms >= max_age_ms {
                evicted.push((o.intent_id, RepriceReason::StaleAge));
                return false;
            }
            // Reconstruct the placement price from stored bps and compare
            // against the current mid.
            if current_mid_milli > 0 {
                let placement_bps = o.price_bps_from_mid.unsigned_abs() as u64;
                let placement_offset =
                    (current_mid_milli.saturating_mul(placement_bps)) / 10_000u64;
                let placement_price = if o.price_bps_from_mid < 0 {
                    current_mid_milli.saturating_sub(placement_offset)
                } else {
                    current_mid_milli.saturating_add(placement_offset)
                };
                let drift_milli = current_mid_milli.abs_diff(placement_price);
                let drift_bps = (drift_milli.saturating_mul(10_000))
                    .checked_div(current_mid_milli)
                    .unwrap_or(0);
                if drift_bps > drift_bps_threshold as u64 + placement_bps {
                    evicted.push((o.intent_id, RepriceReason::MidDrift));
                    return false;
                }
            }
            true
        });

        evicted
    }

    /// Record a (partial) fill against a tracked layer. If the layer is
    /// fully consumed it is dropped from the ladder.
    pub fn on_fill(&self, intent_id: u64, filled_usdc_mil: u64) {
        for mut entry in self.per_market.iter_mut() {
            let state = entry.value_mut();
            if let Some(pos) = state
                .orders
                .iter()
                .position(|o| o.intent_id == intent_id)
            {
                let o = &mut state.orders[pos];
                o.filled_usdc_mil = o.filled_usdc_mil.saturating_add(filled_usdc_mil);
                let remaining = self
                    .config
                    .per_level_notional_usdc_mil
                    .saturating_sub(o.filled_usdc_mil);
                if remaining == 0 {
                    state.orders.swap_remove(pos);
                }
                return;
            }
        }
    }

    /// Drop a cancelled layer from the ladder.
    pub fn on_cancel(&self, intent_id: u64) {
        for mut entry in self.per_market.iter_mut() {
            let state = entry.value_mut();
            if let Some(pos) = state
                .orders
                .iter()
                .position(|o| o.intent_id == intent_id)
            {
                state.orders.swap_remove(pos);
                return;
            }
        }
    }

    /// Peak active layer count across markets (for the
    /// `maker_active_layers` gauge).
    pub fn max_active_layers(&self) -> u64 {
        self.per_market
            .iter()
            .map(|e| e.value().orders.len() as u64)
            .max()
            .unwrap_or(0)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_spec() {
        let c = LayerConfig::default();
        assert_eq!(c.levels, 3);
        assert_eq!(c.step_bps, 20);
        assert_eq!(c.per_level_notional_usdc_mil, 25_000);
        assert_eq!(c.max_age_ms, 5_000);
    }

    #[test]
    fn plan_layers_generates_missing_levels_only() {
        let eng = MakerLayerEngine::default();
        let mid = 500_000u64; // $0.500 in milli
        let budget = 1_000_000u64; // plenty
        let first = eng.plan_layers("m1", "t1", OrderSide::Buy, mid, budget);
        assert_eq!(first.len(), 3);
        // Prices strictly below mid and monotonically decreasing.
        assert!(first.iter().all(|i| i.price_u64 < mid));
        let prices: Vec<u64> = first.iter().map(|i| i.price_u64).collect();
        for w in prices.windows(2) {
            assert!(w[0] > w[1]);
        }
        // Second call finds all levels already live — nothing new.
        let again = eng.plan_layers("m1", "t1", OrderSide::Buy, mid, budget);
        assert!(again.is_empty());
    }

    #[test]
    fn plan_layers_respects_budget() {
        let eng = MakerLayerEngine::default();
        let mid = 500_000u64;
        // Only enough for two layers.
        let budget = 50_000u64;
        let out = eng.plan_layers("m1", "t1", OrderSide::Sell, mid, budget);
        assert_eq!(out.len(), 2);
        // Ask side — prices strictly above mid.
        assert!(out.iter().all(|i| i.price_u64 > mid));
    }

    #[test]
    fn on_fill_full_removes_layer() {
        let eng = MakerLayerEngine::default();
        let mid = 500_000u64;
        let intents = eng.plan_layers("m1", "t1", OrderSide::Buy, mid, 1_000_000);
        let id = intents[0].intent_id;
        eng.on_fill(id, 25_000);
        // Layer fully filled — gone from ladder; a re-plan can refill it.
        let again = eng.plan_layers("m1", "t1", OrderSide::Buy, mid, 1_000_000);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].price_u64, intents[0].price_u64);
    }

    #[test]
    fn on_cancel_clears_layer() {
        let eng = MakerLayerEngine::default();
        let mid = 500_000u64;
        let intents = eng.plan_layers("m1", "t1", OrderSide::Buy, mid, 1_000_000);
        eng.on_cancel(intents[1].intent_id);
        let again = eng.plan_layers("m1", "t1", OrderSide::Buy, mid, 1_000_000);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].price_u64, intents[1].price_u64);
    }
}
