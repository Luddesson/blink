# R-5 — Observer effect (journaling / metrics on the hot path)

**Todo**: `r5-observer`. **Status**: architecture implemented in `blink-journal`; this doc
is the **invariant statement** and follow-up test plan.

## Invariants (must hold forever)

1. **No `.await` from decision/signer/submitter threads** touching the journal.
   Journal ingress is `try_send` on a bounded tokio mpsc. Blocking = bug.
2. **No locks on the hot path** from metrics recording. HDR histograms are per-thread;
   merged off-critical by a background tokio task every 1 s.
3. **Drop-on-full is acceptable and counted.** `blink_journal_rows_dropped_total` is a
   hard SLO: > 0.1 % drops over 10 m ⇒ warn, > 1 % ⇒ page. The hot path MUST NOT slow
   down to avoid drops.
4. **No allocation in the decision kernel except into arenas.** Journal row construction
   happens *after* the decision is published; it may allocate a `String` for `AbortReason`
   context, but must not appear in the `tsc_in..tsc_decision` span.

## Follow-up tests to add (tracked under p0-shadow-capture extensions)

- **Overhead micro-bench**: `blink-benches` includes `stage_stamp` today. Add a new bench
  `decision_with_journal` that runs the stub decision kernel + journal enqueue vs. kernel
  alone. Gate: journal overhead < 50 ns median on the colo box.
- **Drop-stress test**: flood the journal at 10× capacity for 10 s; assert `rows_dropped`
  > 0, assert decision-kernel P99 latency unchanged vs. baseline (within 3σ noise).
  Lives in `blink-journal/tests/overhead.rs`. (Currently not implemented — TODO.)

## Things that DO cross the observer boundary and must be watched

- `log::debug!` calls in hot-path code — audit and remove. `log::trace!` is OK only if
  `log_filter` is `off` in prod.
- String allocations in `DecisionOutcome::NoOp { reason }`. Today `reason` is `String`;
  that allocation MUST happen *after* the decision is emitted, not as part of deciding.
  Add a clippy-level rule or a test that greps the decision kernel source for
  `String::from`/`format!`/`.to_string()` and fails if found.

## Metrics exported (Prometheus names)

```
blink_journal_rows_pushed_total
blink_journal_rows_dropped_total
blink_journal_rows_flushed_total
blink_journal_flush_duration_seconds (histogram)
blink_hdr_merge_duration_seconds (histogram)
```
