//! Wait-free sliding-window counter for (ok, err, max_latency) samples.
//!
//! Fixed ring of `N_BUCKETS` buckets, each covering `window_ms / N_BUCKETS`.
//! Callers supply `now_ns` — the counter never reads the clock itself.
//!
//! Readers are wait-free (atomic loads). Writers are lock-free using a
//! compare-and-swap on the per-bucket epoch to roll the bucket forward.
//!
//! The counter does no heap allocation after construction.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Number of sub-buckets per window. Fixed so it lives entirely inline.
pub const N_BUCKETS: usize = 16;

/// A single sub-bucket: (epoch, ok_count, err_count, max_latency_ns).
///
/// `epoch` is the integer `now_ns / bucket_ns`. When a record_outcome
/// sees `epoch != current`, it CAS-rolls the epoch and zeroes counters.
#[derive(Debug)]
struct Bucket {
    epoch: AtomicU64,
    ok: AtomicU32,
    err: AtomicU32,
    max_lat_ns: AtomicU64,
}

impl Bucket {
    const fn new() -> Self {
        Self {
            epoch: AtomicU64::new(u64::MAX),
            ok: AtomicU32::new(0),
            err: AtomicU32::new(0),
            max_lat_ns: AtomicU64::new(0),
        }
    }

    #[inline]
    fn roll_to(&self, new_epoch: u64) {
        // Try to claim the roll. Whichever writer wins zeroes the counters.
        let cur = self.epoch.load(Ordering::Acquire);
        if cur == new_epoch {
            return;
        }
        // Zero first, then publish the new epoch. Readers that see stale
        // epoch will still gate the counts by epoch-freshness check.
        self.ok.store(0, Ordering::Relaxed);
        self.err.store(0, Ordering::Relaxed);
        self.max_lat_ns.store(0, Ordering::Relaxed);
        let _ = self
            .epoch
            .compare_exchange(cur, new_epoch, Ordering::AcqRel, Ordering::Acquire);
        // Losers drop through; they will still see a rolled bucket.
    }
}

/// Fixed-size sliding window counter. `new(window_ns)` sizes buckets.
#[derive(Debug)]
pub struct SlidingCounter {
    buckets: [Bucket; N_BUCKETS],
    bucket_ns: u64,
}

impl SlidingCounter {
    /// `window_ns` — total window span in nanoseconds (>= N_BUCKETS ns).
    pub fn new(window_ns: u64) -> Self {
        let bucket_ns = (window_ns / N_BUCKETS as u64).max(1);
        Self {
            buckets: [
                Bucket::new(), Bucket::new(), Bucket::new(), Bucket::new(),
                Bucket::new(), Bucket::new(), Bucket::new(), Bucket::new(),
                Bucket::new(), Bucket::new(), Bucket::new(), Bucket::new(),
                Bucket::new(), Bucket::new(), Bucket::new(), Bucket::new(),
            ],
            bucket_ns,
        }
    }

    #[inline]
    fn epoch(&self, now_ns: u64) -> u64 {
        now_ns / self.bucket_ns
    }

    /// Record one outcome. No allocation. Callable from the hot path.
    #[inline]
    pub fn record(&self, ok: bool, latency_ns: u64, now_ns: u64) {
        let ep = self.epoch(now_ns);
        let idx = (ep as usize) % N_BUCKETS;
        let b = &self.buckets[idx];
        if b.epoch.load(Ordering::Acquire) != ep {
            b.roll_to(ep);
        }
        if ok {
            b.ok.fetch_add(1, Ordering::Relaxed);
        } else {
            b.err.fetch_add(1, Ordering::Relaxed);
        }
        // Monotonic max via CAS loop. No allocation.
        let mut prev = b.max_lat_ns.load(Ordering::Relaxed);
        while latency_ns > prev {
            match b.max_lat_ns.compare_exchange_weak(
                prev,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(cur) => prev = cur,
            }
        }
    }

    /// Snapshot totals over the window ending at `now_ns`.
    /// Returns (ok_count, err_count, max_latency_ns).
    #[inline]
    pub fn snapshot(&self, now_ns: u64) -> (u64, u64, u64) {
        let current = self.epoch(now_ns);
        // Oldest valid epoch is `current - (N_BUCKETS - 1)`, clamped.
        let min_valid = current.saturating_sub(N_BUCKETS as u64 - 1);
        let mut ok: u64 = 0;
        let mut err: u64 = 0;
        let mut max_lat: u64 = 0;
        for b in &self.buckets {
            let ep = b.epoch.load(Ordering::Acquire);
            if ep >= min_valid && ep <= current {
                ok += b.ok.load(Ordering::Relaxed) as u64;
                err += b.err.load(Ordering::Relaxed) as u64;
                let m = b.max_lat_ns.load(Ordering::Relaxed);
                if m > max_lat {
                    max_lat = m;
                }
            }
        }
        (ok, err, max_lat)
    }

    /// Total (ok+err) samples currently in the window.
    #[inline]
    pub fn total(&self, now_ns: u64) -> u64 {
        let (ok, err, _) = self.snapshot(now_ns);
        ok + err
    }

    /// Integer percentage err / (ok+err). Returns 0 when total == 0.
    #[inline]
    pub fn error_rate_pct(&self, now_ns: u64) -> u16 {
        let (ok, err, _) = self.snapshot(now_ns);
        let total = ok + err;
        if total == 0 {
            return 0;
        }
        ((err.saturating_mul(100)) / total) as u16
    }

    /// Reset all buckets. Not hot-path; used on HalfOpen→Closed transition.
    pub fn reset(&self) {
        for b in &self.buckets {
            b.epoch.store(u64::MAX, Ordering::Release);
            b.ok.store(0, Ordering::Relaxed);
            b.err.store(0, Ordering::Relaxed);
            b.max_lat_ns.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_bucket_accumulates() {
        let c = SlidingCounter::new(160_000_000); // 160 ms window, 10 ms buckets
        c.record(true, 100, 0);
        c.record(false, 200, 1_000);
        c.record(true, 300, 2_000);
        let (ok, err, mx) = c.snapshot(2_000);
        assert_eq!(ok, 2);
        assert_eq!(err, 1);
        assert_eq!(mx, 300);
    }

    #[test]
    fn rolls_on_bucket_advance() {
        let c = SlidingCounter::new(16_000_000); // 16 ms window, 1 ms buckets
        c.record(false, 0, 0);
        // Advance > N_BUCKETS*bucket_ns so previous bucket falls out.
        let later = 1_000_000_000; // 1 s later
        c.record(true, 0, later);
        let (ok, err, _) = c.snapshot(later);
        assert_eq!(ok, 1);
        assert_eq!(err, 0);
    }

    #[test]
    fn error_rate_pct_integer() {
        let c = SlidingCounter::new(16_000_000);
        for _ in 0..1 {
            c.record(true, 0, 0);
        }
        for _ in 0..3 {
            c.record(false, 0, 0);
        }
        assert_eq!(c.error_rate_pct(0), 75);
    }
}
