//! `DecisionKernel` trait and zero-alloc `KernelVerdict`.

use crate::{snapshot::DecisionSnapshot, stats::KernelStats, IntentFields, SemanticIntentKey};
use blink_shadow::NoOpCode;
use blink_types::AbortReason;

/// Zero-allocation verdict returned by a kernel's `decide` call.
///
/// The lifetime `'a` ties the borrowed `IntentFields` to the
/// `DecisionSnapshot`'s lifetime: the caller MUST consume the verdict
/// (e.g. pass it to [`crate::verdict_to_outcome`]) before the snapshot
/// goes out of scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelVerdict<'a> {
    /// The kernel chose to submit. No COID yet — that is assigned by the
    /// boundary adapter.
    Submit {
        /// Parity / dedup key over the borrowed fields.
        semantic_key: SemanticIntentKey,
        /// Borrowed fields describing the intent.
        fields: IntentFields<'a>,
    },
    /// Abort with a structured reason, optionally carrying a signed
    /// bps metric (e.g. measured drift).
    Abort {
        /// Abort reason. `blink_types::AbortReason` is `#[repr(u8)]`
        /// with no String fields so no mirror is needed.
        reason: AbortReason,
        /// For `Drift`, the measured bps.
        metric_bps: Option<i32>,
    },
    /// Passive skip, structured code (stable across wording changes).
    NoOp {
        /// Stable structured reason.
        code: NoOpCode,
    },
}

/// A decision kernel.
///
/// Implementations are expected to be pure: given identical
/// `DecisionSnapshot`s they MUST return identical verdicts. The shadow
/// runner will flag any violation as a divergence.
pub trait DecisionKernel: Send + Sync {
    /// Stable implementation identifier (used by the shadow runner to
    /// refuse a self-vs-self comparison).
    fn impl_id(&self) -> &'static str;

    /// Produce a verdict for one captured snapshot.
    fn decide<'a>(
        &self,
        snapshot: &DecisionSnapshot<'a>,
        stats: &mut KernelStats,
    ) -> KernelVerdict<'a>;
}
