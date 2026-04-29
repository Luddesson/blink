//! R-8: Polygon mempool / inclusion-latency probe.
//!
//! Connects to a Polygon JSON-RPC WebSocket (`POLYGON_WS_URL`), subscribes
//! to `newPendingTransactions` (hash-only), then for each hash fires
//! `eth_getTransactionByHash` and polls `eth_getTransactionReceipt` to
//! measure pending→included delay. If `POLYGON_WS_URL_2` is set, a second
//! feed runs in parallel and we compute coverage overlap.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
#[cfg(test)]
use anyhow::anyhow;
use clap::Args as ClapArgs;
use futures_util::{SinkExt, StreamExt};
use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::report::{print_json, quantiles_ms, Quantiles};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// How long to observe (seconds).
    #[arg(long, default_value_t = 60)]
    pub duration_secs: u64,
    /// CTF contract address to match on `to` (optional; case-insensitive).
    #[arg(long)]
    pub ctf: Option<String>,
    /// Max seconds to wait for inclusion before giving up on a tx.
    #[arg(long, default_value_t = 10)]
    pub inclusion_timeout_secs: u64,
    /// Coverage overlap window (seconds); used only when POLYGON_WS_URL_2 set.
    #[arg(long, default_value_t = 300)]
    pub overlap_window_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub duration_secs: f64,
    pub ws_primary: String,
    pub ws_secondary: Option<String>,
    pub total_pending: u64,
    pub pending_per_sec: f64,
    pub ctf_match_count: u64,
    pub inclusion: Quantiles,
    /// Coverage stats (only populated if secondary feed provided).
    pub coverage: Option<Coverage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Coverage {
    pub overlap_window_secs: u64,
    pub seen_primary: u64,
    pub seen_secondary: u64,
    pub only_primary: u64,
    pub only_secondary: u64,
    pub in_both: u64,
}

pub async fn run(a: Args) -> anyhow::Result<()> {
    let ws1 = std::env::var("POLYGON_WS_URL")
        .context("POLYGON_WS_URL env var required (Polygon JSON-RPC WebSocket endpoint)")?;
    let ws2 = std::env::var("POLYGON_WS_URL_2").ok();
    let ctf = a.ctf.as_ref().map(|s| s.to_ascii_lowercase());

    let (hash_tx, mut hash_rx) = mpsc::unbounded_channel::<(usize, String, Instant)>();
    let stop = Arc::new(tokio::sync::Notify::new());
    let primary = spawn_feed(0, ws1.clone(), hash_tx.clone(), stop.clone());
    let secondary = if let Some(u) = ws2.clone() {
        Some(spawn_feed(1, u, hash_tx.clone(), stop.clone()))
    } else {
        None
    };
    drop(hash_tx);

    let seen_primary: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    let seen_secondary: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    let inclusion: Arc<Mutex<Histogram<u64>>> = Arc::new(Mutex::new(Histogram::<u64>::new(3)?));
    let ctf_count: Arc<Mutex<u64>> = Arc::new(Mutex::new(0u64));
    let total_pending: Arc<Mutex<u64>> = Arc::new(Mutex::new(0u64));

    let start = Instant::now();
    let run_for = Duration::from_secs(a.duration_secs);

    let collector = {
        let seen_p = seen_primary.clone();
        let seen_s = seen_secondary.clone();
        let inclusion = inclusion.clone();
        let ctf_count = ctf_count.clone();
        let total_pending = total_pending.clone();
        let ctf = ctf.clone();
        let ws1 = ws1.clone();
        let inclusion_timeout = Duration::from_secs(a.inclusion_timeout_secs);
        tokio::spawn(async move {
            while let Some((feed, hash, ts)) = hash_rx.recv().await {
                {
                    let mut tp = total_pending.lock().await;
                    *tp += 1;
                }
                match feed {
                    0 => {
                        seen_p.lock().await.insert(hash.clone(), ts);
                    }
                    _ => {
                        seen_s.lock().await.insert(hash.clone(), ts);
                    }
                }
                // Only chase inclusion on primary (avoid double work).
                if feed == 0 {
                    let ws = ws1.clone();
                    let ctf = ctf.clone();
                    let inclusion = inclusion.clone();
                    let ctf_count = ctf_count.clone();
                    tokio::spawn(async move {
                        if let Some(dt) = chase_inclusion(&ws, &hash, ts, inclusion_timeout, ctf.as_deref(), &ctf_count)
                            .await
                            .ok()
                            .flatten()
                        {
                            let _ = inclusion.lock().await.record(dt.as_micros() as u64);
                        }
                    });
                }
            }
        })
    };

    tokio::time::sleep(run_for).await;
    stop.notify_waiters();
    // Give feeds a moment to flush then close channels.
    tokio::time::sleep(Duration::from_millis(250)).await;
    primary.abort();
    if let Some(h) = secondary {
        h.abort();
    }
    collector.abort();
    let elapsed = start.elapsed();

    let total = *total_pending.lock().await;
    let ctf_match = *ctf_count.lock().await;
    let h = inclusion.lock().await;
    let incl_q = quantiles_ms(&*h);
    drop(h);

    let coverage = if ws2.is_some() {
        let p = seen_primary.lock().await;
        let s = seen_secondary.lock().await;
        let window = Duration::from_secs(a.overlap_window_secs);
        let now = Instant::now();
        let p_in: HashSet<String> = p
            .iter()
            .filter(|(_, t)| now.duration_since(**t) <= window)
            .map(|(k, _)| k.clone())
            .collect();
        let s_in: HashSet<String> = s
            .iter()
            .filter(|(_, t)| now.duration_since(**t) <= window)
            .map(|(k, _)| k.clone())
            .collect();
        let in_both = p_in.intersection(&s_in).count() as u64;
        Some(Coverage {
            overlap_window_secs: a.overlap_window_secs,
            seen_primary: p_in.len() as u64,
            seen_secondary: s_in.len() as u64,
            only_primary: p_in.difference(&s_in).count() as u64,
            only_secondary: s_in.difference(&p_in).count() as u64,
            in_both,
        })
    } else {
        None
    };

    let report = Report {
        duration_secs: elapsed.as_secs_f64(),
        ws_primary: ws1,
        ws_secondary: ws2,
        total_pending: total,
        pending_per_sec: total as f64 / elapsed.as_secs_f64().max(1e-9),
        ctf_match_count: ctf_match,
        inclusion: incl_q,
        coverage,
    };
    print_json(&report)
}

