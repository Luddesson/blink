//! [`ShadowRunner`] — drives two [`DecisionKernel`]s over an iterator
//! of [`DecisionInput`]s and records divergences.
//!
//! Single-threaded by design. The rubber-duck review specifically
//! cautioned against an async / multi-threaded harness until we have
//! bit-for-bit parity at all — spurious failures from reordering would
//! drown the signal.

use blink_types::DecisionOutcome;

use crate::divergence::{DivergenceField, DivergenceRecord};
use crate::fingerprint::{classify_noop, fingerprint, summarize};
use crate::input::DecisionInput;
use crate::journal::ShadowJournal;
use crate::kernel::DecisionKernel;

/// Running tallies emitted by [`ShadowRunner::report`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    /// Total events processed.
    pub events_total: u64,
    /// Events where the two fingerprints differed.
    pub divergences_total: u64,
    /// Submits emitted by the legacy kernel.
    pub submits_legacy: u64,
    /// Submits emitted by the v2 kernel.
    pub submits_v2: u64,
    /// Aborts emitted by the legacy kernel.
    pub aborts_legacy: u64,
    /// Aborts emitted by the v2 kernel.
    pub aborts_v2: u64,
    /// NoOps emitted by the legacy kernel.
    pub noops_legacy: u64,
    /// NoOps emitted by the v2 kernel.
    pub noops_v2: u64,
}

/// Snapshot returned from [`ShadowRunner::report`].
#[derive(Debug, Clone)]
pub struct ShadowReport {
    /// Counters.
    pub counters: Counters,
    /// Captured divergences (cloned out of the runner's buffer).
    pub divergences: Vec<DivergenceRecord>,
}

impl ShadowReport {
    /// Short one-line human summary useful for stdout.
    pub fn pretty_line(&self) -> String {
        format!(
            "events={} divergences={} legacy(s/a/n)={}/{}/{} v2(s/a/n)={}/{}/{}",
            self.counters.events_total,
            self.counters.divergences_total,
            self.counters.submits_legacy,
            self.counters.aborts_legacy,
            self.counters.noops_legacy,
            self.counters.submits_v2,
            self.counters.aborts_v2,
            self.counters.noops_v2,
        )
    }
}

/// Orchestrates a single replay run.
///
/// Construction panics if the two kernels share an `impl_id()` — see
/// rubber-duck blocker #6: two identical kernels trivially agree and
/// would give a false "green" parity run.
pub struct ShadowRunner<L: DecisionKernel, V: DecisionKernel, J: ShadowJournal> {
    legacy: L,
    v2: V,
    journal: J,
    divergences: Vec<DivergenceRecord>,
    counters: Counters,
}

impl<L: DecisionKernel, V: DecisionKernel, J: ShadowJournal> ShadowRunner<L, V, J> {
    /// Build a new runner. Panics if `legacy.impl_id() == v2.impl_id()`.
    pub fn new(legacy: L, v2: V, journal: J) -> Self {
        assert_ne!(
            legacy.impl_id(),
            v2.impl_id(),
            "ShadowRunner: legacy.impl_id() == v2.impl_id() ({}); refusing to run — \
             identical impls give a meaningless green parity report. \
             See rubber-duck blocker #6.",
            legacy.impl_id()
        );
        Self {
            legacy,
            v2,
            journal,
            divergences: Vec::new(),
            counters: Counters::default(),
        }
    }

    /// Run the replay to completion.
    pub fn run<I: IntoIterator<Item = DecisionInput>>(&mut self, inputs: I) {
        for input in inputs {
            self.counters.events_total += 1;
            let l = self.legacy.decide(&input);
            let v = self.v2.decide(&input);

            bump_variant_counter(&l, &mut self.counters, Side::Legacy);
            bump_variant_counter(&v, &mut self.counters, Side::V2);

            let lfp = fingerprint(&l);
            let vfp = fingerprint(&v);
            if lfp != vfp {
                self.counters.divergences_total += 1;
                let rec = DivergenceRecord {
                    event_id: input.raw_event.event_id.raw(),
                    run_id: input.run_id,
                    event_key: input.event_key.clone(),
                    legacy_fp: lfp,
                    v2_fp: vfp,
                    legacy_outcome_summary: summarize(&l),
                    v2_outcome_summary: summarize(&v),
                    first_differing_field: classify_diff(&l, &v),
                };
                self.journal.record(rec.clone());
                self.divergences.push(rec);
            }
        }
    }

    /// Aggregated report.
    pub fn report(&self) -> ShadowReport {
        ShadowReport {
            counters: self.counters,
            divergences: self.divergences.clone(),
        }
    }

