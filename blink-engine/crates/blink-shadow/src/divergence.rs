//! Divergence records emitted by [`crate::runner::ShadowRunner`].

use serde::{Deserialize, Serialize};

/// Which field first differed between legacy and v2 outcomes.
///
/// Determined heuristically from the two [`blink_types::DecisionOutcome`]
/// values — the runner walks the variants in a fixed order (variant >
/// reason > numeric payload) so the report points at the most
/// informative mismatch rather than just saying "hashes differ".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DivergenceField {
    /// Variants themselves differ (Submit vs Abort vs NoOp).
    Variant,
    /// Both Aborts but reasons differ.
    AbortReason,
    /// Both NoOps but classified codes differ.
    NoOpCode,
    /// Both Submits but `intent_hash` differs.
    SubmitIntentHash,
    /// Fingerprints differ but the above checks didn't catch it — a
    /// bug in the fingerprint layout. Report it anyway.
    Opaque,
}

/// One recorded parity failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DivergenceRecord {
    /// Event id lifted from [`blink_types::RawEvent::event_id`].
    pub event_id: u64,
    /// Shadow run id from the input.
    pub run_id: u64,
    /// Stable event key.
    pub event_key: String,
    /// Legacy fingerprint.
    pub legacy_fp: [u8; 32],
    /// v2 fingerprint.
    pub v2_fp: [u8; 32],
    /// Short human summary of the legacy outcome.
    pub legacy_outcome_summary: String,
    /// Short human summary of the v2 outcome.
    pub v2_outcome_summary: String,
    /// The first field found to differ.
    pub first_differing_field: DivergenceField,
}
