//! Atomic hot-path counters for [`Submitter`](super::submitter::Submitter).

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters. Cheap to read/write across threads.
#[derive(Debug, Default)]
pub struct SubmitterStats {
    pub submits_total: AtomicU64,
    pub submits_accepted: AtomicU64,
    pub submits_rejected: AtomicU64,
    pub submits_dedup: AtomicU64,
    pub submits_unknown: AtomicU64,
    pub signer_time_ns_total: AtomicU64,
    pub h2_time_ns_total: AtomicU64,
    pub encode_time_ns_total: AtomicU64,
}

/// Point-in-time snapshot of [`SubmitterStats`]. `Copy` for cheap hand-off
/// to metrics exporters.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubmitterStatsSnapshot {
    pub submits_total: u64,
    pub submits_accepted: u64,
    pub submits_rejected: u64,
    pub submits_dedup: u64,
    pub submits_unknown: u64,
    pub signer_time_ns_total: u64,
    pub h2_time_ns_total: u64,
    pub encode_time_ns_total: u64,
}

impl SubmitterStats {
    #[inline]
    pub fn snapshot(&self) -> SubmitterStatsSnapshot {
        SubmitterStatsSnapshot {
            submits_total: self.submits_total.load(Ordering::Relaxed),
            submits_accepted: self.submits_accepted.load(Ordering::Relaxed),
            submits_rejected: self.submits_rejected.load(Ordering::Relaxed),
            submits_dedup: self.submits_dedup.load(Ordering::Relaxed),
            submits_unknown: self.submits_unknown.load(Ordering::Relaxed),
            signer_time_ns_total: self.signer_time_ns_total.load(Ordering::Relaxed),
            h2_time_ns_total: self.h2_time_ns_total.load(Ordering::Relaxed),
            encode_time_ns_total: self.encode_time_ns_total.load(Ordering::Relaxed),
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn add(&self, field: &AtomicU64, n: u64) {
        field.fetch_add(n, Ordering::Relaxed);
    }
}
