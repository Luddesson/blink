//! [`BreakerSet`] — bundle of per-dimension breakers + a kill switch.

use core::sync::atomic::{AtomicU32, Ordering};

use blink_types::AbortReason;

use crate::breaker::{Admission, Breaker, BreakerConfig, BreakerStatsSnapshot, BreakerTrip, Outcome};
use crate::kill::KillSwitch;
use crate::loss_cap::{LossCapBreaker, LossCapConfig};

/// Config bundle wiring every breaker knob.
#[derive(Debug, Clone, Copy)]
pub struct BreakerSetConfig {
    pub submit: BreakerConfig,
    pub stale_book: BreakerConfig,
    pub rate_limit: BreakerConfig,
    pub loss_cap: LossCapConfig,
    /// Trip `stale_book` after this many consecutive `AbortReason::StaleBook`.
    pub stale_book_streak_threshold: u32,
    /// Trip `rate_limit` after this many consecutive HTTP 429s.
    pub rate_limit_429_streak_threshold: u32,
}

impl Default for BreakerSetConfig {
    fn default() -> Self {
        Self {
            submit: BreakerConfig::default(),
            stale_book: BreakerConfig {
                // Hair-trigger: flip on the first ErrorRate evaluation once
                // a streak trip fires via on_kernel_abort.
                error_rate_pct_threshold: 100,
                min_samples: u32::MAX,
                ..BreakerConfig::default()
            },
            rate_limit: BreakerConfig {
                error_rate_pct_threshold: 100,
                min_samples: u32::MAX,
                ..BreakerConfig::default()
            },
            loss_cap: LossCapConfig::default(),
            stale_book_streak_threshold: 5,
            rate_limit_429_streak_threshold: 3,
        }
    }
}

/// Bundle of breakers guarding the submitter.
#[derive(Debug)]
pub struct BreakerSet {
    pub submit: Breaker,
    pub stale_book: Breaker,
    pub rate_limit: Breaker,
    pub loss_cap: LossCapBreaker,
    pub kill: KillSwitch,
    cfg: BreakerSetConfig,
    stale_book_streak: AtomicU32,
    rate_limit_streak: AtomicU32,
}

impl BreakerSet {
    pub fn new(cfg: BreakerSetConfig) -> Self {
        Self {
            submit: Breaker::new(cfg.submit),
            stale_book: Breaker::new(cfg.stale_book),
            rate_limit: Breaker::new(cfg.rate_limit),
            loss_cap: LossCapBreaker::new(cfg.loss_cap),
            kill: KillSwitch::new(),
            cfg,
            stale_book_streak: AtomicU32::new(0),
            rate_limit_streak: AtomicU32::new(0),
        }
    }

    /// Config by copy.
    pub fn config(&self) -> BreakerSetConfig {
        self.cfg
    }

    /// Top-of-hot-path admission. Checks kill switch, then every breaker
    /// in priority order. First rejecter wins. Zero allocation when all
    /// breakers are Closed and the kill switch is disengaged.
    #[inline]
    pub fn admit_submit(&self, now_ns: u64) -> Admission {
        if self.kill.is_engaged() {
            return Admission::Reject(BreakerTrip::KillSwitch { operator: "kill-switch" });
        }
        match self.loss_cap.admit(now_ns) {
            Admission::Ok => {}
            r => return r,
        }
        match self.stale_book.admit(now_ns) {
            Admission::Ok => {}
            r => return r,
        }
        match self.rate_limit.admit(now_ns) {
            Admission::Ok => {}
            r => return r,
        }
        self.submit.admit(now_ns)
    }

    /// Feed back a submit outcome.
    pub fn on_submit_outcome(&self, accepted: bool, latency_ns: u64, now_ns: u64) {
        self.submit.record_outcome(
            Outcome {
                ok: accepted,
                latency_ns,
            },
            now_ns,
        );
    }

    /// Feed a kernel-side abort. Tracks consecutive StaleBook streaks.
    pub fn on_kernel_abort(&self, reason: AbortReason, now_ns: u64) {
        match reason {
            AbortReason::StaleBook => {
                let n = self.stale_book_streak.fetch_add(1, Ordering::AcqRel) + 1;
                if n >= self.cfg.stale_book_streak_threshold {
                    self.stale_book
                        .trip(BreakerTrip::StaleBookStreak { count: n }, now_ns);
                }
            }
            _ => {
                self.stale_book_streak.store(0, Ordering::Release);
            }
        }
    }

