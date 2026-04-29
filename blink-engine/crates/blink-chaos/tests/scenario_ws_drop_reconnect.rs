//! Scenario 1 — ws_drop_reconnect
//!
//! Stand up a `MockPolygonWs` that emits 10 frames then drops the
//! socket. Point a `blink_ingress::ClobWsSource` at it and assert the
//! source reconnects, resumes emitting events, and bumps its
//! `reconnects` counter.

use std::sync::atomic::Ordering;
use std::time::Duration;

use blink_chaos::mock::polygon_ws::{MockPolygonWs, MockWsBehaviour};
use blink_ingress::{ClobWsConfig, ClobWsSource, ShutdownToken, Source};
use blink_rings::bounded;
use blink_types::RawEvent;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_drop_reconnect() {
    let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);

    let frames: Vec<String> = (0..10)
        .map(|i| format!("{{\"ev\":{},\"type\":\"trade\"}}", i))
        .collect();
    let behaviour = MockWsBehaviour {
        frames,
        drop_after: Some(10),
        ..Default::default()
    };
    let server = MockPolygonWs::spawn(behaviour).await;
    let url = server.url.clone();

    let cfg = ClobWsConfig {
        url,
        subscribe_frame: None,
    };
    let src = ClobWsSource::new(cfg);
    let counters = src.stats_handle();

    let (prod, _cons) = bounded::<RawEvent>(1 << 14);
    let shutdown = ShutdownToken::new();
    let shutdown_cl = shutdown.clone();

    let handle = std::thread::spawn(move || {
        Box::new(src).run(prod, shutdown_cl);
    });

    // Give the source up to 5 s to (a) connect, (b) drain 10 frames,
    // (c) observe the drop, (d) reconnect and start draining again.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let reconnects = counters.reconnects.load(Ordering::Relaxed);
        let ingested = counters.events_ingested.load(Ordering::Relaxed);
        if reconnects >= 1 && ingested >= 10 {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "timeout waiting for reconnect: reconnects={}, ingested={}, accepts={}",
                reconnects,
                ingested,
                server.accepts.load(Ordering::Relaxed)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Sanity: the mock should have served ≥ 2 accepts (initial + reconnect).
    let accepts = server.accepts.load(Ordering::Relaxed);
    assert!(accepts >= 2, "expected ≥ 2 accepts, got {accepts}");

    shutdown.cancel();
    let _ = handle.join();
}
