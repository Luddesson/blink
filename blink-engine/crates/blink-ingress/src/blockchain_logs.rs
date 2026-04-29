//! `BlockchainLogsSource` — Polygon JSON-RPC WS subscriber for CTF
//! contract logs (`eth_subscribe("logs", filter)`).
//!
//! Each log notification is decoded into
//! `RawEvent { source: BlockchainLogs, anchor: Some(..), extra: <json>, .. }`
//! — topics + data live in the opaque `extra` payload so downstream
//! decoders remain free to evolve independently of this crate.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use blink_rings::Producer;
use blink_timestamps::Timestamp;
use blink_types::{EventId, OnChainAnchor, RawEvent, SourceKind, wall_clock_ns};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::{Source, SourceCounters, ShutdownToken, try_push};

const MIN_BACKOFF: Duration = Duration::from_millis(25);
const MAX_BACKOFF: Duration = Duration::from_millis(2000);

/// Configuration for [`BlockchainLogsSource`].
#[derive(Debug, Clone)]
pub struct BlockchainLogsConfig {
    /// Polygon WS URL (typically `ws://127.0.0.1:8546` for a local node).
    pub url: String,
    /// `address` filter(s) for the `eth_subscribe("logs", {..})` call.
    /// Accepts hex-string addresses, passed through verbatim to JSON-RPC.
    pub addresses: Vec<String>,
    /// Optional `topics` array filter (pass-through).
    pub topics: Vec<serde_json::Value>,
}

impl Default for BlockchainLogsConfig {
    fn default() -> Self {
        Self {
            url: "ws://127.0.0.1:8546".to_string(),
            addresses: vec![],
            topics: vec![],
        }
    }
}

/// Blockchain logs source.
pub struct BlockchainLogsSource {
    cfg: BlockchainLogsConfig,
    counters: Arc<SourceCounters>,
}

impl BlockchainLogsSource {
    /// Construct.
    pub fn new(cfg: BlockchainLogsConfig) -> Self {
        Self {
            cfg,
            counters: SourceCounters::new(),
        }
    }
}

impl Source for BlockchainLogsSource {
    fn kind(&self) -> SourceKind {
        SourceKind::BlockchainLogs
    }
    fn stats_handle(&self) -> Arc<SourceCounters> {
        self.counters.clone()
    }

    fn run(self: Box<Self>, sink: Producer<RawEvent>, shutdown: ShutdownToken) {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let cfg = self.cfg.clone();
        let counters = self.counters.clone();
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                log::error!("blockchain-logs: cannot build tokio runtime: {e}");
                return;
            }
        };
        rt.block_on(run_loop(cfg, sink, counters, shutdown));
    }
}

