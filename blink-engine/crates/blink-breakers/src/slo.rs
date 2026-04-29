//! Minimal SLO alerting helpers. Not a Prometheus exporter — just
//! numeric accessors around a [`SlidingCounter`] that the caller owns.
//!
//! Wire these into whatever metrics system the deployment uses. The
//! submitter typically feeds `(ok, latency_ns, now_ns)` samples in, and
//! an ops loop polls `submit_p99_ns` / `error_rate_pct` on a timer.

use crate::sliding::SlidingCounter;

/// Holds a sliding counter used for SLO math. `window_ms` pinned at
/// construction; `p99` is approximated with max-in-window (see
/// [`crate::BreakerTrip::LatencyP99`] for the caveat).
#[derive(Debug)]
pub struct SloAlerts {
    window: SlidingCounter,
    window_ms: u32,
}

impl SloAlerts {
    pub fn new(window_ms: u32) -> Self {
        Self {
            window: SlidingCounter::new((window_ms as u64) * 1_000_000),
            window_ms,
        }
    }

    /// Record a submit latency sample.
    #[inline]
    pub fn record_submit(&self, ok: bool, latency_ns: u64, now_ns: u64) {
        self.window.record(ok, latency_ns, now_ns);
    }

    /// Max-in-window latency over the last `window_ms` (approximates p99).
    /// `None` if no samples. `window_ms` arg is informational only —
    /// internal window size is fixed at construction.
    pub fn submit_p99_ns(&self, _window_ms: u32, now_ns: u64) -> Option<u64> {
        let (ok, err, mx) = self.window.snapshot(now_ns);
        if ok + err == 0 {
            None
        } else {
            Some(mx)
        }
    }

    /// Integer error rate percentage over the window.
    pub fn error_rate_pct(&self, _window_ms: u32, now_ns: u64) -> u16 {
        self.window.error_rate_pct(now_ns)
    }

    /// Informational: window size in ms.
    pub fn window_ms(&self) -> u32 {
        self.window_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_none() {
        let s = SloAlerts::new(100);
        assert!(s.submit_p99_ns(100, 0).is_none());
        assert_eq!(s.error_rate_pct(100, 0), 0);
    }

    #[test]
    fn computes_error_rate_and_max() {
        let s = SloAlerts::new(160);
        s.record_submit(true, 50_000, 0);
        s.record_submit(false, 250_000, 1_000);
        s.record_submit(true, 10_000, 2_000);
        assert_eq!(s.submit_p99_ns(160, 2_000), Some(250_000));
        // 1/3 errors → 33%.
        assert_eq!(s.error_rate_pct(160, 2_000), 33);
    }
}