    /// Reset the stale-book streak — e.g. on a successful book tick.
    pub fn on_fresh_book(&self) {
        self.stale_book_streak.store(0, Ordering::Release);
    }

    /// Feed an HTTP 429 rate-limit response.
    pub fn on_rate_limit_429(&self, now_ns: u64) {
        let n = self.rate_limit_streak.fetch_add(1, Ordering::AcqRel) + 1;
        if n >= self.cfg.rate_limit_429_streak_threshold {
            self.rate_limit
                .trip(BreakerTrip::RateLimit429 { streak: n }, now_ns);
        }
    }

    /// Reset the 429 streak — e.g. on any non-429 response.
    pub fn on_rate_limit_ok(&self) {
        self.rate_limit_streak.store(0, Ordering::Release);
    }

    /// Feed a fill's PnL delta (micro-USDC) into the loss cap breaker.
    pub fn on_fill_pnl(&self, pnl_delta_u_usdc: i64, now_ns: u64) {
        self.loss_cap.on_fill_pnl(pnl_delta_u_usdc, now_ns);
    }

    /// Prometheus-friendly snapshot.
    pub fn snapshot(&self) -> BreakerSetSnapshot {
        BreakerSetSnapshot {
            submit: self.submit.stats().snapshot(),
            stale_book: self.stale_book.stats().snapshot(),
            rate_limit: self.rate_limit.stats().snapshot(),
            kill_engaged: self.kill.is_engaged(),
            kill_generation: self.kill.generation(),
            stale_book_streak: self.stale_book_streak.load(Ordering::Relaxed),
            rate_limit_streak: self.rate_limit_streak.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BreakerSetSnapshot {
    pub submit: BreakerStatsSnapshot,
    pub stale_book: BreakerStatsSnapshot,
    pub rate_limit: BreakerStatsSnapshot,
    pub kill_engaged: bool,
    pub kill_generation: u64,
    pub stale_book_streak: u32,
    pub rate_limit_streak: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breaker::BreakerState;

    #[test]
    fn kill_switch_rejects_immediately() {
        let s = BreakerSet::new(BreakerSetConfig::default());
        assert!(matches!(s.admit_submit(0), Admission::Ok));
        s.kill.engage("ops");
        assert!(matches!(
            s.admit_submit(0),
            Admission::Reject(BreakerTrip::KillSwitch { .. })
        ));
        s.kill.disengage();
        assert!(matches!(s.admit_submit(0), Admission::Ok));
    }

    #[test]
    fn stale_book_streak_trips_at_threshold() {
        let mut cfg = BreakerSetConfig::default();
        cfg.stale_book_streak_threshold = 5;
        let s = BreakerSet::new(cfg);
        let now = 1_000;
        for _ in 0..4 {
            s.on_kernel_abort(AbortReason::StaleBook, now);
        }
        assert!(matches!(**s.stale_book.state(), BreakerState::Closed));
        s.on_kernel_abort(AbortReason::StaleBook, now);
        assert!(matches!(
            **s.stale_book.state(),
            BreakerState::Open {
                reason: BreakerTrip::StaleBookStreak { .. },
                ..
            }
        ));
        assert!(matches!(
            s.admit_submit(now),
            Admission::Reject(BreakerTrip::StaleBookStreak { .. })
        ));
    }

    #[test]
    fn non_stale_book_abort_resets_streak() {
        let mut cfg = BreakerSetConfig::default();
        cfg.stale_book_streak_threshold = 3;
        let s = BreakerSet::new(cfg);
        s.on_kernel_abort(AbortReason::StaleBook, 0);
        s.on_kernel_abort(AbortReason::StaleBook, 0);
        s.on_kernel_abort(AbortReason::Drift, 0); // resets
        s.on_kernel_abort(AbortReason::StaleBook, 0);
        s.on_kernel_abort(AbortReason::StaleBook, 0);
        assert!(matches!(**s.stale_book.state(), BreakerState::Closed));
    }

    #[test]
    fn rate_limit_429_streak_trips() {
        let mut cfg = BreakerSetConfig::default();
        cfg.rate_limit_429_streak_threshold = 3;
        let s = BreakerSet::new(cfg);
        s.on_rate_limit_429(0);
        s.on_rate_limit_429(0);
        assert!(matches!(**s.rate_limit.state(), BreakerState::Closed));
        s.on_rate_limit_429(0);
        assert!(matches!(
            **s.rate_limit.state(),
            BreakerState::Open {
                reason: BreakerTrip::RateLimit429 { .. },
                ..
            }
        ));
    }
}
