//! Scenario 5 — rpc_stall_95p
//!
//! Stand up a `MockPolygonWs` that stalls for 500 ms on every Nth frame
//! (5 % == every 20th). Drive `blink_ingress::BlockchainLogsSource` and
//! assert:
//!
//! * `events_ingested` accumulates events (the stall is tolerated).
//! * `reconnects == 0` — stalls are not drops.
//!
//! Note: `BlockchainLogsSource` expects JSON-RPC subscription frames,
//! so the mock emits well-formed `eth_subscription` notifications.
//! This scenario focuses on behaviour (no spurious reconnects under
//! stalled upstream), not latency SLO numbers — those live in
//! `blink-benches`.

use std::sync::atomic::Ordering;
use std::time::Duration;

use blink_chaos::mock::polygon_ws::{MockPolygonWs, MockWsBehaviour};
use blink_ingress::{BlockchainLogsConfig, BlockchainLogsSource, ShutdownToken, Source};
use blink_rings::bounded;
use blink_types::RawEvent;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_stall_95p() {
    let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);

    // Emit 60 JSON-RPC subscription notifications. Stall every 20th
    // frame by 500 ms (== 5 % of calls). Total wall-clock cost ≈
    // (60/20) * 0.5 s = 1.5 s.
    let mut frames = Vec::new();
    // First, the server has to respond to the client's subscribe RPC
    // with `{"id":1,"result":"0xSUB"}`. `BlockchainLogsSource` sends
    // an `eth_subscribe` request on connect and awaits that reply.
    frames.push(
        r#"{"jsonrpc":"2.0","id":1,"result":"0xsubid"}"#.to_string(),
    );
    for i in 0..60u32 {
        frames.push(format!(
            r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0xsubid","result":{{"address":"0x4bfb41d5b3570defd03c39a9a4d8de6bd8b8982e","topics":["0x0"],"data":"0x","logIndex":"0x{:x}","transactionHash":"0x{:064x}"}}}}}}"#,
            i, i
        ));
    }

    let behaviour = MockWsBehaviour {
        frames,
        drop_after: None,
        stall_every_nth_frame: 20,
        stall_ms: 500,
        inter_frame_ms: 1,
        ..Default::default()
    };
    let server = MockPolygonWs::spawn(behaviour).await;
    let url = server.url.clone();

    let cfg = BlockchainLogsConfig {
        url,
        addresses: vec!["0x4bfb41d5b3570defd03c39a9a4d8de6bd8b8982e".to_string()],
        topics: vec![],
    };
    let src = BlockchainLogsSource::new(cfg);
    let counters = src.stats_handle();

    let (prod, _cons) = bounded::<RawEvent>(1 << 14);
    let shutdown = ShutdownToken::new();
    let shutdown_cl = shutdown.clone();

    let handle = std::thread::spawn(move || {
        Box::new(src).run(prod, shutdown_cl);
    });

    // Wait for at least 40 events (past several stall points) or 8 s.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let ing = counters.events_ingested.load(Ordering::Relaxed);
        if ing >= 40 {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "stalled ingest: ingested={}, reconnects={}",
                ing,
                counters.reconnects.load(Ordering::Relaxed)
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Key invariant: a stall is NOT a drop — `reconnects` stays at 0.
    let reconnects = counters.reconnects.load(Ordering::Relaxed);
    assert_eq!(
        reconnects, 0,
        "stalls should not count as reconnects (got {reconnects})"
    );

    shutdown.cancel();
    let _ = handle.join();
}
