//! Deterministic fingerprint of a [`DecisionOutcome`].
//!
//! The rubber-duck review rejected hashing via `bincode::serialize`
//! because (a) bincode layout is not a stable wire format and (b) a
//! single compiler version bump silently re-hashes every outcome. This
//! module hand-rolls the buffer byte-for-byte, gated by the constant
//! [`VERSION`].
//!
//! # Nondeterminism scrubs
//! - `client_order_id` is deliberately **excluded** from Submit
//!   fingerprints: today it embeds a nonce so the legacy and v2
//!   kernels will never agree on it.
//! - For NoOps the free-text `reason: String` is mapped through
//!   [`classify_noop`] to a stable [`NoOpCode`]; minor wording changes
//!   therefore do not flip parity.
//! - For Aborts, the `metric: Option<i64>` side-channel is **not**
//!   included. It's noisy by design (bps readouts differ by tiny
//!   fractions between impls) and shouldn't trip parity on its own.
//!
//! # Deviation from the task spec
//! The task sketches the Submit fingerprint as hashing `side`, `tif`,
//! `post_only`, `price_ticks`, `size_u`, `token_id`, `market_id`,
//! `event_id` directly. The actual [`blink_types::DecisionOutcome::Submitted`]
//! variant does **not** carry those fields — it carries a pre-computed
//! [`IntentHash`] (a 32-byte deterministic hash of the Intent) plus the
//! `client_order_id`. We fingerprint over the `intent_hash` bytes
//! instead; the parity guarantee is identical because `intent_hash`
//! *is* the deterministic digest of exactly those fields.
//!
//! [`IntentHash`]: blink_types::IntentHash

use blink_types::{AbortReason, DecisionOutcome};
use sha3::{Digest, Keccak256};

/// Wire-format version tag. Bump if you change the byte layout; old
/// fingerprints stored anywhere will be unreadable and should be
/// regenerated.
pub const VERSION: u8 = 1;

const KIND_SUBMIT: u8 = 0;
const KIND_ABORT: u8 = 1;
const KIND_NOOP: u8 = 2;

/// Structured NoOp reason. Stays local to `blink-shadow` for the MVP —
/// promoting this into `blink-types` is a larger contract change the
/// rubber-duck deliberately deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum NoOpCode {
    /// Free-text reason we don't recognize. Visible but noisy.
    Unknown = 0,
    /// Edge below the strategy's minimum profitable spread.
    BelowEdgeThreshold = 1,
    /// Per-market cooldown is still active.
    CooldownActive = 2,
    /// Inventory limit reached; no more orders until position unwinds.
    InventorySaturated = 3,
    /// Event did not match the kernel's filter predicate.
    FilterMismatch = 4,
    /// Dedup cache hit.
    Dedup = 5,
}

/// Map a free-text NoOp reason onto a [`NoOpCode`]. Matches are
/// case-insensitive substring contains; unknown strings map to
/// [`NoOpCode::Unknown`] so they're visible in reports without
/// tripping parity on every wording tweak.
pub fn classify_noop(reason: &str) -> NoOpCode {
    let r = reason.to_ascii_lowercase();
    if r.contains("edge") {
        NoOpCode::BelowEdgeThreshold
    } else if r.contains("cooldown") {
        NoOpCode::CooldownActive
    } else if r.contains("inventory") || r.contains("saturat") {
        NoOpCode::InventorySaturated
    } else if r.contains("filter") || r.contains("mismatch") {
        NoOpCode::FilterMismatch
    } else if r.contains("dedup") || r.contains("duplicate") {
        NoOpCode::Dedup
    } else {
        NoOpCode::Unknown
    }
}

/// Marker type documenting the fingerprint schema version. Not
/// instantiated at runtime — the constant [`VERSION`] is what's baked
/// into the hash.
#[derive(Debug, Clone, Copy)]
pub struct OutcomeFingerprintV1;

/// Hand-rolled byte-for-byte serialisation, then `keccak256`.
///
/// Layout (see module docs):
/// - `u8` version
/// - `u8` kind discriminant
/// - kind-specific payload
pub fn fingerprint(outcome: &DecisionOutcome) -> [u8; 32] {
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    buf.push(VERSION);
    match outcome {
        DecisionOutcome::Submitted {
            intent_hash,
            client_order_id: _, // deliberately excluded
        } => {
            buf.push(KIND_SUBMIT);
            buf.extend_from_slice(&intent_hash.0);
        }
        DecisionOutcome::Aborted { reason, metric: _ } => {
            buf.push(KIND_ABORT);
            buf.push(*reason as u8);
        }
        DecisionOutcome::NoOp { reason } => {
            buf.push(KIND_NOOP);
            let code = classify_noop(reason) as u32;
            buf.extend_from_slice(&code.to_le_bytes());
        }
    }
    let mut h = Keccak256::new();
    h.update(&buf);
    h.finalize().into()
}

