//! Harness self-test. Proves the shadow runner detects a synthetic
//! divergence and that identical-behaviour kernels with different
//! `impl_id`s produce zero divergences.
//!
//! Exit code: 0 on pass, 1 on fail. This is what CI runs for the
//! `p0-shadow` todo.

use std::process::ExitCode;

use blink_shadow::{
    BookSnapshot, DecisionInput, KernelState, MemoryJournal, Position, ResolvedMetadata,
    ShadowRunner, StubKernel,
};
use blink_timestamps::{init_with_policy, InitPolicy, Timestamp};
use blink_types::{RawEvent, SourceKind};

fn mk_input(i: usize) -> DecisionInput {
    DecisionInput {
        run_id: 1,
        event_key: format!("event-{}", i),
        raw_event: RawEvent::minimal(
            SourceKind::Manual,
            "0xselftest".into(),
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
            token_id: "0xselftest".into(),
            market_id: "0xmkt".into(),
            title: "selftest".into(),
            outcome: "YES".into(),
            venue_fees_bps: 0,
        },
        config_hash: [0u8; 32],
        logical_now: Timestamp::UNSET,
    }
}

fn main() -> ExitCode {
    let _ = init_with_policy(InitPolicy::AllowFallback);

    let mut ok = true;

    {
        let legacy = StubKernel::noop(1, "legacy", "below edge threshold");
        let v2 = StubKernel::diverging(2, "v2", "below edge threshold", "event-42");
        let mut runner = ShadowRunner::new(legacy, v2, MemoryJournal::new());
        runner.run((0..100).map(mk_input));
        let report = runner.report();
        println!("[selftest s1] {}", report.pretty_line());
        if report.counters.events_total != 100 {
            eprintln!("[selftest s1] FAIL: expected events_total=100, got {}", report.counters.events_total);
            ok = false;
        }
        if report.counters.divergences_total != 1 {
            eprintln!("[selftest s1] FAIL: expected divergences_total=1, got {}", report.counters.divergences_total);
            ok = false;
        }
        if report.divergences.first().map(|d| d.event_key.as_str()) != Some("event-42") {
            eprintln!("[selftest s1] FAIL: expected divergence at event-42, got {:?}", report.divergences.first().map(|d| d.event_key.clone()));
            ok = false;
        }
    }

    {
        let legacy = StubKernel::noop(11, "legacy", "below edge threshold");
        let v2 = StubKernel::noop(22, "v2", "below edge threshold");
        let mut runner = ShadowRunner::new(legacy, v2, MemoryJournal::new());
        runner.run((0..100).map(mk_input));
        let report = runner.report();
        println!("[selftest s2] {}", report.pretty_line());
        if report.counters.divergences_total != 0 {
            eprintln!("[selftest s2] FAIL: expected divergences_total=0, got {}", report.counters.divergences_total);
            ok = false;
        }
    }

    if ok {
        println!("[selftest] PASS — shadow harness detects synthetic divergence and passes on agreement");
        ExitCode::SUCCESS
    } else {
        eprintln!("[selftest] FAIL — see diagnostics above");
        ExitCode::FAILURE
    }
}
