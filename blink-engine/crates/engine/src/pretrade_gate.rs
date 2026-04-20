//! Pre-trade freshness/drift gate — replaces the fixed 3 s fill-window.
//!
//! All arithmetic is integer-only (prices × 1 000).
//!
//! # Environment variables
//! - `BLINK_GATE_STALE_MS` — max book age before SkipStale (default 800 ms)
//! - `BLINK_GATE_MAX_DRIFT_BPS` — max |price − ref| / ref × 10 000 (default 80 bps)
//! - `BLINK_GATE_POST_ONLY` — enable post-only cross check (default true)

use std::sync::Arc;

use crate::order_book::OrderBookStore;
use crate::types::OrderSide;

/// Outcome of a pre-trade gate check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Proceed,
    SkipStale,
    SkipDrift { bps: i64 },
    SkipPostOnlyCross,
}

pub struct PretradeGate {
    book: Arc<OrderBookStore>,
}

impl PretradeGate {
    pub fn new(book: Arc<OrderBookStore>) -> Self {
        Self { book }
    }

    /// Reads config from env on each call (cached by caller if performance-critical).
    pub fn check(
        &self,
        token_id: &str,
        side: OrderSide,
        price_u64: u64,
        snapshot_age_ms_max: u32,
        max_drift_bps: u16,
        post_only: bool,
    ) -> GateDecision {
        // 1. Freshness check
        match self.book.get_snapshot_age_ms(token_id) {
            None => return GateDecision::SkipStale,
            Some(age_ms) if age_ms > snapshot_age_ms_max => return GateDecision::SkipStale,
            _ => {}
        }

        // 2. Get best bid/ask for drift and post-only
        let book_snap = match self.book.get_book_snapshot(token_id) {
            Some(b) => b,
            None => return GateDecision::SkipStale,
        };

        // Reference price: best_ask for buys (we buy at ask), best_bid for sells
        // (we sell into bid). Use mid when side-specific best unavailable.
        let reference_u64 = match side {
            OrderSide::Buy => book_snap.best_ask().or_else(|| book_snap.mid_price()),
            OrderSide::Sell => book_snap.best_bid().or_else(|| book_snap.mid_price()),
        };

        let reference_u64 = match reference_u64 {
            Some(r) if r > 0 => r,
            _ => return GateDecision::SkipStale,
        };

        // 3. Drift check — integer arithmetic only
        let diff = if price_u64 > reference_u64 {
            price_u64 - reference_u64
        } else {
            reference_u64 - price_u64
        };
        let drift_bps = (diff * 10_000) / reference_u64;
        if drift_bps > max_drift_bps as u64 {
            return GateDecision::SkipDrift {
                bps: drift_bps as i64,
            };
        }

        // 4. Post-only cross check
        if post_only {
            match side {
                OrderSide::Buy => {
                    if let Some(best_ask) = book_snap.best_ask() {
                        if price_u64 >= best_ask {
                            return GateDecision::SkipPostOnlyCross;
                        }
                    }
                }
                OrderSide::Sell => {
                    if let Some(best_bid) = book_snap.best_bid() {
                        if price_u64 <= best_bid {
                            return GateDecision::SkipPostOnlyCross;
                        }
                    }
                }
            }
        }

        GateDecision::Proceed
    }
}

/// Read gate config from environment (call once per signal, not per gate method).
pub struct GateConfig {
    pub stale_ms: u32,
    pub max_drift_bps: u16,
    pub post_only: bool,
}

impl GateConfig {
    /// Reads the gate config using the active [`ExecutionProfile`] for
    /// defaults. Per-knob env vars still override when set.
    pub fn from_env() -> Self {
        Self::from_profile_and_env(crate::execution_profile::ExecutionProfile::from_env())
    }

    /// Resolve gate config from a specific execution profile. Per-knob env
    /// overrides (`BLINK_GATE_STALE_MS`, `BLINK_GATE_MAX_DRIFT_BPS`,
    /// `BLINK_GATE_POST_ONLY`) still win when present.
    pub fn from_profile_and_env(
        profile: crate::execution_profile::ExecutionProfile,
    ) -> Self {
        let knobs = profile.knobs();
        let stale_ms = std::env::var("BLINK_GATE_STALE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(knobs.pretrade_gate_stale_ms);
        let max_drift_bps = std::env::var("BLINK_GATE_MAX_DRIFT_BPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(knobs.pretrade_gate_drift_bps);
        let post_only = std::env::var("BLINK_GATE_POST_ONLY")
            .ok()
            .map(|v| !matches!(v.to_lowercase().as_str(), "false" | "0"))
            .unwrap_or(knobs.post_only);
        Self {
            stale_ms,
            max_drift_bps,
            post_only,
        }
    }
}
