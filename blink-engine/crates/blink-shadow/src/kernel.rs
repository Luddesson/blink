//! [`DecisionKernel`] trait and the minimal [`KernelState`] model the
//! MVP advances per event.
//!
//! The rubber-duck review rejected the "frozen positions" model: the
//! kernel's decision depends on mutable state (inventory, cooldown
//! timers) that evolves as events are processed. The MVP carries that
//! state explicitly inside each [`crate::input::DecisionInput`] so
//! replay is reproducible, and updates it out-of-band (the runner does
//! not mutate it — that's the legacy-integration follow-up
//! `p0-shadow-hook`).

use blink_timestamps::Timestamp;
use blink_types::{AbortReason, DecisionOutcome};
use serde::{Deserialize, Serialize};

use crate::input::DecisionInput;

/// A minimal representation of engine-side position state.
///
/// Signed USDC µunits so long and short positions share a field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Net inventory in signed USDC µunits.
    pub net_size_i: i64,
}

/// Mutable state the kernel consults at decision time.
///
/// Deliberately tiny for the MVP — richer state (per-market cooldowns,
/// per-token open-order counts, risk budgets) lands with the real
/// legacy extraction in `p0-shadow-hook`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelState {
    /// Net position snapshot at decision time.
    pub position: Position,
    /// Cooldown gate expiry, if active.
    pub cooldown_until: Option<Timestamp>,
}

/// A decision kernel under parity test.
///
/// Implementations consume a fully-deterministic [`DecisionInput`] and
/// return a [`DecisionOutcome`]. The contract is strictly pure from the
/// harness's point of view: any observable side effect (logging, metric
/// emission) is the impl's business but MUST NOT influence the outcome.
pub trait DecisionKernel: Send {
    /// Human-readable name for reports.
    fn name(&self) -> &'static str;

    /// Stable impl identifier. The [`crate::runner::ShadowRunner`]
    /// refuses to start if `legacy.impl_id() == v2.impl_id()` — a
    /// configuration mistake the rubber-duck review flagged as silently
    /// producing green "parity" runs.
    fn impl_id(&self) -> u64;

    /// Produce the outcome for one input.
    fn decide(&mut self, input: &DecisionInput) -> DecisionOutcome;
}

/// Trivial kernel that always returns `NoOp { reason }`, unless
/// `divergence_mode` is set, in which case it returns an `Aborted` on
/// a specific event key. Used by the self-test to prove the harness
/// trips on a real divergence.
#[derive(Debug, Clone)]
pub struct StubKernel {
    /// Stable impl id.
    pub id: u64,
    /// Display name.
    pub name_str: &'static str,
    /// Reason string to use for NoOp outcomes.
    pub noop_reason: String,
    /// If true, on events whose `event_key == diverge_on` this kernel
    /// returns an Aborted(Drift) instead of the normal NoOp.
    pub divergence_mode: bool,
    /// Which event_key triggers the divergence.
    pub diverge_on: String,
}

impl StubKernel {
    /// Construct a stub kernel that always NoOps.
    pub fn noop(id: u64, name_str: &'static str, reason: impl Into<String>) -> Self {
        Self {
            id,
            name_str,
            noop_reason: reason.into(),
            divergence_mode: false,
            diverge_on: String::new(),
        }
    }

    /// Construct a stub kernel that diverges on one specific event key.
    pub fn diverging(
        id: u64,
        name_str: &'static str,
        reason: impl Into<String>,
        diverge_on: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name_str,
            noop_reason: reason.into(),
            divergence_mode: true,
            diverge_on: diverge_on.into(),
        }
    }
}

impl DecisionKernel for StubKernel {
    fn name(&self) -> &'static str {
        self.name_str
    }

    fn impl_id(&self) -> u64 {
        self.id
    }

    fn decide(&mut self, input: &DecisionInput) -> DecisionOutcome {
        if self.divergence_mode && input.event_key == self.diverge_on {
            DecisionOutcome::Aborted {
                reason: AbortReason::Drift,
                metric: None,
            }
        } else {
            DecisionOutcome::NoOp {
                reason: self.noop_reason.clone(),
            }
        }
    }
}
