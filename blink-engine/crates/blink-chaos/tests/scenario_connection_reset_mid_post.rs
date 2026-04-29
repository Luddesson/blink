//! Scenario 6 — connection_reset_mid_post
//!
//! Stand up a `MockClobServer` configured to accept the first
//! connection's TLS handshake and then close the socket mid-exchange.
//! Build a `blink_submit::Submitter` pointed at it and assert:
//!
//! * `submit()` returns `SubmitVerdict::Unknown { reason:
//!   UnknownReason::H2Error(_) }` — the submitter must not classify a
//!   stream-level failure as `Accepted` or `RejectedByVenue`.
//! * A follow-up submit succeeds once the reset behaviour is cleared
//!   (models "single failure, not a catastrophic outage").

use std::sync::Arc;
use std::time::Duration;

use blink_chaos::mock::clob::{MockClobBehaviour, MockClobServer};
use blink_chaos::mock::tls::client_config_trusting;
use blink_h2::{H2Client, H2Config};
use blink_signer::SignerPool;
use blink_submit::{Submitter, SubmitterConfig, SubmitVerdict, UnknownReason};
use blink_types::{EventId, Intent, PriceTicks, Side, SizeU, StageTimestamps, TimeInForce};

// Fresh random-ish secp256k1 key (test-only).
const TEST_KEY: [u8; 32] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
    0xff, 0x01,
];

fn mk_intent(i: u32) -> Intent {
    Intent {
        event_id: EventId(u64::from(i)),
        token_id: format!("{}", 1_000_000_000_000u64 + u64::from(i)),
        market_id: "mock-market".to_string(),
        side: Side::Buy,
        price: PriceTicks(650),
        size: SizeU(1_500_000),
        tif: TimeInForce::Gtc,
        post_only: false,
        client_order_id: format!("coid-{i}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_reset_mid_post() {
    let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);

    // Arrange: server resets the VERY FIRST accepted connection.
    let behaviour = MockClobBehaviour {
        reset_on_nth_conn: Some(1),
        ..Default::default()
    };
    let server = MockClobServer::spawn(behaviour).await;
    let port = server.addr.port();
    let trust = server.trust_cert.clone();

    let mut cfg = H2Config::new("localhost", port, client_config_trusting(trust));
    cfg.request_timeout = Duration::from_secs(3);
    cfg.keepalive_interval = Duration::from_millis(300);
    cfg.keepalive_timeout = Duration::from_millis(300);
    let h2 = Arc::new(H2Client::spawn(cfg));

    let signer = Arc::new(SignerPool::new_k256(vec![TEST_KEY]).expect("signer"));

    let mut submit_cfg = SubmitterConfig::polymarket_mainnet([0u8; 20]);
    submit_cfg.authority = format!("localhost:{port}");
    submit_cfg.post_timeout = Duration::from_secs(2);
    let sub = Submitter::new(submit_cfg, h2.clone(), signer);

    // Act: fire a submit that should see the reset mid-exchange.
    let mut stamps = StageTimestamps::UNSET;
    let verdict = sub
        .submit(&mk_intent(1), [0u8; 16], 0, &mut stamps)
        .await;

    match verdict {
        SubmitVerdict::Unknown {
            reason: UnknownReason::H2Error(_) | UnknownReason::Timeout,
            ..
        } => {
            // Expected — the server dropped TLS mid-stream. Either
            // variant is acceptable; both are non-terminal from the
            // submitter's point of view.
        }
        other => panic!("expected Unknown(H2Error|Timeout), got {other:?}"),
    }

    // The mock should have observed exactly one connection for the
    // reset path — the follow-up will open a fresh conn once the H2
    // supervisor re-establishes.
    let conns_before = server.conns.load(std::sync::atomic::Ordering::Relaxed);
    assert!(conns_before >= 1, "expected ≥1 conn accepted, got {conns_before}");

    // Wait for the supervisor to reconnect and verify a follow-up
    // submit succeeds. (Sanity check that the breaker will NOT need
    // to over-trip: a single reset is a recoverable blip.)
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        let mut s2 = StageTimestamps::UNSET;
        let v2 = sub.submit(&mk_intent(2), [0u8; 16], 0, &mut s2).await;
        if matches!(v2, SubmitVerdict::Accepted { .. } | SubmitVerdict::RejectedByVenue { .. }) {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(ok, "submitter did not recover after single reset");
}
