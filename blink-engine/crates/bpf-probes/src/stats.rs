//! Platform-independent telemetry statistics types.
//!
//! These types are always available regardless of platform or feature flags,
//! enabling TUI code to compile on all targets.

// ─── TCP RTT ──────────────────────────────────────────────────────────────────

/// TCP round-trip time statistics for Polymarket CLOB connections.
///
/// Measured at kernel level via `tracepoint/tcp/tcp_rcv_established`, filtered
/// to Polymarket CDN IP range (104.18.0.0/16).
#[derive(Debug, Clone, Default)]
pub struct RttStats {
    pub min_us: u64,
    pub max_us: u64,
    pub avg_us: u64,
    pub p99_us: u64,
    pub samples: u64,
}

// ─── Scheduler Latency ────────────────────────────────────────────────────────

/// Scheduler wakeup-to-run latency for the Blink engine process.
///
/// Measured via `tracepoint/sched/sched_switch` + `sched_wakeup`, filtered
/// to the engine PID. High values indicate CPU contention or priority issues.
#[derive(Debug, Clone, Default)]
pub struct SchedStats {
    pub min_us: u64,
    pub max_us: u64,
    pub avg_us: u64,
    pub p99_us: u64,
    /// Count of wakeup latencies exceeding 100µs threshold.
    pub threshold_violations: u64,
    pub samples: u64,
}

// ─── Syscall Profiling ────────────────────────────────────────────────────────

/// Latency histogram with µs-granularity buckets for syscall profiling.
///
/// Buckets: `[1, 2, 5, 10, 50, 100, 500, 1000]` µs.
/// Each bucket counts syscalls with latency ≥ bucket bound and < next bound.
#[derive(Debug, Clone, Default)]
pub struct SyscallHistogram {
    pub buckets: [u64; 8],
}

impl SyscallHistogram {
    /// Bucket boundaries in microseconds.
    pub const BUCKET_BOUNDS: [u64; 8] = [1, 2, 5, 10, 50, 100, 500, 1000];

    /// Record a single syscall latency sample into the appropriate bucket.
    pub fn record(&mut self, latency_us: u64) {
        for (i, &bound) in Self::BUCKET_BOUNDS.iter().enumerate().rev() {
            if latency_us >= bound {
                self.buckets[i] += 1;
                return;
            }
        }
        self.buckets[0] += 1;
    }

    /// Total number of recorded samples across all buckets.
    pub fn total(&self) -> u64 {
        self.buckets.iter().sum()
    }
}

/// Per-syscall latency statistics for `send()`, `recv()`, and `epoll_wait()`.
///
/// Measured via `raw_tracepoint/sys_enter` + `raw_tracepoint/sys_exit`,
/// tracking time spent inside each syscall.
#[derive(Debug, Clone, Default)]
pub struct SyscallStats {
    pub send_avg_us: u64,
    pub recv_avg_us: u64,
    pub epoll_avg_us: u64,
    pub histogram: SyscallHistogram,
    pub samples: u64,
}

// ─── Combined Snapshot ────────────────────────────────────────────────────────

/// Combined kernel telemetry snapshot for TUI display and monitoring.
///
/// When `available == false`, all stats are zeroed defaults and the TUI
/// should display "N/A" for kernel metrics.
#[derive(Debug, Clone, Default)]
pub struct KernelSnapshot {
    /// Whether eBPF telemetry is actively collecting data.
    pub available: bool,
    pub rtt: RttStats,
    pub sched: SchedStats,
    pub syscall: SyscallStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_record_distributes_correctly() {
        let mut h = SyscallHistogram::default();
        h.record(0); // < 1µs → bucket 0
        h.record(1); // ≥ 1µs → bucket 0
        h.record(3); // ≥ 2µs → bucket 1
        h.record(7); // ≥ 5µs → bucket 2
        h.record(50); // ≥ 50µs → bucket 4
        h.record(999); // ≥ 500µs → bucket 6
        h.record(1000); // ≥ 1000µs → bucket 7
        h.record(5000); // ≥ 1000µs → bucket 7

        assert_eq!(h.buckets[0], 2); // 0µs + 1µs
        assert_eq!(h.buckets[1], 1); // 3µs
        assert_eq!(h.buckets[2], 1); // 7µs
        assert_eq!(h.buckets[4], 1); // 50µs
        assert_eq!(h.buckets[6], 1); // 999µs
        assert_eq!(h.buckets[7], 2); // 1000µs + 5000µs
        assert_eq!(h.total(), 8);
    }

    #[test]
    fn kernel_snapshot_default_is_unavailable() {
        let snap = KernelSnapshot::default();
        assert!(!snap.available);
        assert_eq!(snap.rtt.samples, 0);
        assert_eq!(snap.sched.samples, 0);
        assert_eq!(snap.syscall.samples, 0);
    }
}
