//! Scenario 4 — clock_skew_jump
//!
//! **SKIPPED.** The drift-watcher / test-clock shim in
//! `blink-timestamps` has not landed yet. This test is `#[ignore]`d
//! with an explanatory message so it shows up as a pending-upstream
//! item in `cargo test -p blink-chaos` output.
//!
//! When the shim lands, the test body should:
//!
//! 1. Install a test-only clock whose "now" is controllable.
//! 2. Inject a forward jump of +2 s, then a backward jump of -500 ms.
//! 3. Poll the drift watcher and assert it has flagged each jump.
//! 4. Verify the decision kernel's fallback path is taken (emits
//!    `DecisionOutcome::Aborted { reason: AbortReason::ClockDrift }`
//!    or equivalent — name TBD by that crate's API).

#[test]
#[ignore = "pending test-clock shim in blink-timestamps (see Phase 0 R-4 sub-tasks)"]
fn clock_skew_jump() {
    // When the shim lands, flesh out the body per the module doc
    // above. Keeping this as a no-op #[ignore] makes the missing
    // dependency visible in `cargo test` summaries.
    unimplemented!("requires blink-timestamps test-clock shim");
}
