//! Frozen kernel configuration. Held behind `Arc<KernelConfig>` by the
//! caller; the kernel reads only through a `&KernelConfig` reference
//! captured inside the [`crate::DecisionSnapshot`].
//!
//! The config hash is computed once (at construction) by `config_hash` so
//! replay parity checks do not pay for the hash on every decision.

use sha3::{Digest, Keccak256};

/// Immutable per-decision configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelConfig {
    /// Maximum allowed book-snapshot age in nanoseconds (inclusive).
    pub book_max_age_ns: u64,
    /// Maximum allowed drift between our limit price and the book
    /// midpoint, in basis points (of mid).
    pub max_drift_bps: u32,
    /// Position notional cap, USDC µ-units. If a proposed fill would
    /// cause `|new_qty| * price` to exceed this cap, the kernel emits
    /// `Abort{RiskLimit}`.
    pub max_position_notional: u64,
    /// Minimum edge (bps of limit price) to clear before submitting; any
    /// smaller edge yields `NoOp{BelowEdgeThreshold}`.
    pub edge_threshold_bps: i32,
    /// Default `post_only` flag applied to all intents. `RawEvent` carries
    /// no explicit post-only bit today; downstream strategies may want to
    /// enforce maker-only behaviour via config. `false` matches legacy.
    pub default_post_only: bool,
}

impl KernelConfig {
    /// Conservative default. Explicit — no `Default` derive, because
    /// silent defaults in a hot-path kernel are a footgun.
    pub fn conservative() -> Self {
        Self {
            book_max_age_ns: 800 * 1_000_000, // 800 ms
            max_drift_bps: 50,
            max_position_notional: 1_000_000_000, // 1000 USDC
            edge_threshold_bps: 5,
            default_post_only: false,
        }
    }

    /// Deterministic 32-byte digest for parity.
    pub fn config_hash(&self) -> [u8; 32] {
        let mut h = Keccak256::new();
        h.update(self.book_max_age_ns.to_le_bytes());
        h.update(self.max_drift_bps.to_le_bytes());
        h.update(self.max_position_notional.to_le_bytes());
        h.update(self.edge_threshold_bps.to_le_bytes());
        h.update([self.default_post_only as u8]);
        h.finalize().into()
    }
}
