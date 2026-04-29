//! # blink-breakers
//!
//! Circuit breakers, a process-global kill switch, and SLO-alert helpers
//! sitting between the decision kernel's verdicts and the submitter.
//!
//! Hot-path contract:
//!
//! 1. Submitter calls [`BreakerSet::admit_submit`] **before** invoking
//!    the submit path. The gate returns [`Admission::Ok`] or
//!    [`Admission::Reject`] with a [`BreakerTrip`] reason; the submitter
//!    turns the latter into [`blink_types::AbortReason::CircuitOpen`].
//! 2. After the attempt, the submitter calls
//!    [`BreakerSet::on_submit_outcome`] to feed the breaker.
//! 3. The kernel path calls [`BreakerSet::on_kernel_abort`] for every
//!    abort so streak-based breakers (stale-book) can track.
//! 4. The submitter calls [`BreakerSet::on_rate_limit_429`] on each 429
//!    response, and [`BreakerSet::on_fill_pnl`] on fills.
//!
//! Zero allocation on the hot path when all breakers are `Closed` and the
//! kill switch is disengaged (the common case). State transitions allocate
//! one `Arc<BreakerState>`; those happen off the steady-state path.

mod breaker;
mod kill;
mod loss_cap;
mod set;
mod sliding;
mod slo;

pub use breaker::{
    Admission, Breaker, BreakerConfig, BreakerState, BreakerStats, BreakerStatsSnapshot,
    BreakerTrip, Outcome,
};
pub use kill::KillSwitch;
pub use loss_cap::{LossCapBreaker, LossCapConfig};
pub use set::{BreakerSet, BreakerSetConfig, BreakerSetSnapshot};
pub use sliding::SlidingCounter;
pub use slo::SloAlerts;