fn spawn_feed(
    feed: usize,
    url: String,
    tx: mpsc::UnboundedSender<(usize, String, Instant)>,
    stop: Arc<tokio::sync::Notify>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run_feed(feed, &url, tx, stop).await {
            tracing::warn!(feed, %e, "feed ended with error");
        }
    })
}

async fn run_feed(
    feed: usize,
    url: &str,
    tx: mpsc::UnboundedSender<(usize, String, Instant)>,
    stop: Arc<tokio::sync::Notify>,
) -> anyhow::Result<()> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .with_context(|| format!("ws connect {}", url))?;
    let sub = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_subscribe",
        "params": ["newPendingTransactions"]
    });
    ws.send(Message::Text(sub.to_string().into())).await?;

    loop {
        tokio::select! {
            _ = stop.notified() => break,
            msg = ws.next() => {
                let Some(msg) = msg else { break };
                let msg = match msg { Ok(m) => m, Err(e) => { tracing::warn!(%e, "ws recv"); break; } };
                let text = match &msg {
                    Message::Text(t) => t.to_string(),
                    Message::Binary(b) => String::from_utf8_lossy(b).into_owned(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(hash) = v.pointer("/params/result").and_then(|h| h.as_str()) {
                        let _ = tx.send((feed, hash.to_string(), Instant::now()));
                    }
                }
            }
        }
    }
    Ok(())
}

async fn chase_inclusion(
    ws_url: &str,
    hash: &str,
    seen_at: Instant,
    timeout: Duration,
    ctf: Option<&str>,
    ctf_count: &Arc<Mutex<u64>>,
) -> anyhow::Result<Option<Duration>> {
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await?;

    // eth_getTransactionByHash — once, to inspect `to`.
    let req = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"eth_getTransactionByHash","params":[hash]
    });
    ws.send(Message::Text(req.to_string().into())).await?;
    if let Some(Ok(Message::Text(t))) = ws.next().await {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
            if let Some(to) = v.pointer("/result/to").and_then(|h| h.as_str()) {
                if let Some(target) = ctf {
                    if to.eq_ignore_ascii_case(target) {
                        let mut c = ctf_count.lock().await;
                        *c += 1;
                        tracing::info!(%hash, %to, "ctf-match tx detail: {}", v);
                    }
                }
            }
        }
    }

    // Poll receipt until timeout.
    let deadline = seen_at + timeout;
    let mut id = 2u64;
    while Instant::now() < deadline {
        let req = serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"eth_getTransactionReceipt","params":[hash]
        });
        id += 1;
        if ws.send(Message::Text(req.to_string().into())).await.is_err() {
            break;
        }
        if let Some(Ok(Message::Text(t))) = ws.next().await {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if v.get("result").and_then(|r| r.as_object()).is_some() {
                    return Ok(Some(Instant::now().duration_since(seen_at)));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_roundtrip() {
        let r = Report {
            duration_secs: 60.0,
            ws_primary: "wss://a".into(),
            ws_secondary: Some("wss://b".into()),
            total_pending: 1234,
            pending_per_sec: 20.5,
            ctf_match_count: 3,
            inclusion: Quantiles::default(),
            coverage: Some(Coverage {
                overlap_window_secs: 300,
                seen_primary: 100,
                seen_secondary: 98,
                only_primary: 5,
                only_secondary: 3,
                in_both: 95,
            }),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&s).unwrap();
        assert_eq!(back.total_pending, 1234);
        assert_eq!(back.coverage.unwrap().in_both, 95);
    }

    #[test]
    fn missing_env_is_error() {
        // Calling run without POLYGON_WS_URL should fail fast. We verify the
        // error surface here (cannot run the full probe in CI).
        std::env::remove_var("POLYGON_WS_URL");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(run(Args {
                duration_secs: 1,
                ctf: None,
                inclusion_timeout_secs: 1,
                overlap_window_secs: 1,
            }))
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("POLYGON_WS_URL"), "got: {msg}");
    }

    // Suppress unused warning for anyhow when tests don't exercise it.
    #[allow(dead_code)]
    fn _use_anyhow() -> anyhow::Result<()> {
        Err(anyhow!("x"))
    }
}
