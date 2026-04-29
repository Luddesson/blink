//! Per-dimension circuit breaker.
//!
//! Holds a lock-free state machine (Closed → Open → HalfOpen → …) using
//! `arc_swap::ArcSwap<BreakerState>` and a [`SlidingCounter`] of recent
//! outcomes. `admit` is the top of the submitter hot path — one atomic
//! load plus a match.

use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};

use crate::sliding::SlidingCounter;

/// State of one breaker. Small, `Copy`; wrapped in `Arc` for swapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal: admits all traffic.
    Closed,
    /// Tripped: rejects until `until_ns`.
    Open { until_ns: u64, reason: BreakerTrip },
    /// Cool-off elapsed: admits exactly one probe request.
    HalfOpen { probe_allowed: bool },
}

/// Why a breaker tripped. Carried inside [`BreakerState::Open`] and
/// returned by [`Admission::Reject`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerTrip {
    /// Error-rate over the rolling window exceeded the configured cap.
    ErrorRate {
        window_ms: u32,
        threshold_pct: u16,
        observed_pct: u16,
    },
    /// Max-observed latency over the rolling window exceeded the cap.
    /// (We call this "p99" for API parity with ops tooling; internally
    /// we track max-in-window because p99 requires a histogram the
    /// caller — SloAlerts — owns.)
    LatencyP99 {
        window_ms: u32,
        threshold_ns: u64,
        observed_ns: u64,
    },
    /// Manual kill — operator flipped a switch.
    KillSwitch { operator: &'static str },
    /// N consecutive StaleBook aborts from the decision kernel.
    StaleBookStreak { count: u32 },
    /// Daily loss cap reached.
    LossCap { pnl_u_usdc: i64 },
    /// N consecutive HTTP 429 rate-limit responses.
    RateLimit429 { streak: u32 },
}

/// Configuration for a single [`Breaker`].
#[derive(Debug, Clone, Copy)]
pub struct BreakerConfig {
    /// Trip if error_rate_pct over the window > this.
    pub error_rate_pct_threshold: u16,
    /// Rolling window for error rate.
    pub error_rate_window_ms: u32,
    /// Trip if max latency in window > this. 0 disables.
    pub latency_p99_ns_threshold: u64,
    /// Rolling window for latency stats.
    pub latency_window_ms: u32,
    /// How long Open lasts before becoming HalfOpen.
    pub cool_off_ms: u32,
    /// (Reserved for future multi-probe policies; single probe today.)
    pub half_open_probe_every_ms: u32,
    /// Don't evaluate thresholds until at least this many samples.
    pub min_samples: u32,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            error_rate_pct_threshold: 50,
            error_rate_window_ms: 1_000,
            latency_p99_ns_threshold: 0,
            latency_window_ms: 1_000,
            cool_off_ms: 1_000,
            half_open_probe_every_ms: 100,
            min_samples: 20,
        }
    }
}

/// Admission verdict for one `admit` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Admit the caller. Submitter may proceed.
    Ok,
    /// Reject. The carried [`BreakerTrip`] is the reason from the current
    /// Open state (or a just-computed rejection for HalfOpen).
    Reject(BreakerTrip),
}

/// Observed submit outcome, fed back into the breaker.
#[derive(Debug, Clone, Copy)]
pub struct Outcome {
    pub ok: bool,
    pub latency_ns: u64,
}

/// Read-only stats for Prometheus / ops.
#[derive(Debug, Default)]
pub struct BreakerStats {
    pub admits: AtomicU64,
    pub rejects: AtomicU64,
    pub trips: AtomicU64,
    pub probes_succeeded: AtomicU64,
    pub probes_failed: AtomicU64,
}

