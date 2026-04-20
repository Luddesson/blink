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
        }
    }

    pub fn transition(&mut self, new_state: OrderState) {
        self.state = new_state;
        self.last_updated = Instant::now();
    }
}
