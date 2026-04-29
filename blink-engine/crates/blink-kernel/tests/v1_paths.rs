//! Per-path coverage of the v1 kernel's 8 check stages plus stability
//! / overflow invariants. Uses the `test-support` feature for builders.

#![cfg(feature = "test-support")]

use blink_kernel::test_support::{book_with_top, fixture, raw_event};
use blink_kernel::{
    DecisionKernel, KernelConfig, KernelStats, KernelVerdict, V1Kernel,
};
use blink_shadow::NoOpCode;
use blink_types::{AbortReason, Side, SourceKind};

fn decide_once(f: &blink_kernel::test_support::Fixture) -> (KernelVerdict<'_>, KernelStats) {
    let snap = f.snapshot();
    let mut stats = KernelStats::new();
    let v = V1Kernel::new().decide(&snap, &mut stats);
    (v, stats)
}

#[test]
fn impl_id_is_stable() {
    assert_eq!(V1Kernel::new().impl_id(), "blink-kernel-v1");
}

#[test]
fn observe_only_shortcircuits_to_filter_mismatch() {
    // Mempool-tap / legal-gated events must never reach Submit.
    // See docs/rebuild/R3_LEGAL_MEMO_STUB.md.
    let mut f = fixture();
    f.event.observe_only = true;
    let (v, _) = decide_once(&f);
    assert!(matches!(
        v,
        KernelVerdict::NoOp { code: NoOpCode::FilterMismatch }
    ));
}

#[test]
fn submit_happy_path() {
    let f = fixture();
    let (v, _) = decide_once(&f);
    match v {
        KernelVerdict::Submit { .. } => {}
        other => panic!("expected Submit, got {other:?}"),
    }
}

#[test]
fn stale_book_aborts() {
    let mut f = fixture();
    f.logical_now_ns = 1_000_000_000 + 2_000_000_000;
    let (v, _) = decide_once(&f);
    assert!(matches!(
        v,
        KernelVerdict::Abort { reason: AbortReason::StaleBook, .. }
    ));
}

#[test]
fn fresh_book_does_not_abort_stale() {
    let f = fixture();
    let (v, _) = decide_once(&f);
    assert!(!matches!(
        v,
        KernelVerdict::Abort { reason: AbortReason::StaleBook, .. }
    ));
}

#[test]
fn drift_exceeds_limit_aborts() {
    let mut f = fixture();
    f.config = KernelConfig {
        max_drift_bps: 1,
        ..KernelConfig::conservative()
    };
    // Mid = 510. Price 505 ⇒ ~-98 bps drift, well over the 1 bps cap.
    f.event = raw_event("0xtoken", "0xmarket", Side::Buy, 505, 1_000);
    let (v, _) = decide_once(&f);
    match v {
        KernelVerdict::Abort { reason: AbortReason::Drift, metric_bps: Some(m) } => {
            assert!(m.abs() > 1, "expected sizeable drift metric, got {m}");
        }
        other => panic!("expected Drift abort, got {other:?}"),
    }
}

#[test]
fn small_drift_does_not_abort() {
    let mut f = fixture();
    f.config = KernelConfig {
        max_drift_bps: 500,
        ..KernelConfig::conservative()
    };
    let (v, _) = decide_once(&f);
    assert!(!matches!(
        v,
        KernelVerdict::Abort { reason: AbortReason::Drift, .. }
    ));
}

#[test]
fn post_only_cross_aborts() {
    let mut f = fixture();
    f.event = raw_event("0xtoken", "0xmarket", Side::Buy, 520, 1_000);
    f.config.default_post_only = true;
    f.config.max_drift_bps = 10_000;
    let (v, _) = decide_once(&f);
    assert!(matches!(
        v,
        KernelVerdict::Abort { reason: AbortReason::PostOnlyCross, .. }
    ));
}

#[test]
fn non_crossing_does_not_trip_post_only() {
    let mut f = fixture();
    f.config.default_post_only = true;
    f.config.max_drift_bps = 10_000;
    let (v, _) = decide_once(&f);
    assert!(!matches!(
        v,
        KernelVerdict::Abort { reason: AbortReason::PostOnlyCross, .. }
    ));
}

#[test]
fn cooldown_active_noop() {
    let mut f = fixture();
    f.position.cooldown_until_ns = f.logical_now_ns + 10_000_000;
    let (v, _) = decide_once(&f);
    assert!(matches!(v, KernelVerdict::NoOp { code: NoOpCode::CooldownActive }));
}

#[test]
fn cooldown_expired_no_noop() {
    let mut f = fixture();
    f.position.cooldown_until_ns = f.logical_now_ns.saturating_sub(1);
    let (v, _) = decide_once(&f);
    assert!(!matches!(v, KernelVerdict::NoOp { code: NoOpCode::CooldownActive }));
}

#[test]
fn risk_limit_aborts_on_notional_cap() {
    let mut f = fixture();
    f.config.max_position_notional = 1;
    let (v, _) = decide_once(&f);
    assert!(matches!(v, KernelVerdict::Abort { reason: AbortReason::RiskLimit, .. }));
}

#[test]
fn risk_ok_under_cap() {
    let mut f = fixture();
    f.config.max_position_notional = u64::MAX;
    let (v, _) = decide_once(&f);
    assert!(!matches!(v, KernelVerdict::Abort { reason: AbortReason::RiskLimit, .. }));
}

