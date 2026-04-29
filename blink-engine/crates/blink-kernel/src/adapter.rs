//! Boundary adapter: translate a `KernelVerdict` (borrowed, zero-alloc)
//! into a journal-visible `blink_types::DecisionOutcome` (owned,
//! allocating).
//!
//! **Call site discipline.** This function allocates (String fields on
//! `Intent`, hex COID). It MUST be invoked **outside** the
//! `tsc_decide .. tsc_submit_queue` timing span, after the kernel has
//! returned.

use crate::{kernel::KernelVerdict, types::SemanticIntentKey};
use blink_types::{DecisionOutcome, IntentHash};
use sha3::{Digest, Keccak256};

/// Deterministic client-order-id (COID) generation for parity:
/// first 16 bytes of `keccak256(semantic_key ‖ run_id ‖ attempt)`,
/// rendered as 32 lowercase hex chars.
pub fn derive_coid(semantic_key: &SemanticIntentKey, run_id: &[u8; 16], attempt: u8) -> String {
    let mut h = Keccak256::new();
    h.update(semantic_key.0);
    h.update(run_id);
    h.update([attempt]);
    let digest: [u8; 32] = h.finalize().into();
    hex::encode(&digest[..16])
}

/// Deterministic `IntentHash` that binds semantic_key + COID. Distinct
/// from `SemanticIntentKey` (which excludes COID) so downstream diffs
/// can show the full-submit hash that actually reaches the venue.
pub fn derive_intent_hash(semantic_key: &SemanticIntentKey, coid: &str) -> IntentHash {
    let mut h = Keccak256::new();
    h.update(semantic_key.0);
    h.update([0xFFu8]);
    h.update(coid.as_bytes());
    IntentHash(h.finalize().into())
}

/// Convert a kernel verdict into the owned journal outcome. Allocates.
///
/// - `Submit` becomes `DecisionOutcome::Submitted { intent_hash, coid }`.
///   The matching `blink_types::Intent` is reconstructable by the caller
///   from the same `semantic_key + run_id + attempt` if needed.
/// - `Abort` passes the repr-u8 reason through directly.
/// - `NoOp` stringifies the structured code with a stable format so the
///   shadow fingerprint's `classify_noop` round-trips back to the
///   original [`blink_shadow::NoOpCode`].
pub fn verdict_to_outcome(
    verdict: KernelVerdict<'_>,
    run_id: &[u8; 16],
    attempt: u8,
) -> DecisionOutcome {
    match verdict {
        KernelVerdict::Submit { semantic_key, fields: _ } => {
            let coid = derive_coid(&semantic_key, run_id, attempt);
            let intent_hash = derive_intent_hash(&semantic_key, &coid);
            DecisionOutcome::Submitted { intent_hash, client_order_id: coid }
        }
        KernelVerdict::Abort { reason, metric_bps } => DecisionOutcome::Aborted {
            reason,
            metric: metric_bps.map(|b| b as i64),
        },
        KernelVerdict::NoOp { code } => DecisionOutcome::NoOp {
            reason: noop_code_string(code).to_string(),
        },
    }
}

/// Stable string that `blink_shadow::classify_noop` maps back to `code`.
fn noop_code_string(code: blink_shadow::NoOpCode) -> &'static str {
    use blink_shadow::NoOpCode;
    match code {
        NoOpCode::Unknown => "unknown",
        NoOpCode::BelowEdgeThreshold => "below edge threshold",
        NoOpCode::CooldownActive => "cooldown active",
        NoOpCode::InventorySaturated => "inventory saturated",
        NoOpCode::FilterMismatch => "filter mismatch",
        NoOpCode::Dedup => "dedup",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blink_shadow::{classify_noop, NoOpCode};

    #[test]
    fn noop_strings_roundtrip_through_classify() {
        for code in [
            NoOpCode::BelowEdgeThreshold,
            NoOpCode::CooldownActive,
            NoOpCode::InventorySaturated,
            NoOpCode::FilterMismatch,
            NoOpCode::Dedup,
        ] {
            let s = noop_code_string(code);
            assert_eq!(classify_noop(s), code, "noop string {s:?} did not roundtrip");
        }
    }

    #[test]
    fn coid_is_deterministic_and_len_32() {
        let sk = SemanticIntentKey([7u8; 32]);
        let a = derive_coid(&sk, &[1u8; 16], 0);
        let b = derive_coid(&sk, &[1u8; 16], 0);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        let c = derive_coid(&sk, &[1u8; 16], 1);
        assert_ne!(a, c, "distinct attempts must produce distinct COIDs");
    }
}
