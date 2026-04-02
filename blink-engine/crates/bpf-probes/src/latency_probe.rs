//! Application-level latency probes for the Blink HFT engine.
//!
//! Unlike the kernel-level eBPF telemetry (which measures TCP RTT, scheduler
//! jitter, and syscall overhead), these probes track **application-level**
//! timestamps for the hot-path event lifecycle:
//!
//! 1. WebSocket message received
//! 2. Order sent to CLOB REST API
//! 3. Fill confirmation received
//!
//! # Alert mechanism
//!
//! Any recorded latency exceeding **500 µs** triggers a `warn!` log.
//! This threshold is intentionally aggressive for an HFT engine.
//!
//! # Implementations
//!
//! | Type | Purpose |
//! |------|---------|
//! | [`NullProbe`] | No-op for local dev / Windows / when probes are disabled |
//! | [`LoggingProbe`] | Logs every measurement via `tracing` and alerts on >500 µs |
//! | *`EbpfProbe`* | TODO: Linux production probe backed by user-space eBPF maps |

use tracing::warn;

// ─── Alert threshold ─────────────────────────────────────────────────────────

/// Any latency above this threshold (in microseconds) triggers a warning.
pub const LATENCY_ALERT_THRESHOLD_US: u64 = 500;

// ─── LatencyProbe trait ──────────────────────────────────────────────────────

/// Application-level latency probe for the Blink HFT hot path.
///
/// Implementations must be `Send + Sync` for use across async tasks.
pub trait LatencyProbe: Send + Sync {
    /// Record the timestamp (in microseconds since an arbitrary epoch) when a
    /// WebSocket message is received from the Polymarket feed.
    fn record_ws_receive(&self, ts_us: u64);

    /// Record the timestamp when an order is dispatched to the CLOB REST API.
    fn record_order_sent(&self, ts_us: u64);

    /// Record the timestamp when a fill confirmation is received.
    fn record_fill_received(&self, ts_us: u64);
}

// ─── NullProbe (no-op) ──────────────────────────────────────────────────────

/// No-op latency probe for local development and Windows builds.
///
/// All methods are inlined away to zero overhead.
pub struct NullProbe;

impl LatencyProbe for NullProbe {
    #[inline(always)]
    fn record_ws_receive(&self, _ts_us: u64) {}

    #[inline(always)]
    fn record_order_sent(&self, _ts_us: u64) {}

    #[inline(always)]
    fn record_fill_received(&self, _ts_us: u64) {}
}

// ─── LoggingProbe ────────────────────────────────────────────────────────────

/// Latency probe that logs measurements and alerts on threshold violations.
///
/// Tracks the last WebSocket-receive timestamp so it can compute the
/// ws→order and ws→fill deltas and warn if they exceed 500 µs.
pub struct LoggingProbe {
    last_ws_receive: std::sync::atomic::AtomicU64,
}

impl LoggingProbe {
    pub fn new() -> Self {
        Self {
            last_ws_receive: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl Default for LoggingProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyProbe for LoggingProbe {
    fn record_ws_receive(&self, ts_us: u64) {
        self.last_ws_receive
            .store(ts_us, std::sync::atomic::Ordering::Relaxed);
        tracing::trace!(ts_us, "ws_receive recorded");
    }

    fn record_order_sent(&self, ts_us: u64) {
        let ws_ts = self
            .last_ws_receive
            .load(std::sync::atomic::Ordering::Relaxed);
        if ws_ts > 0 {
            let delta = ts_us.saturating_sub(ws_ts);
            tracing::debug!(delta_us = delta, "ws→order latency");
            if delta > LATENCY_ALERT_THRESHOLD_US {
                warn!(
                    delta_us = delta,
                    threshold_us = LATENCY_ALERT_THRESHOLD_US,
                    "⚠️  LATENCY ALERT: ws→order exceeds threshold"
                );
            }
        }
    }

    fn record_fill_received(&self, ts_us: u64) {
        let ws_ts = self
            .last_ws_receive
            .load(std::sync::atomic::Ordering::Relaxed);
        if ws_ts > 0 {
            let delta = ts_us.saturating_sub(ws_ts);
            tracing::debug!(delta_us = delta, "ws→fill latency");
            if delta > LATENCY_ALERT_THRESHOLD_US {
                warn!(
                    delta_us = delta,
                    threshold_us = LATENCY_ALERT_THRESHOLD_US,
                    "⚠️  LATENCY ALERT: ws→fill exceeds threshold"
                );
            }
        }
    }
}

// ─── EbpfProbe (stub) ────────────────────────────────────────────────────────

/// Stub for a production eBPF-backed latency probe.
///
/// On the production Linux server this will write timestamps directly into
/// eBPF maps that kernel-side probes can correlate with network-stack events.
///
/// TODO(Phase 5 — Linux production):
/// - Open shared BPF maps (`/sys/fs/bpf/blink_latency_map`)
/// - Write `(event_type, timestamp_us)` tuples via `libbpf` user-space API
/// - Kernel-side BPF programs compute the full NIC→app→CLOB→fill pipeline
pub struct EbpfProbe;

impl EbpfProbe {
    /// Attempt to open BPF maps and connect to the kernel probes.
    ///
    /// # Current behaviour
    /// Always returns `Ok(Self)` — the probe is a no-op stub.
    ///
    /// # Future behaviour (Linux production)
    /// Will open pinned BPF maps and return `Err` if the kernel probes
    /// are not loaded.
    #[allow(clippy::unnecessary_wraps)]
    pub fn try_attach() -> anyhow::Result<Self> {
        // TODO: open /sys/fs/bpf/blink_latency_map
        // TODO: verify kernel probes are attached
        warn!("EbpfProbe: stub — kernel map integration not yet implemented");
        Ok(Self)
    }
}

impl LatencyProbe for EbpfProbe {
    fn record_ws_receive(&self, ts_us: u64) {
        // TODO: write (WS_RECEIVE, ts_us) to BPF map key 0
        let _ = ts_us;
    }

    fn record_order_sent(&self, ts_us: u64) {
        // TODO: write (ORDER_SENT, ts_us) to BPF map key 1
        let _ = ts_us;
    }

    fn record_fill_received(&self, ts_us: u64) {
        // TODO: write (FILL_RECEIVED, ts_us) to BPF map key 2
        let _ = ts_us;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_probe_is_noop() {
        let probe = NullProbe;
        probe.record_ws_receive(1000);
        probe.record_order_sent(1200);
        probe.record_fill_received(1500);
        // No panic, no side effects.
    }

    #[test]
    fn logging_probe_tracks_deltas() {
        let probe = LoggingProbe::new();
        probe.record_ws_receive(1_000_000);
        // Within threshold — no alert.
        probe.record_order_sent(1_000_100);
        // Exceeds threshold — would emit warn! log.
        probe.record_fill_received(1_001_000);
    }

    #[test]
    fn ebpf_probe_stub_attach() {
        let probe = EbpfProbe::try_attach().unwrap();
        probe.record_ws_receive(100);
        probe.record_order_sent(200);
        probe.record_fill_received(300);
    }

    #[test]
    fn logging_probe_handles_zero_baseline() {
        let probe = LoggingProbe::new();
        // No ws_receive recorded yet — order_sent should not panic.
        probe.record_order_sent(500);
        probe.record_fill_received(600);
    }
}