    /// Borrow the journal (useful for tests).
    pub fn journal(&self) -> &J {
        &self.journal
    }
}

#[derive(Copy, Clone)]
enum Side {
    Legacy,
    V2,
}

fn bump_variant_counter(o: &DecisionOutcome, c: &mut Counters, s: Side) {
    match (o, s) {
        (DecisionOutcome::Submitted { .. }, Side::Legacy) => c.submits_legacy += 1,
        (DecisionOutcome::Submitted { .. }, Side::V2) => c.submits_v2 += 1,
        (DecisionOutcome::Aborted { .. }, Side::Legacy) => c.aborts_legacy += 1,
        (DecisionOutcome::Aborted { .. }, Side::V2) => c.aborts_v2 += 1,
        (DecisionOutcome::NoOp { .. }, Side::Legacy) => c.noops_legacy += 1,
        (DecisionOutcome::NoOp { .. }, Side::V2) => c.noops_v2 += 1,
    }
}

fn classify_diff(l: &DecisionOutcome, v: &DecisionOutcome) -> DivergenceField {
    match (l, v) {
        (DecisionOutcome::Submitted { intent_hash: a, .. },
         DecisionOutcome::Submitted { intent_hash: b, .. }) => {
            if a.0 != b.0 {
                DivergenceField::SubmitIntentHash
            } else {
                DivergenceField::Opaque
            }
        }
        (DecisionOutcome::Aborted { reason: a, .. },
         DecisionOutcome::Aborted { reason: b, .. }) => {
            if *a as u8 != *b as u8 {
                DivergenceField::AbortReason
            } else {
                DivergenceField::Opaque
            }
        }
        (DecisionOutcome::NoOp { reason: a },
         DecisionOutcome::NoOp { reason: b }) => {
            if classify_noop(a) != classify_noop(b) {
                DivergenceField::NoOpCode
            } else {
                DivergenceField::Opaque
            }
        }
        _ => DivergenceField::Variant,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{BookSnapshot, DecisionInput, ResolvedMetadata};
    use crate::journal::MemoryJournal;
    use crate::kernel::{KernelState, Position, StubKernel};
    use blink_timestamps::{init_with_policy, InitPolicy, Timestamp};
    use blink_types::{RawEvent, SourceKind};

    fn mk_input(i: usize) -> DecisionInput {
        DecisionInput {
            run_id: 1,
            event_key: format!("event-{}", i),
            raw_event: RawEvent::minimal(
                SourceKind::Manual,
                "0xtok".into(),
                Timestamp::UNSET,
            ),
            book_snapshot: BookSnapshot {
                best_bid_price: None,
                best_bid_size: None,
                best_ask_price: None,
                best_ask_size: None,
                snapshot_age_ms: 0,
            },
            kernel_state: KernelState {
                position: Position::default(),
                cooldown_until: None,
            },
            resolved_metadata: ResolvedMetadata {
                token_id: "0xtok".into(),
                market_id: "0xmkt".into(),
                title: "t".into(),
                outcome: "YES".into(),
                venue_fees_bps: 0,
            },
            config_hash: [0u8; 32],
            logical_now: Timestamp::UNSET,
        }
    }

    #[test]
    #[should_panic(expected = "impl_id")]
    fn identical_impl_ids_panic() {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        let a = StubKernel::noop(7, "a", "below edge threshold");
        let b = StubKernel::noop(7, "b", "below edge threshold");
        let _r = ShadowRunner::new(a, b, MemoryJournal::new());
    }

    #[test]
    fn zero_divergences_when_kernels_agree() {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        let a = StubKernel::noop(1, "legacy", "below edge threshold");
        let b = StubKernel::noop(2, "v2", "below edge threshold");
        let mut r = ShadowRunner::new(a, b, MemoryJournal::new());
        r.run((0..50).map(mk_input));
        let rep = r.report();
        assert_eq!(rep.counters.events_total, 50);
        assert_eq!(rep.counters.divergences_total, 0);
        assert!(rep.divergences.is_empty());
    }

    #[test]
    fn synthetic_divergence_is_recorded() {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        let legacy = StubKernel::noop(1, "legacy", "below edge threshold");
        let v2 = StubKernel::diverging(2, "v2", "below edge threshold", "event-42");
        let mut r = ShadowRunner::new(legacy, v2, MemoryJournal::new());
        r.run((0..100).map(mk_input));
        let rep = r.report();
        assert_eq!(rep.counters.events_total, 100);
        assert_eq!(rep.counters.divergences_total, 1);
        assert_eq!(rep.divergences[0].event_key, "event-42");
        assert_eq!(rep.divergences[0].first_differing_field, DivergenceField::Variant);
        assert_eq!(r.journal().len(), 1);
    }
}
