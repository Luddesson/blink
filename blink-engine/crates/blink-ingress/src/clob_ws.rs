//! `ClobWsSource` — subscribes to the Polymarket CLOB live-activity
//! WebSocket. Reconnects with exponential backoff (25 ms → 2 s).

use std::sync::Arc;
use std::time::Duration;

use blink_rings::Producer;
use blink_timestamps::Timestamp;
use blink_types::{RawEvent, SourceKind, wall_clock_ns, EventId};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::{Source, SourceCounters, ShutdownToken, try_push};

const MIN_BACKOFF: Duration = Duration::from_millis(25);
const MAX_BACKOFF: Duration = Duration::from_millis(2000);

/// Configuration for [`ClobWsSource`].
#[derive(Debug, Clone)]
pub struct ClobWsConfig {
    /// WS endpoint URL. Default upstream:
    /// `wss://ws-subscriptions-clob.polymarket.com/ws/live-activity`.
    pub url: String,
    /// Optional JSON subscribe frame sent immediately after connect.
    /// `None` means no subscribe frame (server pushes by default).
    pub subscribe_frame: Option<String>,
}

impl Default for ClobWsConfig {
    fn default() -> Self {
        Self {
            url: "wss://ws-subscriptions-clob.polymarket.com/ws/live-activity".to_string(),
            subscribe_frame: None,
        }
    }
}

/// CLOB WS source.
pub struct ClobWsSource {
    cfg: ClobWsConfig,
    counters: Arc<SourceCounters>,
}

impl ClobWsSource {
    /// Construct.
    pub fn new(cfg: ClobWsConfig) -> Self {
        Self {
            cfg,
            counters: SourceCounters::new(),
        }
    }
}

impl Source for ClobWsSource {
    fn kind(&self) -> SourceKind {
        SourceKind::ClobWs
    }
    fn stats_handle(&self) -> Arc<SourceCounters> {
        self.counters.clone()
    }

    fn run(self: Box<Self>, sink: Producer<RawEvent>, shutdown: ShutdownToken) {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let counters = self.counters.clone();
        let cfg = self.cfg.clone();
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                log::error!("clob-ws: cannot build tokio runtime: {e}");
                return;
            }
        };
        rt.block_on(run_loop(cfg, sink, counters, shutdown));
    }
}

async fn run_loop(
    cfg: ClobWsConfig,
    mut sink: Producer<RawEvent>,
    counters: Arc<SourceCounters>,
    shutdown: ShutdownToken,
) {
    let mut backoff = MIN_BACKOFF;
    while !shutdown.is_cancelled() {
        match tokio_tungstenite::connect_async(&cfg.url).await {
            Ok((mut ws, _)) => {
                backoff = MIN_BACKOFF;
                if let Some(frame) = &cfg.subscribe_frame {
                    if let Err(e) = ws.send(Message::Text(frame.clone().into())).await {
                        log::warn!("clob-ws: subscribe send failed: {e}");
                        counters
                            .reconnects
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                }
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => {
                            let _ = ws.close(None).await;
                            return;
                        }
                        msg = ws.next() => match msg {
                            Some(Ok(Message::Text(t))) => {
                                if let Some(ev) = make_raw_event(t.as_bytes()) {
                                    try_push(&mut sink, &counters, ev);
                                }
                            }
                            Some(Ok(Message::Binary(b))) => {
                                if let Some(ev) = make_raw_event(&b) {
                                    try_push(&mut sink, &counters, ev);
                                }
                            }
                            Some(Ok(Message::Ping(p))) => {
                                let _ = ws.send(Message::Pong(p)).await;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                log::warn!("clob-ws: frame error: {e}");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!("clob-ws: connect failed: {e}");
            }
        }
        counters
            .reconnects
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.cancelled() => return,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn make_raw_event(payload: &[u8]) -> Option<RawEvent> {
    // Keep this permissive — upstream schema evolves. We emit a RawEvent
    // with the raw JSON as `extra`; downstream parsers decode lazily.
    if payload.is_empty() {
        return None;
    }
    Some(RawEvent {
        event_id: EventId::fetch_next(),
        source: SourceKind::ClobWs,
        source_seq: u64::MAX,
        anchor: None,
        token_id: String::new(),
        market_id: None,
        side: None,
        price: None,
        size: None,
        tsc_in: Timestamp::now(),
        wall_ns: wall_clock_ns(),
        extra: Some(payload.to_vec().into_boxed_slice()),
        observe_only: false,
        maker_wallet: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Tiny in-process WS echo server. Accepts one connection, sends the
    /// provided text messages in sequence, then closes.
    async fn run_test_server(messages: Vec<String>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("ws://{addr}/ws");
        let h = tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                for m in messages {
                    let _ = ws.send(Message::Text(m.into())).await;
                }
                let _ = ws.close(None).await;
            }
        });
        (url, h)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parses_messages_from_mock_ws() {
        let msgs = vec![
            r#"{"m":"fill","tokenId":"0xa","price":0.6,"size":1}"#.to_string(),
            r#"{"m":"book","tokenId":"0xa","bids":[]}"#.to_string(),
        ];
        let (url, server) = run_test_server(msgs).await;
        let (prod, mut cons) = blink_rings::bounded::<RawEvent>(16);
        let src = ClobWsSource::new(ClobWsConfig {
            url,
            subscribe_frame: None,
        });
        let counters = src.stats_handle();
        let shutdown = ShutdownToken::new();
        let shutdown_clone = shutdown.clone();
        let run_handle = std::thread::spawn(move || {
            Box::new(src).run(prod, shutdown_clone);
        });

        // Wait until two events land or 3s timeout.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = Vec::new();
        while got.len() < 2 && std::time::Instant::now() < deadline {
            if let Some(ev) = cons.pop() {
                got.push(ev);
            } else {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        shutdown.cancel();
        let _ = server.await;
        let _ = run_handle.join();

        assert_eq!(got.len(), 2, "expected two RawEvents from mock WS");
        for ev in &got {
            assert_eq!(ev.source, SourceKind::ClobWs);
            assert!(ev.extra.is_some());
            assert!(!ev.observe_only);
        }
        let stats = counters.snapshot();
        assert!(stats.events_ingested >= 2);
    }
}
