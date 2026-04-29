//! Daily loss cap breaker. 24 hourly buckets of `AtomicI64` deltas.
//!
//! The sum of the buckets whose epoch is within the trailing 24h window
//! approximates daily PnL. If `sum < -cap`, we trip. Disengage happens
//! naturally after the cool-off window when PnL rolls up.

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};

use crate::breaker::{Admission, BreakerState, BreakerTrip};

/// Number of hourly buckets spanning the loss-cap window.
const N_HOURS: usize = 24;
const HOUR_NS: u64 = 3_600 * 1_000_000_000;

#[derive(Debug)]
struct HourBucket {
    epoch: AtomicU64, // now_ns / HOUR_NS
    sum: AtomicI64,
}

impl HourBucket {
    const fn new() -> Self {
        Self {
            epoch: AtomicU64::new(u64::MAX),
            sum: AtomicI64::new(0),
        }
    }
}

/// Config for the daily loss cap.
#[derive(Debug, Clone, Copy)]
pub struct LossCapConfig {
    /// Cap magnitude in micro-USDC. If `rolling_24h_sum < -cap`, trip.
    pub cap_u_usdc: i64,
    /// Cool-off, identical semantics to [`crate::BreakerConfig::cool_off_ms`].
    pub cool_off_ms: u32,
}

impl Default for LossCapConfig {
    fn default() -> Self {
        Self {
            cap_u_usdc: 1_000_000_000, // $1000 in micro-USDC
            cool_off_ms: 60_000,
        }
    }
}

/// Loss-cap breaker.
#[derive(Debug)]
pub struct LossCapBreaker {
    cfg: LossCapConfig,
    buckets: [HourBucket; N_HOURS],
    state: ArcSwap<BreakerState>,
    /// Fast-path tag: 0 = Closed, else slow path (mirrors [`Breaker`]).
    fast_tag: core::sync::atomic::AtomicU8,
}

impl LossCapBreaker {
    pub fn new(cfg: LossCapConfig) -> Self {
        Self {
            cfg,
            buckets: [
                HourBucket::new(), HourBucket::new(), HourBucket::new(), HourBucket::new(),
                HourBucket::new(), HourBucket::new(), HourBucket::new(), HourBucket::new(),
                HourBucket::new(), HourBucket::new(), HourBucket::new(), HourBucket::new(),
                HourBucket::new(), HourBucket::new(), HourBucket::new(), HourBucket::new(),
                HourBucket::new(), HourBucket::new(), HourBucket::new(), HourBucket::new(),
                HourBucket::new(), HourBucket::new(), HourBucket::new(), HourBucket::new(),
            ],
            state: ArcSwap::from_pointee(BreakerState::Closed),
            fast_tag: core::sync::atomic::AtomicU8::new(0),
        }
    }

    pub fn config(&self) -> LossCapConfig {
        self.cfg
    }

    #[inline]
    pub fn state(&self) -> Guard<Arc<BreakerState>> {
        self.state.load()
    }

    /// Record a PnL delta (micro-USDC). Recomputes rolling sum; trips on breach.
    pub fn on_fill_pnl(&self, pnl_delta_u_usdc: i64, now_ns: u64) {
        let ep = now_ns / HOUR_NS;
        let idx = (ep as usize) % N_HOURS;
        let b = &self.buckets[idx];
        let cur = b.epoch.load(Ordering::Acquire);
        if cur != ep {
            // Roll the bucket: reset then publish epoch.
            b.sum.store(0, Ordering::Relaxed);
            let _ = b
                .epoch
                .compare_exchange(cur, ep, Ordering::AcqRel, Ordering::Acquire);
        }
        b.sum.fetch_add(pnl_delta_u_usdc, Ordering::Relaxed);

        let sum = self.rolling_sum(now_ns);
        if sum < -self.cfg.cap_u_usdc {
            self.trip(sum, now_ns);
        } else {
            // Recovery: if Open and cool-off elapsed, step to HalfOpen.
            self.maybe_step_from_open(now_ns);
        }
    }

