//! `RouterFillHook` — callback interface for fill and cancel events.
//!
//! Implementors (risk manager, portfolio, paper engine) register via
//! `Arc<dyn RouterFillHook>` and are called by the reconciler on every fill
//! update.  The interface is intentionally synchronous (no `async`) to keep
//! the hot path allocation-free.

use crate::types::OrderSide;

#[derive(Debug, Clone)]
pub struct RouterFillEvent {
    pub intent_id: u64,
    pub order_id: Option<String>,
    pub token_id: String,
    pub side: OrderSide,
    /// Newly confirmed fill notional in milliUSDC.
    pub delta_size_u64: u64,
    /// Cumulative confirmed fill notional in milliUSDC.
    pub cumulative_size_u64: u64,
    /// Remaining unfilled notional in milliUSDC.
    pub remaining_size_u64: u64,
    /// Fill/limit price scaled x1_000.
    pub fill_price_u64: u64,
    pub is_full: bool,
}

/// Notification interface for order fill events emitted by the reconciler.
///
/// Implementations must be `Send + Sync` (called from the reconciler task).
/// All size/price arguments use integer milliUSDC units (value × 1_000)
/// to avoid floating-point in the hot path.
pub trait RouterFillHook: Send + Sync {
    /// Called for every newly confirmed fill delta.
    fn on_fill_update(&self, event: RouterFillEvent) {
        if event.is_full {
            self.on_full_fill(event.intent_id);
        } else {
            self.on_partial_fill(event.intent_id, event.delta_size_u64, event.fill_price_u64);
        }
    }

    /// Called when a partial fill is confirmed.
    ///
    /// - `intent_id`: the router's intent identifier.
    /// - `delta_size_u64`: newly filled size in milliUSDC (cumulative delta).
    /// - `fill_price_u64`: fill price in millicents (price × 1_000).
    fn on_partial_fill(&self, intent_id: u64, delta_size_u64: u64, fill_price_u64: u64);

    /// Called when an order reaches fully-filled state.
    fn on_full_fill(&self, intent_id: u64);

    /// Called when a cancel is confirmed by the exchange.
    fn on_cancel_confirmed(&self, intent_id: u64) {
        let _ = intent_id;
    }
}

/// No-op implementation used in tests and dry-run contexts.
pub struct NoopFillHook;

impl RouterFillHook for NoopFillHook {
    fn on_fill_update(&self, _event: RouterFillEvent) {}
    fn on_partial_fill(&self, _intent_id: u64, _delta: u64, _price: u64) {}
    fn on_full_fill(&self, _intent_id: u64) {}
}
