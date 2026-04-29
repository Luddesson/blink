//! Scenario 3 — rate_limit_429_streak
//!
//! Mock CLOB returns `429 Retry-After: 30` for 5 consecutive requests.
//! Feed each into `BreakerSet::on_rate_limit_429` and assert:
//!
//! * `rate_limit` breaker trips once the streak threshold is crossed.
//! * `admit_submit` rejects with `BreakerTrip::RateLimit429`.
//! * After `cool_off_ms`, the breaker moves to HalfOpen and a success
//!   probe closes it.

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
async fn rate_limit_429_streak() {
    let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);

    let behaviour = MockClobBehaviour {
        fail_streak: 5,
        fail_status: 429,
        fail_body: Bytes::from_static(b"{\"error\":\"rate limited\"}"),
        fail_retry_after_secs: Some(30),
        ..Default::default()
    };
    let server = MockClobServer::spawn(behaviour).await;
    let port = server.addr.port();
    let h2cfg = H2Config::new("localhost", port, client_config_trusting(server.trust_cert.clone()));
    let h2 = Arc::new(H2Client::spawn(h2cfg));
    h2.ensure_connected().await.expect("h2 connect");

    let mut set_cfg = BreakerSetConfig::default();
    set_cfg.rate_limit_429_streak_threshold = 3;
    set_cfg.rate_limit = BreakerConfig {
        // The rate-limit breaker is driven by the streak counter, not
        // the error-rate path — so keep min_samples infinite.
        error_rate_pct_threshold: 100,
        error_rate_window_ms: 1_000,
        latency_p99_ns_threshold: 0,
        latency_window_ms: 1_000,
        cool_off_ms: 200,
        half_open_probe_every_ms: 50,
        min_samples: u32::MAX,
    };
    let breakers = BreakerSet::new(set_cfg);

    let mut now_ns: u64 = 1_000_000_000;
    let mut saw_retry_after = false;
    for _ in 0..5 {
        let resp = h2
            .post("/order", &[("content-type", "application/json")], Bytes::from_static(b"{}"))
            .await
            .expect("post");
        assert_eq!(resp.status, 429);
        if resp
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("retry-after") && v == "30")
        {
            saw_retry_after = true;
        }
        breakers.on_rate_limit_429(now_ns);
        now_ns += Duration::from_millis(10).as_nanos() as u64;
    }
    assert!(saw_retry_after, "mock should have emitted Retry-After: 30");

    // After ≥3 429s the rate_limit breaker should be Open.
    assert!(matches!(
        **breakers.rate_limit.state(),
        BreakerState::Open {
            reason: BreakerTrip::RateLimit429 { .. },
            ..
        }
    ));
    assert!(matches!(
        breakers.admit_submit(now_ns),
        Admission::Reject(BreakerTrip::RateLimit429 { .. })
    ));

    // Wait out the cool-off, then probe → Closed.
    now_ns += Duration::from_millis(500).as_nanos() as u64;
    let probe = breakers.rate_limit.admit(now_ns);
    assert!(matches!(probe, Admission::Ok));
    breakers.rate_limit.record_outcome(
        blink_breakers::Outcome {
            ok: true,
            latency_ns: 1_000_000,
        },
        now_ns,
    );
    assert!(matches!(**breakers.rate_limit.state(), BreakerState::Closed));
}
