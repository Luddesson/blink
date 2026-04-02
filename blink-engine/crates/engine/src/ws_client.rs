//! Persistent WebSocket client with exponential-backoff reconnection.
//!
//! Connects to the Polymarket CLOB WebSocket feed, subscribes to configured
//! markets, and routes incoming events to the order book and sniffer.
//!
//! # Reconnection strategy
//! Initial backoff: 1 s, doubling on each failure up to 30 s ceiling.
//! After 3+ consecutive failures, a 45 s cooldown prevents Cloudflare
//! rate-limiting.  A session lasting ≥15 s resets both backoff and counter.
//!
//! # Known limitation
//! `tokio-tungstenite` + `native-tls` connections through Cloudflare are
//! intermittently RST'd (os error 10054) after 2-60 s, especially with
//! multi-market subscriptions.  TCP_NODELAY **must** be disabled (Nagle kept)
//! to avoid near-instant RSTs.  The system is designed to be resilient to
//! these drops — RN1 poller is the primary data source.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, instrument, warn};

use crate::{
    activity_log::{ActivityLog, EntryKind, push as log_push},
    config::Config,
    order_book::OrderBookStore,
    sniffer::Sniffer,
    tick_recorder::{TickRecord, now_ms},
    types::{MarketEvent, RN1Signal},
};

/// Polymarket expects application-level `"PING"` text messages (uppercase, NOT
/// protocol-level WebSocket pings).  Server replies with `"PONG"` text.
/// Docs: "Send PING every 10 seconds" — matching exactly.
const PING_INTERVAL:       Duration = Duration::from_secs(10);
/// Conservative initial backoff prevents Cloudflare rate-limiting after an RST.
const INITIAL_BACKOFF:     Duration = Duration::from_millis(1_000);
const MAX_BACKOFF:         Duration = Duration::from_secs(30);
/// If we receive no data at all (neither market events nor pong replies) for
/// this long, the connection is considered dead and we force a reconnect.
const PONG_TIMEOUT:        Duration = Duration::from_secs(45);
const CONNECT_TIMEOUT:     Duration = Duration::from_secs(10);
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(15);
/// After this many consecutive failures, add a long cooldown to avoid being
/// blacklisted by Cloudflare.
const CONSECUTIVE_FAIL_COOLDOWN_THRESHOLD: u32 = 3;
const CONSECUTIVE_FAIL_COOLDOWN:           Duration = Duration::from_secs(45);

#[derive(Default)]
pub struct WsHealthMetrics {
    pub ping_sent: AtomicU64,
    pub pong_recv: AtomicU64,
    pub reconnect_attempts: AtomicU64,
    pub last_pong_unix_ms: AtomicU64,
}

#[derive(Default)]
struct MessageParseCounters {
    parsed:       u64,
    unknown:      u64,
    parse_failed: u64,
}