async fn run_loop(
    cfg: BlockchainLogsConfig,
    mut sink: Producer<RawEvent>,
    counters: Arc<SourceCounters>,
    shutdown: ShutdownToken,
) {
    let mut backoff = MIN_BACKOFF;
    while !shutdown.is_cancelled() {
        match tokio_tungstenite::connect_async(&cfg.url).await {
            Ok((mut ws, _)) => {
                backoff = MIN_BACKOFF;
                // Build and send subscribe frame.
                let filter = build_logs_filter(&cfg);
                let sub = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "eth_subscribe",
                    "params": ["logs", filter],
                });
                if let Err(e) = ws.send(Message::Text(sub.to_string().into())).await {
                    log::warn!("blockchain-logs: subscribe failed: {e}");
                    counters.reconnects.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => {
                            let _ = ws.close(None).await;
                            return;
                        }
                        msg = ws.next() => match msg {
                            Some(Ok(Message::Text(t))) => {
                                if let Some(ev) = decode_log_notification(t.as_bytes()) {
                                    try_push(&mut sink, &counters, ev);
                                }
                            }
                            Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                log::warn!("blockchain-logs: frame error: {e}");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
            Err(e) => log::warn!("blockchain-logs: connect failed: {e}"),
        }
        counters.reconnects.fetch_add(1, Ordering::Relaxed);
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.cancelled() => return,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn build_logs_filter(cfg: &BlockchainLogsConfig) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    if cfg.addresses.len() == 1 {
        obj.insert("address".into(), serde_json::Value::String(cfg.addresses[0].clone()));
    } else if !cfg.addresses.is_empty() {
        obj.insert(
            "address".into(),
            serde_json::Value::Array(
                cfg.addresses
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !cfg.topics.is_empty() {
        obj.insert("topics".into(), serde_json::Value::Array(cfg.topics.clone()));
    }
    serde_json::Value::Object(obj)
}

/// Decode an `eth_subscription` notification payload into a `RawEvent`.
/// Returns `None` for subscription acks, non-log messages, or malformed
/// payloads.
pub fn decode_log_notification(bytes: &[u8]) -> Option<RawEvent> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let params = v.get("params")?;
    let result = params.get("result")?;
    let tx_hash_str = result.get("transactionHash")?.as_str()?;
    let tx_hash = parse_hex_bytes::<32>(tx_hash_str)?;
    let log_index = result
        .get("logIndex")
        .and_then(|v| v.as_str())
        .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(u32::MAX);
    // Best-effort: the CTF `OrderFilled` layout places the maker address as
    // the first indexed parameter (`topics[1]`). If present and zero-padded
    // to 32 bytes, extract the low 20 bytes as the maker wallet. Non-address
    // topics (e.g. raw hashes) won't have the zero-prefix, so we leave the
    // field `None` rather than mis-classify them. Downstream flow signals
    // treat `None` as "unknown cohort".
    let maker_wallet = result
        .get("topics")
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.get(1))
        .and_then(|t| t.as_str())
        .and_then(topic_as_address);
    Some(RawEvent {
        event_id: EventId::fetch_next(),
        source: SourceKind::BlockchainLogs,
        source_seq: u64::MAX,
        anchor: Some(OnChainAnchor { tx_hash, log_index }),
        token_id: String::new(),
        market_id: None,
        side: None,
        price: None,
        size: None,
        tsc_in: Timestamp::now(),
        wall_ns: wall_clock_ns(),
        extra: Some(bytes.to_vec().into_boxed_slice()),
        observe_only: false,
        maker_wallet,
    })
}

/// Parse a 32-byte hex topic as an EVM address. Returns `Some` only when the
/// topic is zero-padded on the high 12 bytes, which is the canonical ABI
/// encoding for an `address` indexed parameter.
fn topic_as_address(topic: &str) -> Option<[u8; 20]> {
    let raw = parse_hex_bytes::<32>(topic)?;
    if raw[..12].iter().any(|b| *b != 0) {
        return None;
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&raw[12..]);
    Some(out)
}

fn parse_hex_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for i in 0..N {
        out[i] = u8::from_str_radix(&stripped[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn decodes_log_notification() {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let tx = "0x".to_string() + &"ab".repeat(32);
        let payload = format!(
            r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x1","result":{{"address":"0xabc","transactionHash":"{tx}","logIndex":"0x5","topics":["0xdead"],"data":"0x"}}}}}}"#
        );
        let ev = decode_log_notification(payload.as_bytes()).expect("decoded");
        assert_eq!(ev.source, SourceKind::BlockchainLogs);
        let a = ev.anchor.unwrap();
        assert_eq!(a.tx_hash[0], 0xab);
        assert_eq!(a.log_index, 5);
        assert!(ev.extra.is_some());
        assert!(ev.maker_wallet.is_none(), "no indexed address topic");
    }

    #[test]
    fn decodes_log_extracts_maker_from_topic1() {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let tx = "0x".to_string() + &"ab".repeat(32);
        // topic[0] = event sig hash (unused); topic[1] = address-padded maker.
        let sig = "0x".to_string() + &"11".repeat(32);
        let maker_padded =
            "0x000000000000000000000000".to_string() + &"cd".repeat(20);
        let payload = format!(
            r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x1","result":{{"transactionHash":"{tx}","logIndex":"0x5","topics":["{sig}","{maker_padded}"],"data":"0x"}}}}}}"#
        );
        let ev = decode_log_notification(payload.as_bytes()).expect("decoded");
        let maker = ev.maker_wallet.expect("maker extracted");
        assert_eq!(maker, [0xcd; 20]);
    }

    #[test]
    fn decodes_log_skips_non_address_topic1() {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let tx = "0x".to_string() + &"ab".repeat(32);
        // topic[1] is 32 random bytes (high 12 not zero) → not an address.
        let sig = "0x".to_string() + &"11".repeat(32);
        let rnd = "0x".to_string() + &"ef".repeat(32);
        let payload = format!(
            r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x1","result":{{"transactionHash":"{tx}","logIndex":"0x5","topics":["{sig}","{rnd}"],"data":"0x"}}}}}}"#
        );
        let ev = decode_log_notification(payload.as_bytes()).expect("decoded");
        assert!(ev.maker_wallet.is_none());
    }

    #[test]
    fn decodes_ack_as_none() {
        let ack = r#"{"jsonrpc":"2.0","id":1,"result":"0xsubid"}"#;
        assert!(decode_log_notification(ack.as_bytes()).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mock_ws_emits_log_event() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("ws://{addr}/");
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            // Consume the subscribe frame.
            let _ = ws.next().await;
            // Ack.
            let _ = ws
                .send(Message::Text(
                    r#"{"jsonrpc":"2.0","id":1,"result":"0xsubid"}"#.into(),
                ))
                .await;
            // One log.
            let tx = "0x".to_string() + &"cd".repeat(32);
            let notif = format!(
                r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0xsubid","result":{{"transactionHash":"{tx}","logIndex":"0x2","topics":[],"data":"0x"}}}}}}"#
            );
            let _ = ws.send(Message::Text(notif.into())).await;
            let _ = ws.close(None).await;
        });

        let (prod, mut cons) = blink_rings::bounded::<RawEvent>(16);
        let src = BlockchainLogsSource::new(BlockchainLogsConfig {
            url,
            addresses: vec!["0xctf".into()],
            topics: vec![],
        });
        let shutdown = ShutdownToken::new();
        let shutdown2 = shutdown.clone();
        let handle = std::thread::spawn(move || {
            Box::new(src).run(prod, shutdown2);
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = None;
        while got.is_none() && std::time::Instant::now() < deadline {
            if let Some(ev) = cons.pop() {
                got = Some(ev);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        shutdown.cancel();
        let _ = server.await;
        let _ = handle.join();

        let ev = got.expect("log event");
        assert_eq!(ev.source, SourceKind::BlockchainLogs);
        assert_eq!(ev.anchor.unwrap().log_index, 2);
    }
}
