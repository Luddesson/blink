//! Microsecond-level latency measurement for the BLINK hot path.
//!
//! Tracks signal age (time from RN1Signal creation to consumption) and
//! WebSocket message throughput.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ─── LatencyStats ─────────────────────────────────────────────────────────────

/// Rolling-window latency statistics (raw `Duration` samples).
pub struct LatencyStats {
    samples: Vec<Duration>,
    window: usize,
}

impl LatencyStats {
    pub fn new(window: usize) -> Self {
        Self {
            samples: Vec::with_capacity(window),
            window,
        }
    }

    /// Record a new latency sample, evicting the oldest if the window is full.
    pub fn record(&mut self, d: Duration) {
        if self.samples.len() >= self.window {
            self.samples.remove(0);
        }
        self.samples.push(d);
    }

    pub fn min_us(&self) -> Option<u64> {
        self.samples.iter().map(|d| d.as_micros() as u64).min()
    }

    pub fn max_us(&self) -> Option<u64> {
        self.samples.iter().map(|d| d.as_micros() as u64).max()
    }

    pub fn avg_us(&self) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: u128 = self.samples.iter().map(|d| d.as_micros()).sum();
        Some((sum / self.samples.len() as u128) as u64)
    }

    pub fn p99_us(&self) -> Option<u64> {
        self.percentile_us(0.99)
    }

    pub fn p95_us(&self) -> Option<u64> {
        self.percentile_us(0.95)
    }

    pub fn p50_us(&self) -> Option<u64> {
        self.percentile_us(0.50)
    }

    pub fn count(&self) -> usize {
        self.samples.len()
    }

    /// Returns the raw samples as microseconds for histogram rendering.
    pub fn samples_us(&self) -> Vec<u64> {
        self.samples.iter().map(|d| d.as_micros() as u64).collect()
    }

    fn percentile_us(&self, p: f64) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<u64> = self.samples.iter().map(|d| d.as_micros() as u64).collect();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * p) as usize).min(sorted.len() - 1);
        Some(sorted[idx])
    }
}

// ─── LatencyTracker ───────────────────────────────────────────────────────────

/// Bundle of all latency tracking state shared across threads.
pub struct LatencyTracker {
    /// Time from RN1Signal creation (`detected_at`) to consumption in the engine.
    pub signal_age: Arc<Mutex<LatencyStats>>,
    /// WebSocket message throughput counter.
    pub msgs_per_sec: Arc<Mutex<MsgCounter>>,
}

impl LatencyTracker {
    pub fn new(window: usize) -> Self {
        Self {
            signal_age: Arc::new(Mutex::new(LatencyStats::new(window))),
            msgs_per_sec: Arc::new(Mutex::new(MsgCounter::new())),
        }
    }
}

// ─── MsgCounter ───────────────────────────────────────────────────────────────

/// Sliding 1-second window message counter for WS throughput measurement.
pub struct MsgCounter {
    count_current: u64,
    count_last_sec: u64,
    window_start: Instant,
}

impl MsgCounter {
    pub fn new() -> Self {
        Self {
            count_current: 0,
            count_last_sec: 0,
            window_start: Instant::now(),
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
            self.count_last_sec = (self.count_current as f64 / elapsed.as_secs_f64()) as u64;
            self.count_current = 0;
            self.window_start = Instant::now();
        }
        self.count_last_sec
    }
}