impl MessageParseCounters {
    fn log_summary(&self, reason: &str) {
        info!(
            reason,
            parsed = self.parsed,
            unknown = self.unknown,
            parse_failed = self.parse_failed,
            "WS parser session summary"
        );
    }
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Adds ±25% jitter to a duration to avoid thundering-herd reconnects.
fn jittered(d: Duration) -> Duration {
    let millis = d.as_millis() as u64;
    if millis == 0 { return d; }
    let jitter_range = (millis / 4).max(1);
    let offset = rand::thread_rng().gen_range(0..=jitter_range * 2) as i64 - jitter_range as i64;
    Duration::from_millis((millis as i64 + offset).max(10) as u64)
}

/// Runs the WebSocket client loop indefinitely with automatic reconnection.
///
/// - Connects to [`Config::ws_url`].
/// - Sends a `subscribe` message for all configured markets.
/// - Forwards parsed events to the order-book store and the sniffer.
/// - Emits detected RN1 signals via `signal_tx`.
/// - Optionally records every order event to ClickHouse via `tick_tx`.
/// - Never returns under normal operation; propagates only unrecoverable errors.
pub async fn run_ws(
    config:               Arc<Config>,
    book_store:           Arc<OrderBookStore>,
    signal_tx:            crossbeam_channel::Sender<RN1Signal>,
    ws_live:              Arc<AtomicBool>,
    activity:             Option<ActivityLog>,
    msg_count:            Arc<AtomicU64>,
    tick_tx:              Option<crossbeam_channel::Sender<TickRecord>>,
    market_subscriptions: Arc<Mutex<Vec<String>>>,
    force_reconnect:      Arc<AtomicBool>,
    health_metrics:       Option<Arc<WsHealthMetrics>>,
) -> Result<()> {
    let sniffer = Sniffer::new(&config.rn1_wallet);
    let mut backoff = INITIAL_BACKOFF;
    let mut consecutive_failures: u32 = 0;

    loop {
        info!(url = %config.ws_url, consecutive_failures, "Connecting to WebSocket feed");
        if let Some(ref hm) = health_metrics {
            hm.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
        }

        // After repeated failures, add a long cooldown before retrying to avoid
        // Cloudflare rate-limiting / IP blacklisting.
        if consecutive_failures >= CONSECUTIVE_FAIL_COOLDOWN_THRESHOLD {
            let cooldown = jittered(CONSECUTIVE_FAIL_COOLDOWN);
            warn!(
                consecutive_failures,
                cooldown_secs = cooldown.as_secs_f32(),
                "Too many consecutive failures — cooling down before retry"
            );
            if let Some(ref log) = activity {
                log_push(log, EntryKind::Warn,
                    format!("WS cooldown {:.0}s after {} failures", cooldown.as_secs_f32(), consecutive_failures));
            }
            tokio::time::sleep(cooldown).await;
        }

        let session_start = Instant::now();

        match connect_and_run(
            &config,
            &book_store,
            &sniffer,
            &signal_tx,
            &ws_live,
            &activity,
            &msg_count,
            &tick_tx,
            &market_subscriptions,
            &force_reconnect,
            health_metrics.as_ref(),
        ).await {
            Ok(()) => {
                ws_live.store(false, Ordering::Relaxed);
                // Even after a clean close, pause briefly so Cloudflare doesn't
                // see us as a connection-churning bot.
                let pause = jittered(Duration::from_secs(5));
                info!(pause_ms = pause.as_millis(), "WebSocket closed cleanly — reconnecting after pause");
                if let Some(ref log) = activity {
                    log_push(log, EntryKind::Warn, "WS closed cleanly — reconnecting");
                }
                tokio::time::sleep(pause).await;
                backoff = INITIAL_BACKOFF;
                consecutive_failures = 0;
            }
            Err(err) => {
                ws_live.store(false, Ordering::Relaxed);
                let session_secs = session_start.elapsed().as_secs();

                // Reset backoff if we had a sustained successful connection,
                // but still pause to avoid rapid reconnection patterns that
                // trigger Cloudflare rate-limiting.
                if session_start.elapsed() >= BACKOFF_RESET_AFTER {
                    let pause = jittered(Duration::from_secs(5));
                    info!(
                        session_secs,
                        pause_ms = pause.as_millis(),
                        "Session was healthy — pausing before reconnect"
                    );
                    tokio::time::sleep(pause).await;
                    backoff = INITIAL_BACKOFF;
                    consecutive_failures = 0;
                } else {
                    consecutive_failures += 1;
                }

                let sleep_dur = jittered(backoff);
                error!(
                    error      = %err,
                    backoff_ms = sleep_dur.as_millis(),
                    "WebSocket error — backing off before reconnect"
                );
                if let Some(ref log) = activity {
                    log_push(log, EntryKind::Warn, format!("WS error — retry in {}ms", sleep_dur.as_millis()));
                }
                tokio::time::sleep(sleep_dur).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

// ─── Connection lifecycle ─────────────────────────────────────────────────────

/// Establishes a single WebSocket connection and pumps messages until the
/// stream ends or an error occurs.
#[instrument(skip_all, fields(ws_url = %config.ws_url))]
async fn connect_and_run(
    config:               &Config,
    book_store:           &Arc<OrderBookStore>,
    sniffer:              &Sniffer,
    signal_tx:            &crossbeam_channel::Sender<RN1Signal>,
    ws_live:              &Arc<AtomicBool>,
    activity:             &Option<ActivityLog>,
    msg_count:            &Arc<AtomicU64>,
    tick_tx:              &Option<crossbeam_channel::Sender<TickRecord>>,
    market_subscriptions: &Arc<Mutex<Vec<String>>>,
    force_reconnect:      &Arc<AtomicBool>,
    health_metrics:       Option<&Arc<WsHealthMetrics>>,
) -> Result<()> {
    // NOTE: connect_async keeps Nagle's algorithm (no TCP_NODELAY).  This is
    // intentional — TCP_NODELAY causes Cloudflare to RST the connection after
    // ~2-5s because the burst of tiny TCP segments looks suspicious.
    let ws_stream = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        connect_async(config.ws_url.as_str()),
    ).await {
        Ok(Ok((stream, _response))) => stream,
        Ok(Err(e)) => return Err(anyhow::anyhow!("WebSocket handshake failed: {e}")),
        Err(_) => return Err(anyhow::anyhow!("WebSocket handshake timed out after {}s", CONNECT_TIMEOUT.as_secs())),
    };

    ws_live.store(true, Ordering::Relaxed);
    info!("WebSocket handshake complete");
    let markets = {
        let subs = market_subscriptions.lock().unwrap();
        if subs.is_empty() {
            config.markets.clone()
        } else {
            subs.clone()
        }
    };

    if let Some(ref log) = activity {
        log_push(log, EntryKind::Engine,
            format!("WS connected to {}  subscribed to {} markets", config.ws_url, markets.len()));
    }

    let (mut write, mut read) = ws_stream.split();

    // Correct Polymarket CLOB WS format: type="market", assets_ids=[...]
    // No auth required for the public market channel.
    // initial_dump=false avoids a large burst of 20+ messages that can trigger
    // Cloudflare throttling on the connection.
    let sub_payload = serde_json::json!({
        "type":       "market",
        "assets_ids": markets,
    });
    write
        .send(Message::Text(sub_payload.to_string().into()))
        .await
        .map_err(|e| anyhow::anyhow!("failed to send subscription: {e}"))?;
    info!("Subscribed to markets");

    // Send an immediate PING to establish keepalive pattern right away.
    // Without this, connections die within 2s after the initial dump burst.
    // If it fails, we continue anyway — the ping_ticker will try again.
    match write.send(Message::Text("PING".into())).await {
        Ok(()) => {
            if let Some(hm) = health_metrics {
                hm.ping_sent.fetch_add(1, Ordering::Relaxed);
            }
        }
        Err(e) => {
            warn!(error = %e, "Initial PING failed — continuing anyway");
        }
    }

    // ── Message / ping loop ───────────────────────────────────────────────
    let mut ping_ticker = tokio::time::interval(PING_INTERVAL);
    ping_ticker.tick().await; // consume the immediate first tick
    let mut reconnect_ticker = tokio::time::interval(Duration::from_millis(250));
    reconnect_ticker.tick().await;
    let reconnect_debounce = Duration::from_millis(config.ws_reconnect_debounce_ms.max(250));
    let connected_at = Instant::now();
    let mut parse_counters = MessageParseCounters::default();
    let mut last_data_at = Instant::now(); // pong watchdog: track last sign of life

    loop {
        tokio::select! {
            biased;

            // Incoming message
            frame = read.next() => {
                match frame {
                    None => {
                        parse_counters.log_summary("stream-ended");
                        info!("WebSocket stream ended (server closed connection)");
                        return Ok(());
                    }
                    Some(Err(err)) => {
                        parse_counters.log_summary("stream-error");
                        return Err(anyhow::anyhow!("WebSocket read error: {err}"));
                    }
                    Some(Ok(msg)) => {
                        last_data_at = Instant::now();
                        handle_message(
                            msg,
                            book_store,
                            sniffer,
                            signal_tx,
                            msg_count,
                            tick_tx,
                            &mut parse_counters,
                            config.ws_parse_error_preview_chars,
                            health_metrics,
                        );
                    }
                }
            }

            // Heartbeat: send application-level "ping" text (Polymarket protocol)
            _ = ping_ticker.tick() => {
                // Check if we've received any data (messages or pongs) recently
                if last_data_at.elapsed() > PONG_TIMEOUT {
                    warn!(
                        silent_secs = last_data_at.elapsed().as_secs(),
                        "Pong watchdog: no data received — forcing reconnect"
                    );
                    parse_counters.log_summary("pong-timeout");
                    return Err(anyhow::anyhow!("pong watchdog timeout: {}s without data", last_data_at.elapsed().as_secs()));
                }

                debug!("Sending application-level PING");
                if let Err(err) = write.send(Message::Text("PING".into())).await {
                    return Err(anyhow::anyhow!("failed to send ping: {err}"));
                }
                if let Some(hm) = health_metrics {
                    hm.ping_sent.fetch_add(1, Ordering::Relaxed);
                }
            }

            // Runtime subscription changes: re-subscribe on the EXISTING
            // connection instead of tearing down and reconnecting.  This avoids
            // the TCP RST cascade that happens when we rapidly reconnect through
            // Cloudflare.
            _ = reconnect_ticker.tick() => {
                if force_reconnect.load(Ordering::Relaxed) {
                    if connected_at.elapsed() >= reconnect_debounce {
                        force_reconnect.store(false, Ordering::Relaxed);

                        let updated_markets = {
                            let subs = market_subscriptions.lock().unwrap();
                            subs.clone()
                        };

                        if !updated_markets.is_empty() {
                            // Use dynamic subscribe with "operation" field (avoids
                            // full reconnect per Polymarket WS docs).
                            let sub_payload = serde_json::json!({
                                "type":       "market",
                                "assets_ids": updated_markets,
                                "operation":  "subscribe",
                            });
                            match write.send(Message::Text(sub_payload.to_string().into())).await {
                                Ok(()) => {
                                    info!(
                                        markets = updated_markets.len(),
                                        "Re-subscribed to updated market list on existing connection"
                                    );
                                    if let Some(ref log) = activity {
                                        log_push(log, EntryKind::Engine,
                                            format!("WS re-subscribed to {} markets (inline)", updated_markets.len()));
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to send re-subscription — will reconnect");
                                    parse_counters.log_summary("resub-failed");
                                    return Err(anyhow::anyhow!("failed to re-subscribe: {e}"));
                                }
                            }
                        }
                    } else {
                        debug!(
                            debounce_ms  = reconnect_debounce.as_millis(),
                            elapsed_ms   = connected_at.elapsed().as_millis(),
                            "Reconnect pending — waiting for debounce window"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_book::OrderBookStore;
    use crate::types::OrderSide;

    #[test]
    fn parses_single_object_payload() {
        let payload = r#"{
            "event_type":"price_change",
            "market":"0xmarket",
            "price_changes":[
                {"asset_id":"asset-1","price":"0.500","size":"10","side":"BUY"}
            ]
        }"#;
        let store = Arc::new(OrderBookStore::new());
        let sniffer = Sniffer::new("0xdeadbeef");
        let (signal_tx, _rx) = crossbeam_channel::bounded(16);
        let msg_count = Arc::new(AtomicU64::new(0));
        let mut counters = MessageParseCounters::default();

        handle_message(
            Message::Text(payload.into()),
            &store,
            &sniffer,
            &signal_tx,
            &msg_count,
            &None,
            &mut counters,
            120,
            None,
        );

        let best_bid = store.top_of_book("asset-1", OrderSide::Sell).map(|(p, _)| p);
        assert_eq!(best_bid, Some(500));
        assert_eq!(counters.parsed, 1);
        assert_eq!(counters.parse_failed, 0);
    }

    #[test]
    fn parses_array_payload_batch() {
        let payload = r#"[
            {
                "event_type":"price_change",
                "market":"0xmarket",
                "price_changes":[
                    {"asset_id":"asset-2","price":"0.420","size":"7","side":"BUY"}
                ]
            },
            {
                "event_type":"price_change",
                "market":"0xmarket",
                "price_changes":[
                    {"asset_id":"asset-2","price":"0.580","size":"9","side":"SELL"}
                ]
            }
        ]"#;
        let store = Arc::new(OrderBookStore::new());
        let sniffer = Sniffer::new("0xdeadbeef");
        let (signal_tx, _rx) = crossbeam_channel::bounded(16);
        let msg_count = Arc::new(AtomicU64::new(0));
        let mut counters = MessageParseCounters::default();

        handle_message(
            Message::Text(payload.into()),
            &store,
            &sniffer,
            &signal_tx,
            &msg_count,
            &None,
            &mut counters,
            120,
            None,
        );

        let best_bid = store.top_of_book("asset-2", OrderSide::Sell).map(|(p, _)| p);
        let best_ask = store.top_of_book("asset-2", OrderSide::Buy).map(|(p, _)| p);
        assert_eq!(best_bid, Some(420));
        assert_eq!(best_ask, Some(580));
        assert_eq!(counters.parsed, 2);
        assert_eq!(counters.parse_failed, 0);
    }
}

// ─── Message dispatcher ───────────────────────────────────────────────────────

/// Parses and routes a single WebSocket frame.
///
/// Unknown `event_type` values are silently ignored (logged at `debug`).
/// Order events are forwarded to `tick_tx` when present (ClickHouse recording).
fn handle_message(
    msg:        Message,
    book_store: &Arc<OrderBookStore>,
    sniffer:    &Sniffer,
    signal_tx:  &crossbeam_channel::Sender<RN1Signal>,
    msg_count:  &Arc<AtomicU64>,
    tick_tx:    &Option<crossbeam_channel::Sender<TickRecord>>,
    parse_counters: &mut MessageParseCounters,
    parse_error_preview_chars: usize,
    health_metrics: Option<&Arc<WsHealthMetrics>>,
) {
    match msg {
        Message::Text(text) => {
            msg_count.fetch_add(1, Ordering::Relaxed);

            // Handle Polymarket keep-alive responses before attempting JSON parse.
            // The server replies "PONG" (uppercase) to our "PING" text messages.
            {
                let trimmed = text.trim();
                if trimmed == "PONG" {
                    debug!("Received application-level PONG");
                    if let Some(hm) = health_metrics {
                        hm.pong_recv.fetch_add(1, Ordering::Relaxed);
                        hm.last_pong_unix_ms.store(now_ms(), Ordering::Relaxed);
                    }
                    return;
                }
                if trimmed == r#"{"heartbeat":{}}"# || trimmed.starts_with(r#"{"heartbeat""#) {
                    debug!("Received server heartbeat");
                    return;
                }
            }

            // simd-json requires a mutable byte buffer (modifies in-place).
            let mut bytes = text.as_bytes().to_vec();
            match simd_json::from_slice::<MarketEvent>(&mut bytes) {
                Ok(event) => {
                    parse_counters.parsed += 1;
                    // Forward order events to ClickHouse recorder if active.
                    if let (Some(tx), MarketEvent::Order(ref ev)) = (tick_tx, &event) {
                        let tick = TickRecord {
                            timestamp_ms: now_ms(),
                            token_id:     ev.market.clone(),
                            side:         ev.side.to_string(),
                            price:        crate::types::parse_price(&ev.price),
                            size:         crate::types::parse_price(&ev.original_size),
                            wallet:       ev.owner.clone(),
                        };
                        if let Err(e) = tx.try_send(tick) {
                            debug!(error = %e, "tick channel full — tick dropped");
                        }
                    }
                    // Unknown variants are valid (parsed successfully via #[serde(other)])
                    // but carry no data — skip further processing.
                    if matches!(event, MarketEvent::Unknown) {
                        parse_counters.unknown += 1;
                        return;
                    }
                    book_store.apply_update(&event);
                    if let Some(signal) = sniffer.check_order_event(&event) {
                        if let Err(err) = signal_tx.try_send(signal) {
                            warn!(error = %err, "RN1 signal channel full — signal dropped");
                        }
                    }
                }
                Err(err) => {
                    // Try array payloads: Polymarket sometimes sends batches.
                    let mut arr_bytes = text.as_bytes().to_vec();
                    match simd_json::from_slice::<Vec<MarketEvent>>(&mut arr_bytes) {
                        Ok(events) => {
                            for event in events {
                                parse_counters.parsed += 1;
                                if matches!(event, MarketEvent::Unknown) {
                                    parse_counters.unknown += 1;
                                    continue;
                                }
                                if let (Some(tx), MarketEvent::Order(ref ev)) = (tick_tx, &event) {
                                    let tick = TickRecord {
                                        timestamp_ms: now_ms(),
                                        token_id:     ev.market.clone(),
                                        side:         ev.side.to_string(),
                                        price:        crate::types::parse_price(&ev.price),
                                        size:         crate::types::parse_price(&ev.original_size),
                                        wallet:       ev.owner.clone(),
                                    };
                                    if let Err(e) = tx.try_send(tick) {
                                        debug!(error = %e, "tick channel full — tick dropped");
                                    }
                                }
                                book_store.apply_update(&event);
                                if let Some(signal) = sniffer.check_order_event(&event) {
                                    if let Err(send_err) = signal_tx.try_send(signal) {
                                        warn!(error = %send_err, "RN1 signal channel full — signal dropped");
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            parse_counters.parse_failed += 1;
                            let preview: &str = if text.len() > parse_error_preview_chars {
                                &text[..parse_error_preview_chars]
                            } else {
                                &text
                            };
                            debug!(error = %err, preview, "ignoring unrecognised WS payload");
                        }
                    }
                }
            }
        }
        Message::Ping(_) => {
            // tokio-tungstenite auto-replies with Pong; nothing to do here.
            debug!("Received ping from server");
        }
        Message::Pong(_) => {
            debug!("Received pong (heartbeat ACK)");
            if let Some(hm) = health_metrics {
                hm.pong_recv.fetch_add(1, Ordering::Relaxed);
                hm.last_pong_unix_ms.store(now_ms(), Ordering::Relaxed);
            }
        }
        Message::Close(frame) => {
            info!(frame = ?frame, "Received WebSocket close frame");
        }
        Message::Binary(_) | Message::Frame(_) => {
            debug!("Ignoring binary/raw frame");
        }
    }
}
