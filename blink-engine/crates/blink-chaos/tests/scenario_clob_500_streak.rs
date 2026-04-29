//! Scenario 2 — clob_500_streak
//!
//! Stand up a `MockClobServer` that returns HTTP 500 for 20 consecutive
//! requests then returns 200 for the rest. Fire 25 submits; feed the
//! venue outcomes into a `blink_breakers::BreakerSet` using an
//! error-rate breaker configured to trip at 50 % over a short
//! window.
//!
//! Assertions:
//! * After the streak, `BreakerSet::submit.state()` is `Open`.
//! * `admit_submit` rejects once the breaker opens.
//! * After the cool-off elapses the breaker moves to HalfOpen and a
//!   subsequent successful outcome closes it.
//!
//! This does *not* wire the live `Submitter` through the breaker yet
//! (the submit hot path's breaker integration lands in a later phase).
//! Instead the scenario verifies the behavioural contract the two
//! crates must satisfy together: given the outcomes the mock produces,
//! the breaker transitions through the expected state graph.

use std::sync::Arc;
use std::time::Duration;

use blink_breakers::{
    Admission, BreakerConfig, BreakerSet, BreakerSetConfig, BreakerState, BreakerTrip,
};
use blink_chaos::mock::clob::{MockClobBehaviour, MockClobServer};
use blink_chaos::mock::tls::client_config_trusting;
use blink_h2::{H2Client, H2Config};
use bytes::Bytes;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clob_500_streak() {
    let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);

    // ── Mock server: 20-request 500 streak, then OK. ──────────────────
    let behaviour = MockClobBehaviour {
        fail_streak: 20,
        fail_status: 500,
        fail_body: Bytes::from_static(b"{\"error\":\"server error\"}"),
        ..Default::default()
    };
    let server = MockClobServer::spawn(behaviour).await;
    let port = server.addr.port();

    let mut h2cfg = H2Config::new("localhost", port, client_config_trusting(server.trust_cert.clone()));
    h2cfg.request_timeout = Duration::from_secs(3);
    let h2 = Arc::new(H2Client::spawn(h2cfg));
    h2.ensure_connected().await.expect("h2 connect");

    // ── BreakerSet tuned to trip fast. ────────────────────────────────
    let mut set_cfg = BreakerSetConfig::default();
    set_cfg.submit = BreakerConfig {
        error_rate_pct_threshold: 50,
        error_rate_window_ms: 5_000,
        latency_p99_ns_threshold: 0,
        latency_window_ms: 5_000,
        cool_off_ms: 200,
        half_open_probe_every_ms: 50,
        min_samples: 10,
    };
    let breakers = BreakerSet::new(set_cfg);

    let mut now_ns: u64 = 1_000_000_000;
    let mut rejects_after_trip = 0u64;
    let mut tripped_at: Option<u64> = None;

    for i in 0..25u32 {
        match breakers.submit.admit(now_ns) {
            Admission::Ok => {
                let resp = h2
                    .post("/order", &[("content-type", "application/json")], Bytes::from_static(b"{}"))
                    .await
                    .expect("post");
                let accepted = resp.status == 200;
                breakers.on_submit_outcome(accepted, 1_000_000, now_ns);
                if !accepted && tripped_at.is_none() {
                    if let BreakerState::Open { .. } = **breakers.submit.state() {
                        tripped_at = Some(i as u64);
                    }
                }
            }
            Admission::Reject(_) => {
                rejects_after_trip += 1;
            }
        }
        now_ns += 10_000_000; // +10 ms per iteration
    }

    // ── Assert 1: breaker tripped while the streak was in flight. ─────
    let tripped_at = tripped_at.expect("submit breaker should have tripped during the 500 streak");
    assert!(
        tripped_at <= 20,
        "breaker should trip inside the 20-request 500 streak (tripped_at={tripped_at})"
    );

    // ── Assert 2: while Open, admit_submit rejects. ───────────────────
    assert!(matches!(**breakers.submit.state(), BreakerState::Open { .. }));
    assert!(rejects_after_trip > 0, "expected ≥1 reject after trip");
    assert!(matches!(
        breakers.submit.admit(now_ns),
        Admission::Reject(BreakerTrip::ErrorRate { .. })
    ));

    // ── Assert 3: after cool-off we move to HalfOpen. ─────────────────
    // Jump now past `cool_off_ms` so admit claims the probe slot.
    now_ns += 500_000_000;
    let probe_admission = breakers.submit.admit(now_ns);
    assert!(
        matches!(probe_admission, Admission::Ok),
        "expected HalfOpen probe to be admitted, got {probe_admission:?}"
    );
    assert!(matches!(
        **breakers.submit.state(),
        BreakerState::HalfOpen { .. }
    ));

    // ── Assert 4: probe success → Closed. ─────────────────────────────
    // Fake-feed a successful outcome rather than actually posting, so
    // we don't depend on the mock's streak count.
    breakers.on_submit_outcome(true, 1_000_000, now_ns);
    assert!(matches!(**breakers.submit.state(), BreakerState::Closed));
    assert!(matches!(breakers.submit.admit(now_ns), Admission::Ok));

    let fails = server.fails.load(std::sync::atomic::Ordering::Relaxed);
    assert!(fails >= 10, "expected ≥10 failures served, got {fails}");
}