/// Short human-readable summary of an outcome — used inside
/// [`crate::divergence::DivergenceRecord`] so reports are grep-able
/// without dragging the full variant in.
pub fn summarize(outcome: &DecisionOutcome) -> String {
    match outcome {
        DecisionOutcome::Submitted {
            intent_hash,
            client_order_id,
        } => format!(
            "Submitted(ih={},coid={})",
            hex8(&intent_hash.0),
            client_order_id
        ),
        DecisionOutcome::Aborted { reason, metric } => {
            format!("Aborted({:?},metric={:?})", reason, metric)
        }
        DecisionOutcome::NoOp { reason } => {
            format!("NoOp({:?},{:?})", classify_noop(reason), reason)
        }
    }
}

fn hex8(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(16);
    for b in &bytes[..8] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

// ─── Map abort reason for callers (non-pub: used only by tests today) ─

#[allow(dead_code)]
fn abort_reason_u8(r: AbortReason) -> u8 {
    r as u8
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blink_types::{AbortReason, DecisionOutcome, IntentHash};

    fn noop(s: &str) -> DecisionOutcome {
        DecisionOutcome::NoOp {
            reason: s.to_string(),
        }
    }

    fn abort(r: AbortReason) -> DecisionOutcome {
        DecisionOutcome::Aborted {
            reason: r,
            metric: None,
        }
    }

    fn submit(ih: [u8; 32], coid: &str) -> DecisionOutcome {
        DecisionOutcome::Submitted {
            intent_hash: IntentHash(ih),
            client_order_id: coid.to_string(),
        }
    }

    /// Wire-format guard. If this test ever has to be updated, bump
    /// `VERSION` and plan a coordinated rotation — see module docs.
    #[test]
    fn fingerprint_is_byte_stable_for_canned_inputs() {
        // keccak256([0x01, 0x02, 0x00, 0x00, 0x00, 0x00]) — NoOp/Unknown.
        let got = fingerprint(&noop(""));
        let expected: [u8; 32] = [
            0x5a, 0xef, 0x29, 0xb9, 0x76, 0x57, 0xec, 0x83,
            0xa8, 0xdb, 0x68, 0xe0, 0x28, 0x91, 0xf4, 0x30,
            0x2f, 0xbd, 0x30, 0x81, 0x70, 0x4a, 0x44, 0x2a,
            0x55, 0x44, 0x49, 0x13, 0x80, 0x34, 0x8e, 0x6b,
        ];
        assert_eq!(
            got, expected,
            "wire-format changed; see fingerprint.rs module docs"
        );
    }

    #[test]
    fn submit_abort_noop_have_distinct_fingerprints() {
        let a = fingerprint(&submit([1u8; 32], "coid-a"));
        let b = fingerprint(&abort(AbortReason::Drift));
        let c = fingerprint(&noop("below edge threshold"));
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn noop_freetext_collapses_to_code() {
        let a = fingerprint(&noop("below edge threshold"));
        let b = fingerprint(&noop("Below Edge Threshold (0.3 bps)"));
        assert_eq!(classify_noop("below edge threshold"), NoOpCode::BelowEdgeThreshold);
        assert_eq!(a, b);
    }

    #[test]
    fn submit_ignores_client_order_id() {
        let a = fingerprint(&submit([9u8; 32], "coid-a"));
        let b = fingerprint(&submit([9u8; 32], "something-else"));
        assert_eq!(a, b);
    }

    #[test]
    fn submit_cares_about_intent_hash() {
        let a = fingerprint(&submit([9u8; 32], "coid"));
        let b = fingerprint(&submit([10u8; 32], "coid"));
        assert_ne!(a, b);
    }

    #[test]
    fn abort_reasons_distinguish() {
        let a = fingerprint(&abort(AbortReason::Drift));
        let b = fingerprint(&abort(AbortReason::StaleBook));
        assert_ne!(a, b);
    }

    #[test]
    fn unknown_noop_reasons_are_equal() {
        // Both map to Unknown so parity is preserved; surface via report.
        let a = fingerprint(&noop("wizard went off to lunch"));
        let b = fingerprint(&noop("universe forgot to exist"));
        assert_eq!(a, b);
    }
}
