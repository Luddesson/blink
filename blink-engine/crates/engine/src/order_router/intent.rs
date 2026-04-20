//! Order intent — immutable description of a trading decision.
//!
//! An `OrderIntent` is created in `handle_signal` after all sizing/risk checks
//! have passed. It is handed to `OrderRouter::submit()` for async HTTP
//! submission with idempotent retry.

use std::time::Instant;

use crate::order_signer::SignedOrder;
use crate::strategy::StrategyMode;
use crate::types::{OrderSide, TimeInForce};

/// A fully signed order payload cached on the intent so that retries
/// replay the exact same bytes (same salt, same nonce, same signature).
pub type SignedOrderPayload = SignedOrder;

/// Immutable description of a trading intent. Created once, never mutated.
#[derive(Debug, Clone)]
pub struct OrderIntent {
    /// Monotonic ID generated at signal ingress. Globally unique per session.
    pub intent_id: u64,
    /// Polymarket condition/market ID (optional — may not be known for all signals).
    pub market_id: String,
    /// Polymarket token/asset ID (the long numeric string).
    pub token_id: String,
    pub side: OrderSide,
    /// Limit price scaled ×1 000 (integer; no floats in hot path).
    pub price_u64: u64,
    /// USDC notional scaled ×1 000 (integer; no floats in hot path).
    pub size_u64: u64,
    pub tif: TimeInForce,
    pub strategy_mode: StrategyMode,
    /// Wall-clock time the intent was created (after sizing/risk gates).
    pub requested_at: Instant,
    /// Pre-computed signed EIP-712 payload.
    ///
    /// Set before `router.submit()` so retries do not re-sign with a fresh
    /// salt/nonce — the exact same bytes are replayed to the exchange.
    pub signed_payload: Option<SignedOrderPayload>,
}