impl BreakerStats {
    pub fn snapshot(&self) -> BreakerStatsSnapshot {
        BreakerStatsSnapshot {
            admits: self.admits.load(Ordering::Relaxed),
            rejects: self.rejects.load(Ordering::Relaxed),
            trips: self.trips.load(Ordering::Relaxed),
            probes_succeeded: self.probes_succeeded.load(Ordering::Relaxed),
            probes_failed: self.probes_failed.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BreakerStatsSnapshot {
    pub admits: u64,
    pub rejects: u64,
    pub trips: u64,
    pub probes_succeeded: u64,
    pub probes_failed: u64,
}

/// Per-dimension circuit breaker.
#[derive(Debug)]
pub struct Breaker {
    cfg: BreakerConfig,
    /// Fast-path tag mirroring `state`. `0` means `Closed` — hot-path
    /// checks only this atomic and returns `Admission::Ok` without
    /// touching ArcSwap. Anything non-zero falls through to the slow
    /// path which reloads `state` and does the full match.
    fast_tag: core::sync::atomic::AtomicU8,
    state: ArcSwap<BreakerState>,
    window: SlidingCounter,
    stats: BreakerStats,
}

const TAG_CLOSED: u8 = 0;
const TAG_OPEN: u8 = 1;
const TAG_HALF_OPEN: u8 = 2;

impl Breaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        let win_ms = cfg
            .error_rate_window_ms
            .max(cfg.latency_window_ms)
            .max(1);
        let window = SlidingCounter::new((win_ms as u64) * 1_000_000);
        Self {
            cfg,
            fast_tag: core::sync::atomic::AtomicU8::new(TAG_CLOSED),
            state: ArcSwap::from_pointee(BreakerState::Closed),
            window,
            stats: BreakerStats::default(),
        }
    }

    /// Current config (by copy; cheap).
    pub fn config(&self) -> BreakerConfig {
        self.cfg
    }

    /// Current state. `Guard<Arc<_>>` is a lock-free read; no allocation.
    #[inline]
    pub fn state(&self) -> Guard<Arc<BreakerState>> {
        self.state.load()
    }

    /// Hot path: ask permission. Zero allocation on Closed / Open paths.
    /// HalfOpen→Open / Open→HalfOpen transitions allocate one Arc.
    #[inline]
    pub fn admit(&self, now_ns: u64) -> Admission {
        // Fast path: one relaxed load. Closed → Ok, no ArcSwap traffic.
        if self.fast_tag.load(Ordering::Acquire) == TAG_CLOSED {
            self.stats.admits.fetch_add(1, Ordering::Relaxed);
            return Admission::Ok;
        }
        self.admit_slow(now_ns)
    }

    #[cold]
    #[inline(never)]
    fn admit_slow(&self, now_ns: u64) -> Admission {
        let g = self.state.load();
        let s = **g;
        let v = match s {
            BreakerState::Closed => Admission::Ok,
            BreakerState::Open { until_ns, reason } => {
                if now_ns >= until_ns {
                    let claimed = Arc::new(BreakerState::HalfOpen { probe_allowed: false });
                    let prev = self.state.compare_and_swap(&g, claimed);
                    if Arc::ptr_eq(&prev, &g) {
                        self.fast_tag.store(TAG_HALF_OPEN, Ordering::Release);
                        Admission::Ok
                    } else {
                        Admission::Reject(reason)
                    }
                } else {
                    Admission::Reject(reason)
                }
            }
            BreakerState::HalfOpen { probe_allowed: true } => {
                let claimed = Arc::new(BreakerState::HalfOpen { probe_allowed: false });
                let prev = self.state.compare_and_swap(&g, claimed);
                if Arc::ptr_eq(&prev, &g) {
                    Admission::Ok
                } else {
                    Admission::Reject(open_placeholder())
                }
            }
            BreakerState::HalfOpen { probe_allowed: false } => {
                Admission::Reject(open_placeholder())
            }
        };
        match v {
            Admission::Ok => self.stats.admits.fetch_add(1, Ordering::Relaxed),
            Admission::Reject(_) => self.stats.rejects.fetch_add(1, Ordering::Relaxed),
        };
        v
    }

    /// Feed back a submit outcome. Drives Closed→Open, HalfOpen→Closed/Open.
    pub fn record_outcome(&self, outcome: Outcome, now_ns: u64) {
        self.window.record(outcome.ok, outcome.latency_ns, now_ns);

        let g = self.state.load();
        match **g {
            BreakerState::Closed => {
                let total = self.window.total(now_ns);
                if total < self.cfg.min_samples as u64 {
                    return;
                }
                let pct = self.window.error_rate_pct(now_ns);
                if pct > self.cfg.error_rate_pct_threshold {
                    self.trip(
                        BreakerTrip::ErrorRate {
                            window_ms: self.cfg.error_rate_window_ms,
                            threshold_pct: self.cfg.error_rate_pct_threshold,
                            observed_pct: pct,
                        },
                        now_ns,
                    );
                    return;
                }
                if self.cfg.latency_p99_ns_threshold > 0 {
                    let (_, _, mx) = self.window.snapshot(now_ns);
                    if mx > self.cfg.latency_p99_ns_threshold {
                        self.trip(
                            BreakerTrip::LatencyP99 {
                                window_ms: self.cfg.latency_window_ms,
                                threshold_ns: self.cfg.latency_p99_ns_threshold,
                                observed_ns: mx,
                            },
                            now_ns,
                        );
                    }
                }
            }
            BreakerState::HalfOpen { .. } => {
                if outcome.ok {
                    // Probe success → Closed. Reset window so stale pre-trip
                    // error counts don't immediately re-trip.
                    self.window.reset();
                    self.state.store(Arc::new(BreakerState::Closed));
                    self.fast_tag.store(TAG_CLOSED, Ordering::Release);
                    self.stats.probes_succeeded.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.stats.probes_failed.fetch_add(1, Ordering::Relaxed);
                    self.trip(
                        BreakerTrip::ErrorRate {
                            window_ms: self.cfg.error_rate_window_ms,
                            threshold_pct: self.cfg.error_rate_pct_threshold,
                            observed_pct: 100,
                        },
                        now_ns,
                    );
                }
            }
            BreakerState::Open { .. } => {
                // No-op: outcomes on Open mean a caller bypassed us or a
                // probe slipped through. Ignore.
            }
        }
    }

    /// Force-open the breaker (manual kill, external signal). Overwrites
    /// any existing Open with a fresh cool-off.
    pub fn trip(&self, reason: BreakerTrip, now_ns: u64) {
        let until_ns = now_ns.saturating_add((self.cfg.cool_off_ms as u64) * 1_000_000);
        self.state
            .store(Arc::new(BreakerState::Open { until_ns, reason }));
        self.fast_tag.store(TAG_OPEN, Ordering::Release);
        self.stats.trips.fetch_add(1, Ordering::Relaxed);
    }

    /// Force-close the breaker (ops override).
    pub fn close(&self) {
        self.window.reset();
        self.state.store(Arc::new(BreakerState::Closed));
        self.fast_tag.store(TAG_CLOSED, Ordering::Release);
    }

    /// Stats handle (for Prometheus, etc.).
    pub fn stats(&self) -> &BreakerStats {
        &self.stats
    }
}

/// Placeholder trip reason used when we reject a HalfOpen request for
/// losing the probe CAS. The originating Open reason has already been
/// consumed by the transition, so we report a synthetic ErrorRate zero.
#[inline]
fn open_placeholder() -> BreakerTrip {
    BreakerTrip::ErrorRate {
        window_ms: 0,
        threshold_pct: 0,
        observed_pct: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tight_cfg() -> BreakerConfig {
        BreakerConfig {
            error_rate_pct_threshold: 50,
            error_rate_window_ms: 160,
            latency_p99_ns_threshold: 0,
            latency_window_ms: 160,
            cool_off_ms: 100,
            half_open_probe_every_ms: 10,
            min_samples: 4,
        }
    }

    #[test]
    fn closed_to_open_on_error_rate() {
        let b = Breaker::new(tight_cfg());
        assert!(matches!(**b.state(), BreakerState::Closed));

        let now = 1_000_000_000u64;
        for i in 0..10 {
            b.record_outcome(
                Outcome {
                    ok: false,
                    latency_ns: 0,
                },
                now + i,
            );
        }
        match **b.state() {
            BreakerState::Open { reason: BreakerTrip::ErrorRate { .. }, .. } => {}
            s => panic!("expected Open(ErrorRate), got {:?}", s),
        }
    }

    #[test]
    fn open_to_half_open_after_cool_off() {
        let b = Breaker::new(tight_cfg());
        let now = 1_000_000_000u64;
        b.trip(BreakerTrip::KillSwitch { operator: "t" }, now);
        // Still Open right away.
        assert!(matches!(b.admit(now), Admission::Reject(_)));
        // After cool_off elapses, first admit claims the probe → Ok.
        let later = now + (tight_cfg().cool_off_ms as u64) * 1_000_000 + 1;
        assert!(matches!(b.admit(later), Admission::Ok));
        // Second concurrent admit at the same instant must be rejected.
        assert!(matches!(b.admit(later), Admission::Reject(_)));
    }

    #[test]
    fn half_open_probe_success_closes() {
        let b = Breaker::new(tight_cfg());
        let now = 1_000_000_000u64;
        b.trip(BreakerTrip::KillSwitch { operator: "t" }, now);
        let later = now + (tight_cfg().cool_off_ms as u64) * 1_000_000 + 1;
        assert!(matches!(b.admit(later), Admission::Ok));
        b.record_outcome(
            Outcome {
                ok: true,
                latency_ns: 0,
            },
            later,
        );
        assert!(matches!(**b.state(), BreakerState::Closed));
    }

    #[test]
    fn half_open_probe_failure_reopens() {
        let b = Breaker::new(tight_cfg());
        let now = 1_000_000_000u64;
        b.trip(BreakerTrip::KillSwitch { operator: "t" }, now);
        let later = now + (tight_cfg().cool_off_ms as u64) * 1_000_000 + 1;
        assert!(matches!(b.admit(later), Admission::Ok));
        b.record_outcome(
            Outcome {
                ok: false,
                latency_ns: 0,
            },
            later,
        );
        assert!(matches!(**b.state(), BreakerState::Open { .. }));
    }
}
