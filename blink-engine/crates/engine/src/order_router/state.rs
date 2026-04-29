//! PendingOrder state machine for the order router.
//!
//! State transitions:
//!
//! ```text
//!  Created → Submitting → Acked       → PartialFilled → Filled
//!                       → SubmitUnknown → (reconciled) → Acked / Rejected
//!                       → Rejected
//!          → Cancelling → Cancelled
//!          → Stale
//! ```

use std::time::Instant;

use crate::types::OrderSide;

/// Lifecycle state of an order managed by the router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderState {
    Created,
    Submitting,
    SubmitUnknown,
    Acked,
    PartialFilled,
    Filled,
    Cancelling,
    Cancelled,
    Rejected,
    Stale,
}

impl OrderState {
    /// Returns true when this state is terminal (no further transitions expected).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            OrderState::Filled | OrderState::Cancelled | OrderState::Rejected | OrderState::Stale
        )
    }
}

/// A submitted order tracked by the router's `PendingOrderStore`.
#[derive(Debug, Clone)]
pub struct PendingOrder {
    pub intent_id: u64,
    pub order_id: Option<String>,
    pub client_order_id: String,
    pub market_id: String,
    pub token_id: String,
    pub side: OrderSide,
    pub size_usdc: f64,
    /// Integer-scaled size (USDC × 1_000) used by the risk gate.
    pub size_u64: u64,
    pub entry_price: f64,
    pub state: OrderState,
    pub submit_attempts: u32,
    pub created_at: Instant,
    pub last_updated: Instant,
    /// Entered terminal state at this instant (for GC age tracking).
    pub terminal_at: Option<Instant>,

    // ── SubmitUnknown resolver ────────────────────────────────────────────────
    /// Number of `find_order_by_client_id` lookups performed so far.
    pub lookup_attempts: u32,

    // ── Fill accounting (integer math — milliUSDC = value × 1_000) ───────────
    /// Total filled size in milliUSDC (integer; avoids f64 in hot path).
    pub filled_size_u64: u64,
    /// Remaining unfilled size in milliUSDC.
    pub remaining_size_u64: u64,
    /// Volume-weighted average fill price in millicents (price × 1_000).
    pub avg_fill_price_u64: u64,
    /// Instant of most recent fill update (`None` = never filled).
    pub last_fill_ts: Option<Instant>,

    // ── Cancel tracking ───────────────────────────────────────────────────────
    /// Number of cancel attempts sent to the exchange.
    pub cancel_attempts: u8,
}

impl PendingOrder {
    pub fn new(
        intent_id: u64,
        market_id: String,
        token_id: String,
        side: OrderSide,
        size_usdc: f64,
        size_u64: u64,
        entry_price: f64,
    ) -> Self {
        let now = Instant::now();
        let size_u64 = (size_usdc * 1_000.0) as u64;
        Self {
            intent_id,
            order_id: None,
            client_order_id: format!("blk-{intent_id}"),
            market_id,
            token_id,
            side,
            size_usdc,
            size_u64,
            entry_price,
            state: OrderState::Created,
            submit_attempts: 0,
            created_at: now,
            last_updated: now,
            terminal_at: None,
            lookup_attempts: 0,
            filled_size_u64: 0,
            remaining_size_u64: size_u64,
            avg_fill_price_u64: 0,
            last_fill_ts: None,
            cancel_attempts: 0,
        }
    }

    pub fn transition(&mut self, new_state: OrderState) {
        if new_state.is_terminal() && self.terminal_at.is_none() {
            self.terminal_at = Some(Instant::now());
        }
        self.state = new_state;
        self.last_updated = Instant::now();
    }

    /// Apply a fill update using integer math.
    ///
    /// `new_filled_u64` is the *cumulative* filled size in milliUSDC.
    /// `fill_price_u64` is the fill price in millicents.
    /// Returns the delta (how much new fill this update adds).
    pub fn apply_fill_update(&mut self, new_filled_u64: u64, fill_price_u64: u64) -> u64 {
        let delta = new_filled_u64.saturating_sub(self.filled_size_u64);
        if delta == 0 {
            return 0;
        }
        let total = self.filled_size_u64 + delta;
        // Volume-weighted average price (integer arithmetic).
        if total > 0 {
            self.avg_fill_price_u64 =
                (self.avg_fill_price_u64 * self.filled_size_u64 + fill_price_u64 * delta) / total;
        }
        self.filled_size_u64 = total;
        let size_u64 = (self.size_usdc * 1_000.0) as u64;
        self.remaining_size_u64 = size_u64.saturating_sub(total);
        self.last_fill_ts = Some(Instant::now());
        delta
    }
}
