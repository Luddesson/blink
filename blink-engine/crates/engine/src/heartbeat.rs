//! Polymarket session heartbeat worker.
//!
//! Polymarket's CLOB requires a periodic `POST /heartbeat` to keep L2
//! credentials active.  A stale heartbeat window can cause the exchange to
//! cancel all open orders for the session.
//!
//! # Usage
//!
//! ```no_run
//! use engine::heartbeat::spawn_heartbeat_worker;
//! use engine::order_executor::OrderExecutor;
//! use engine::config::Config;
//!
//! let config = Config::from_env().unwrap();
//! let executor = OrderExecutor::from_config(&config);
//! // Spawn once at engine startup, before submitting any orders.
//! spawn_heartbeat_worker(executor, None);
//! ```
//!
//! The worker runs until the process exits.  Failures are logged as `warn`
//! (non-fatal) — the session may degrade but the engine continues running.
//! Operators should monitor `heartbeat_failures` in the SLO metrics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use crate::order_executor::OrderExecutor;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Default heartbeat interval — 8 s gives safe margin below 29 s expiry.
const DEFAULT_INTERVAL_SECS: u64 = 8;
/// Minimum allowed interval — prevents accidental hammering.
const MIN_INTERVAL_SECS:     u64 = 5;
/// Maximum allowed interval — beyond this the session risks expiry.
const MAX_INTERVAL_SECS:     u64 = 29;
/// Consecutive failures before tripping the circuit breaker.
const CONSECUTIVE_FAIL_THRESHOLD: u64 = 3;

// ─── SLO counters ─────────────────────────────────────────────────────────────

/// Shared counters updated by the heartbeat worker and readable by the SLO
/// snapshot / TUI dashboard.
#[derive(Debug, Default)]
pub struct HeartbeatMetrics {
    /// Total heartbeats sent successfully.
    pub ok_count:      AtomicU64,
    /// Total heartbeats that received a non-2xx or network error response.
    pub fail_count:    AtomicU64,
    /// Consecutive failures since the last success (resets on OK).
    pub consecutive_fails: AtomicU64,
    /// Unix-ms timestamp of the last successful heartbeat (0 = never).
    pub last_ok_ms:    AtomicU64,
}

impl HeartbeatMetrics {
    pub fn snapshot(&self) -> HeartbeatSnapshot {
        HeartbeatSnapshot {
            ok_count:          self.ok_count.load(Ordering::Relaxed),
            fail_count:        self.fail_count.load(Ordering::Relaxed),
            consecutive_fails: self.consecutive_fails.load(Ordering::Relaxed),
            last_ok_ms:        self.last_ok_ms.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time snapshot of heartbeat health for dashboards / alerts.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeartbeatSnapshot {
    pub ok_count:          u64,
    pub fail_count:        u64,
    pub consecutive_fails: u64,
    /// Last successful heartbeat timestamp in Unix milliseconds.
    pub last_ok_ms: u64,
}

impl HeartbeatSnapshot {
    /// Returns `true` if no heartbeat has succeeded in the last `threshold_ms`
    /// milliseconds — a signal that the session may be stale.
    pub fn is_stale(&self, threshold_ms: u64) -> bool {
        if self.last_ok_ms == 0 {
            return true;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now_ms.saturating_sub(self.last_ok_ms) > threshold_ms
    }
}

// ─── Worker ───────────────────────────────────────────────────────────────────

/// Spawns a background Tokio task that sends `POST /heartbeat` to Polymarket
/// on a fixed cadence.
///
/// # Arguments
/// * `executor` — An `OrderExecutor` configured with live credentials.
/// * `metrics`  — Optional shared metrics handle; if `None` a new one is
///   allocated internally (useful when callers don't need to read counters).
/// * `risk` — Optional shared risk manager for tripping circuit breaker on
///   consecutive heartbeat failures.
///
/// Returns the `Arc<HeartbeatMetrics>` handle so callers can read SLO state.
pub fn spawn_heartbeat_worker(
    executor: OrderExecutor,
    metrics:  Option<Arc<HeartbeatMetrics>>,
    risk:     Option<Arc<std::sync::Mutex<crate::risk_manager::RiskManager>>>,
) -> Arc<HeartbeatMetrics> {
    let metrics = metrics.unwrap_or_default();
    let m       = Arc::clone(&metrics);

    let interval_secs = std::env::var("HEARTBEAT_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS)
        .clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS);

    info!(interval_secs, "💓 Heartbeat worker started");

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        ticker.tick().await; // skip the first immediate tick

        loop {
            ticker.tick().await;
            match executor.send_heartbeat().await {
                Ok(()) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    m.ok_count.fetch_add(1, Ordering::Relaxed);
                    m.consecutive_fails.store(0, Ordering::Relaxed);
                    m.last_ok_ms.store(now_ms, Ordering::Relaxed);
                    info!("💓 Heartbeat OK");
                }
                Err(e) => {
                    m.fail_count.fetch_add(1, Ordering::Relaxed);
                    let consec = m.consecutive_fails.fetch_add(1, Ordering::Relaxed) + 1;
                    warn!(error = %e, consecutive_fails = consec, "💔 Heartbeat failed — session may degrade");

                    if consec >= CONSECUTIVE_FAIL_THRESHOLD {
                        if let Some(ref risk) = risk {
                            error!(
                                consecutive_fails = consec,
                                threshold = CONSECUTIVE_FAIL_THRESHOLD,
                                "🚨 Heartbeat dead — tripping circuit breaker to protect open orders"
                            );
                            risk.lock().unwrap().trip_circuit_breaker(
                                &format!("heartbeat_dead_{}consecutive_failures", consec),
                            );
                        }
                    }
                }
            }
        }
    });

    metrics
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_stale_when_never_sent() {
        let snap = HeartbeatSnapshot { ok_count: 0, fail_count: 0, last_ok_ms: 0, consecutive_fails: 0 };
        assert!(snap.is_stale(30_000));
    }

    #[test]
    fn snapshot_not_stale_when_recent() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let snap = HeartbeatSnapshot { ok_count: 1, fail_count: 0, last_ok_ms: now_ms, consecutive_fails: 0 };
        assert!(!snap.is_stale(30_000));
    }

    #[test]
    fn snapshot_stale_when_old() {
        // last_ok_ms set to 1 minute ago
        let old_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
            .saturating_sub(60_001);
        let snap = HeartbeatSnapshot { ok_count: 5, fail_count: 0, last_ok_ms: old_ms, consecutive_fails: 0 };
        assert!(snap.is_stale(60_000));
    }
}
