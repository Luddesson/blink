//! Microsecond-level latency measurement for the BLINK hot path.
//!
//! Tracks signal age (time from RN1Signal creation to consumption) and
//! WebSocket message throughput.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

// ─── LatencySummary ──────────────────────────────────────────────────────────

/// Snapshot of latency statistics, suitable for JSON serialization.
#[derive(Debug, Clone, Serialize)]
pub struct LatencySummary {
    pub count:   usize,
    pub min_us:  u64,
    pub max_us:  u64,
    pub avg_us:  u64,
    pub p50_us:  u64,
    pub p95_us:  u64,
    pub p99_us:  u64,
    pub p999_us: u64,
    /// Histogram buckets: [0-10µs, 10-50µs, 50-100µs, 100-500µs, 500-1000µs, 1000+µs]
    pub histogram: [u32; 6],
}

// ─── LatencyStats ─────────────────────────────────────────────────────────────

/// Rolling-window latency statistics (raw `Duration` samples).
///
/// Uses `VecDeque` for O(1) push/pop on both ends.
pub struct LatencyStats {
    samples:  VecDeque<Duration>,
    window:   usize,
    /// Incrementally-maintained histogram buckets (updated on every record).
    buckets:  [u32; 6],
    /// Rolling sum for fast avg_us without iterating samples.
    sum_us:   u128,
}

impl LatencyStats {
    pub fn new(window: usize) -> Self {
        Self {
            samples:  VecDeque::with_capacity(window),
            window,
            buckets:  [0u32; 6],
            sum_us:   0,
        }
    }

    /// Record a new latency sample, evicting the oldest if the window is full.
    /// O(1) amortised — no shifting of existing elements.
    pub fn record(&mut self, d: Duration) {
        let us = d.as_micros() as u64;
        // Evict oldest if window full
        if self.samples.len() >= self.window {
            let old = self.samples.pop_front().expect("non-empty");
            let old_us = old.as_micros() as u64;
            self.sum_us = self.sum_us.saturating_sub(old_us as u128);
            // Decrement old bucket
            let old_idx = Self::bucket_idx(old_us);
            self.buckets[old_idx] = self.buckets[old_idx].saturating_sub(1);
        }
        self.sum_us += us as u128;
        self.buckets[Self::bucket_idx(us)] += 1;
        self.samples.push_back(d);
    }

    #[inline]
    fn bucket_idx(us: u64) -> usize {
        match us {
            0..=9     => 0,
            10..=49   => 1,
            50..=99   => 2,
            100..=499 => 3,
            500..=999 => 4,
            _         => 5,
        }
    }

    pub fn count(&self) -> usize { self.samples.len() }

    /// Returns all samples as microseconds (for TUI histogram rendering).
    pub fn samples_us(&self) -> Vec<u64> {
        self.samples.iter().map(|d| d.as_micros() as u64).collect()
    }

    pub fn min_us(&self) -> Option<u64> {
        self.samples.iter().map(|d| d.as_micros() as u64).min()
    }

    pub fn max_us(&self) -> Option<u64> {
        self.samples.iter().map(|d| d.as_micros() as u64).max()
    }

    pub fn avg_us(&self) -> Option<u64> {
        if self.samples.is_empty() { return None; }
        Some((self.sum_us / self.samples.len() as u128) as u64)
    }

    pub fn p50_us(&self)  -> Option<u64> { self.percentile(0.50) }
    pub fn p95_us(&self)  -> Option<u64> { self.percentile(0.95) }
    pub fn p99_us(&self)  -> Option<u64> { self.percentile(0.99) }
    pub fn p999_us(&self) -> Option<u64> { self.percentile(0.999) }

    fn percentile(&self, p: f64) -> Option<u64> {
        if self.samples.is_empty() { return None; }
        let mut sorted: Vec<u64> = self.samples.iter().map(|d| d.as_micros() as u64).collect();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * p) as usize).min(sorted.len() - 1);
        Some(sorted[idx])
    }

    fn ensure_sorted(&self) {}

    /// Returns the incrementally-maintained histogram buckets — O(1).
    pub fn histogram_buckets(&self) -> [u32; 6] { self.buckets }

    /// Returns a latency summary snapshot.
    pub fn summary(&self) -> LatencySummary {
        LatencySummary {
            count:     self.count(),
            min_us:    self.min_us().unwrap_or(0),
            max_us:    self.max_us().unwrap_or(0),
            avg_us:    self.avg_us().unwrap_or(0),
            p50_us:    self.percentile(0.50).unwrap_or(0),
            p95_us:    self.percentile(0.95).unwrap_or(0),
            p99_us:    self.percentile(0.99).unwrap_or(0),
            p999_us:   self.percentile(0.999).unwrap_or(0),
            histogram: self.buckets,
        }
    }
}

// ─── LatencyTracker ───────────────────────────────────────────────────────────

/// Bundle of all latency tracking state shared across threads.
pub struct LatencyTracker {
    /// Time from RN1Signal creation (`detected_at`) to consumption in the engine.
    pub signal_age:   Arc<Mutex<LatencyStats>>,
    /// WebSocket message throughput counter.
    pub msgs_per_sec: Arc<Mutex<MsgCounter>>,
}

impl LatencyTracker {
    pub fn new(window: usize) -> Self {
        Self {
            signal_age:   Arc::new(Mutex::new(LatencyStats::new(window))),
            msgs_per_sec: Arc::new(Mutex::new(MsgCounter::new())),
        }
    }
}

// ─── MsgCounter ───────────────────────────────────────────────────────────────

/// Sliding 1-second window message counter for WS throughput measurement.
pub struct MsgCounter {
    count_current:  u64,
    count_last_sec: u64,
    window_start:   Instant,
}

impl MsgCounter {
    pub fn new() -> Self {
        Self {
            count_current:  0,
            count_last_sec: 0,
            window_start:   Instant::now(),
        }
    }

    /// Increment on every WS message received.
    pub fn tick(&mut self) {
        self.count_current += 1;
    }

    /// Returns msgs/s for the last completed window.  
    /// Resets the counter when ≥1 second has elapsed.
    pub fn per_second(&mut self) -> u64 {
        let elapsed = self.window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.count_last_sec =
                (self.count_current as f64 / elapsed.as_secs_f64()) as u64;
            self.count_current = 0;
            self.window_start  = Instant::now();
        }
        self.count_last_sec
    }
}