    /// Sum the last 24 hourly buckets.
    #[inline]
    pub fn rolling_sum(&self, now_ns: u64) -> i64 {
        let current = now_ns / HOUR_NS;
        let min_valid = current.saturating_sub(N_HOURS as u64 - 1);
        let mut sum: i64 = 0;
        for b in &self.buckets {
            let ep = b.epoch.load(Ordering::Acquire);
            if ep >= min_valid && ep <= current {
                sum = sum.saturating_add(b.sum.load(Ordering::Relaxed));
            }
        }
        sum
    }

    /// Hot-path admit.
    #[inline]
    pub fn admit(&self, now_ns: u64) -> Admission {
        if self.fast_tag.load(Ordering::Acquire) == 0 {
            return Admission::Ok;
        }
        self.admit_slow(now_ns)
    }

    #[cold]
    #[inline(never)]
    fn admit_slow(&self, now_ns: u64) -> Admission {
        let g = self.state.load();
        match **g {
            BreakerState::Closed => Admission::Ok,
            BreakerState::Open { until_ns, reason } => {
                if now_ns >= until_ns {
                    let claimed = Arc::new(BreakerState::HalfOpen { probe_allowed: false });
                    let prev = self.state.compare_and_swap(&g, claimed);
                    if Arc::ptr_eq(&prev, &g) {
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
                    Admission::Reject(BreakerTrip::LossCap { pnl_u_usdc: 0 })
                }
            }
            BreakerState::HalfOpen { probe_allowed: false } => {
                Admission::Reject(BreakerTrip::LossCap { pnl_u_usdc: 0 })
            }
        }
    }

    fn trip(&self, observed_sum: i64, now_ns: u64) {
        let until_ns = now_ns.saturating_add((self.cfg.cool_off_ms as u64) * 1_000_000);
        self.state.store(Arc::new(BreakerState::Open {
            until_ns,
            reason: BreakerTrip::LossCap {
                pnl_u_usdc: observed_sum,
            },
        }));
        self.fast_tag.store(1, Ordering::Release);
    }

    fn maybe_step_from_open(&self, now_ns: u64) {
        let g = self.state.load();
        if let BreakerState::Open { until_ns, .. } = **g {
            if now_ns >= until_ns {
                let next = Arc::new(BreakerState::Closed);
                if Arc::ptr_eq(&self.state.compare_and_swap(&g, next), &g) {
                    self.fast_tag.store(0, Ordering::Release);
                }
            }
        }
    }

    /// Ops override.
    pub fn close(&self) {
        self.state.store(Arc::new(BreakerState::Closed));
        self.fast_tag.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_on_sum_below_neg_cap() {
        let b = LossCapBreaker::new(LossCapConfig {
            cap_u_usdc: 1_000,
            cool_off_ms: 10,
        });
        let now = 5 * HOUR_NS;
        b.on_fill_pnl(-500, now);
        assert!(matches!(**b.state(), BreakerState::Closed));
        b.on_fill_pnl(-600, now);
        match **b.state() {
            BreakerState::Open { reason: BreakerTrip::LossCap { pnl_u_usdc }, .. } => {
                assert!(pnl_u_usdc < -1_000);
            }
            s => panic!("expected Open(LossCap), got {:?}", s),
        }
    }

    #[test]
    fn recovery_reopens_after_cool_off() {
        let b = LossCapBreaker::new(LossCapConfig {
            cap_u_usdc: 1_000,
            cool_off_ms: 10,
        });
        let now = 5 * HOUR_NS;
        b.on_fill_pnl(-2_000, now);
        assert!(matches!(**b.state(), BreakerState::Open { .. }));

        let later = now + 20 * 1_000_000; // > cool_off
        // A positive PnL delta inside the same hour bucket pulls the sum
        // back above -cap; recovery step happens.
        b.on_fill_pnl(1_500, later);
        let s = **b.state();
        match s {
            BreakerState::Closed | BreakerState::HalfOpen { .. } => {}
            other => panic!("expected Closed/HalfOpen, got {:?}", other),
        }
    }
}