#[test]
fn risk_adversarial_i64_extremes_do_not_panic() {
    let mut f = fixture();
    f.position.open_qty_signed_u = i64::MAX;
    f.event = raw_event("0xtoken", "0xmarket", Side::Buy, 510, u64::MAX);
    f.config.max_drift_bps = 10_000;
    let (v, _stats) = decide_once(&f);
    assert!(matches!(v, KernelVerdict::Abort { reason: AbortReason::RiskLimit, .. }));

    let mut g = fixture();
    g.position.open_qty_signed_u = i64::MIN;
    g.event = raw_event("0xtoken", "0xmarket", Side::Sell, 510, u64::MAX);
    g.config.max_drift_bps = 10_000;
    let (v2, _) = decide_once(&g);
    assert!(matches!(v2, KernelVerdict::Abort { reason: AbortReason::RiskLimit, .. }));
}

#[test]
fn below_edge_threshold_noop() {
    let mut f = fixture();
    f.book = book_with_top("0xtoken", "0xmarket", 509, 511, f.book.source_wall_ns);
    f.event = raw_event("0xtoken", "0xmarket", Side::Buy, 510, 1_000);
    f.config = KernelConfig {
        edge_threshold_bps: 10_000,
        max_drift_bps: 10_000,
        ..KernelConfig::conservative()
    };
    let (v, _) = decide_once(&f);
    assert!(matches!(v, KernelVerdict::NoOp { code: NoOpCode::BelowEdgeThreshold }));
}

#[test]
fn sufficient_edge_does_not_noop() {
    let f = fixture();
    let (v, _) = decide_once(&f);
    assert!(!matches!(v, KernelVerdict::NoOp { code: NoOpCode::BelowEdgeThreshold }));
}

#[test]
fn dedup_hit_noop() {
    let mut f = fixture();
    let (v0, _) = decide_once(&f);
    let key = match v0 {
        KernelVerdict::Submit { semantic_key, .. } => semantic_key,
        other => panic!("setup expected Submit, got {other:?}"),
    };
    f.recent.insert(key.0);
    let (v, _) = decide_once(&f);
    assert!(matches!(v, KernelVerdict::NoOp { code: NoOpCode::Dedup }));
}

#[test]
fn non_actionable_source_is_filter_mismatch() {
    let mut f = fixture();
    f.event.source = SourceKind::CtfLog;
    let (v, _) = decide_once(&f);
    assert!(matches!(v, KernelVerdict::NoOp { code: NoOpCode::FilterMismatch }));
}

#[test]
fn missing_price_is_filter_mismatch() {
    let mut f = fixture();
    f.event.price = None;
    let (v, _) = decide_once(&f);
    assert!(matches!(v, KernelVerdict::NoOp { code: NoOpCode::FilterMismatch }));
}

#[test]
fn semantic_key_is_stable_across_repeats() {
    let f = fixture();
    let snap = f.snapshot();
    let mut stats = KernelStats::new();
    let k0 = match V1Kernel::new().decide(&snap, &mut stats) {
        KernelVerdict::Submit { semantic_key, .. } => semantic_key,
        other => panic!("expected Submit, got {other:?}"),
    };
    for _ in 0..100 {
        let snap = f.snapshot();
        let mut s = KernelStats::new();
        let k = match V1Kernel::new().decide(&snap, &mut s) {
            KernelVerdict::Submit { semantic_key, .. } => semantic_key,
            other => panic!("expected Submit, got {other:?}"),
        };
        assert_eq!(k.0, k0.0);
    }
}

#[test]
fn semantic_key_independent_of_attempt() {
    use blink_kernel::verdict_to_outcome;
    use blink_types::DecisionOutcome;

    let f = fixture();

    let snap0 = f.snapshot();
    let mut s0 = KernelStats::new();
    let v0 = V1Kernel::new().decide(&snap0, &mut s0);
    let k0 = match &v0 {
        KernelVerdict::Submit { semantic_key, .. } => *semantic_key,
        _ => unreachable!(),
    };
    let out0 = verdict_to_outcome(v0, &f.run_id, 0);

    let snap1 = f.snapshot();
    let mut s1 = KernelStats::new();
    let v1 = V1Kernel::new().decide(&snap1, &mut s1);
    let k1 = match &v1 {
        KernelVerdict::Submit { semantic_key, .. } => *semantic_key,
        _ => unreachable!(),
    };
    let out1 = verdict_to_outcome(v1, &f.run_id, 1);

    assert_eq!(k0.0, k1.0);

    match (out0, out1) {
        (
            DecisionOutcome::Submitted { intent_hash: h0, client_order_id: c0 },
            DecisionOutcome::Submitted { intent_hash: h1, client_order_id: c1 },
        ) => {
            assert_ne!(c0, c1);
            assert_ne!(h0.0, h1.0);
        }
        _ => panic!("expected both Submitted"),
    }
}

#[test]
fn stats_counters_increment() {
    let f = fixture();
    let snap = f.snapshot();
    let mut stats = KernelStats::new();
    let _ = V1Kernel::new().decide(&snap, &mut stats);
    assert_eq!(stats.decisions_total, 1);
    assert_eq!(stats.submitted, 1);
}
