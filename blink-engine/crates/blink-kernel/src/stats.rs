//! Kernel-side counters. Mutated in-place by `decide`; held by the caller
//! on the same thread so no atomics are required.

use blink_shadow::NoOpCode;
use blink_types::AbortReason;

/// Per-kernel counters. Indexed by the `u8` repr of `AbortReason` /
/// `NoOpCode`. Arrays are fixed-size so there's no heap indirection.
#[derive(Debug, Default, Clone)]
pub struct KernelStats {
    /// Total calls to `decide`.
    pub decisions_total: u64,
    /// Count of `KernelVerdict::Submit`.
    pub submitted: u64,
    /// Count of each `AbortReason`. Indexed by `r as u8`.
    pub aborted: [u64; 8],
    /// Count of each `NoOpCode`. Indexed by `c as u32 as usize`.
    pub noop: [u64; 6],
    /// Count of risk-gate denials caused by i128 overflow of the
    /// computed position-notional intermediate — distinct from ordinary
    /// `RiskLimit` aborts because the underlying input was adversarial.
    pub risk_denied_i128_overflow: u64,
}

impl KernelStats {
    /// Fresh zeroed counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a `Submit` outcome.
    #[inline]
    pub(crate) fn bump_submit(&mut self) {
        self.submitted = self.submitted.saturating_add(1);
    }

    /// Record an `Abort{reason}`. Safe to call with any
    /// `blink_types::AbortReason`.
    #[inline]
    pub(crate) fn bump_abort(&mut self, reason: AbortReason) {
        let idx = reason as u8 as usize;
        if idx < self.aborted.len() {
            self.aborted[idx] = self.aborted[idx].saturating_add(1);
        }
    }

    /// Record a `NoOp{code}`.
    #[inline]
    pub(crate) fn bump_noop(&mut self, code: NoOpCode) {
        let idx = code as u32 as usize;
        if idx < self.noop.len() {
            self.noop[idx] = self.noop[idx].saturating_add(1);
        }
    }
}
