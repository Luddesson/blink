//! Newtypes and the borrowed `IntentFields` bundle.
//!
//! The kernel deliberately defines its **own** `PriceTicks(u32)` distinct
//! from `blink_types::PriceTicks(u64)` — the boundary adapter widens at
//! the `verdict_to_outcome` step. Keeping the hot-path integer narrow
//! preserves i128 headroom for the risk-gate arithmetic.

/// Share quantity (CTF unit). Unsigned.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SharesU(pub u64);

/// USDC micro-unit notional. Unsigned.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct NotionalUUsdc(pub u64);

/// Price in ticks (Polymarket × 1000 convention). Narrowed to `u32` in
/// the kernel; widened to `blink_types::PriceTicks(u64)` at the boundary.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PriceTicks(pub u32);

/// Keccak256 over the canonical serialisation of an `IntentFields` value,
/// **excluding** any client-order-id. Used as both the parity fingerprint
/// and the dedup key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct SemanticIntentKey(pub [u8; 32]);

/// Fields describing the intent the kernel would submit. Borrows its
/// string ids from the snapshot so the hot path does no heap work.
///
/// The boundary adapter (`verdict_to_outcome`) is responsible for
/// converting this into the owned `blink_types::Intent` outside the
/// `tsc_decide` span.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct IntentFields<'a> {
    /// Polymarket token id (hex).
    pub token_id: &'a str,
    /// Polymarket condition / market id (hex).
    pub market_id: &'a str,
    /// Trade direction.
    pub side: blink_types::Side,
    /// Limit price in ticks.
    pub price: PriceTicks,
    /// Share quantity.
    pub size: SharesU,
    /// Time-in-force.
    pub tif: blink_types::TimeInForce,
    /// Post-only honour flag.
    pub post_only: bool,
}

impl<'a> IntentFields<'a> {
    /// Serialise the intent into the exact byte layout hashed by
    /// [`SemanticIntentKey`]:
    ///
    /// ```text
    ///   token_id.as_bytes()
    /// ‖ 0xFF                       # delimiter — not valid UTF-8
    /// ‖ market_id.as_bytes()
    /// ‖ 0xFF
    /// ‖ side   (u8)
    /// ‖ price  (u32, little endian)
    /// ‖ size   (u64, little endian)
    /// ‖ tif    (u8)
    /// ‖ post_only (u8)
    /// ```
    ///
    /// The 0xFF delimiters prevent length-extension collisions between
    /// `(token="ab", market="cd")` and `(token="abcd", market="")`.
    #[inline]
    pub fn semantic_key(&self) -> SemanticIntentKey {
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        h.update(self.token_id.as_bytes());
        h.update([0xFFu8]);
        h.update(self.market_id.as_bytes());
        h.update([0xFFu8]);
        h.update([self.side as u8]);
        h.update(self.price.0.to_le_bytes());
        h.update(self.size.0.to_le_bytes());
        h.update([self.tif as u8]);
        h.update([self.post_only as u8]);
        SemanticIntentKey(h.finalize().into())
    }
}
