//! Lightweight JSON-RPC control/status server for agent orchestration.
//!
//! Exposes a local HTTP endpoint (`POST /rpc`) with JSON-RPC 2.0 methods:
//! - `blink_status`
//! - `paper_summary`
//! - `set_pause`

use std::io;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::paper_engine::PaperEngine;

#[derive(Clone)]
pub struct AgentRpcState {
    pub ws_live: Arc<AtomicBool>,
    pub trading_paused: Arc<AtomicBool>,
    pub msg_count: Arc<AtomicU64>,
    pub risk_status: Arc<Mutex<String>>,
    pub market_subscriptions: Arc<Mutex<Vec<String>>>,
    pub shutdown: Arc<AtomicBool>,
    pub paper: Option<Arc<PaperEngine>>,
    pub bullpen: Option<Arc<crate::bullpen_bridge::BullpenBridge>>,
    pub discovery_store: Option<Arc<tokio::sync::RwLock<crate::bullpen_discovery::DiscoveryStore>>>,
    pub convergence_store: Option<Arc<tokio::sync::RwLock<crate::bullpen_smart_money::ConvergenceStore>>>,
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    id: Value,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

pub async fn run_agent_rpc_server(bind_addr: &str, state: AgentRpcState) -> Result<()> {
    let listener = TcpListener::bind(bind_addr).await?;
    tracing::info!(bind_addr, "Agent RPC server listening");

    while !state.shutdown.load(Ordering::Relaxed) {
        match tokio::time::timeout(Duration::from_millis(250), listener.accept()).await {
            Ok(Ok((stream, _addr))) => {
                let st = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, st).await {
                        tracing::warn!(error = %e, "Agent RPC connection error");
                    }
                });
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "Agent RPC accept error");
            }
            Err(_) => {}
        }
    }

    Ok(())
}

async fn handle_connection(mut stream: TcpStream, state: AgentRpcState) -> Result<()> {
    let mut buf = vec![0u8; 16 * 1024];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);

    let (method, path, content_len, body_start) = parse_http_headers(&req)?;
    if method != "POST" || path != "/rpc" {
        write_http(&mut stream, 404, json!({"error":"not_found"})).await?;
        return Ok(());
    }

    let mut body_bytes = req.as_bytes()[body_start..].to_vec();
    while body_bytes.len() < content_len {
        let m = stream.read(&mut buf).await?;
        if m == 0 {
            break;
        }
        body_bytes.extend_from_slice(&buf[..m]);
    }
    if body_bytes.len() < content_len {
        return Err(anyhow!("incomplete request body"));
    }
    body_bytes.truncate(content_len);

    let rpc_req: RpcRequest = serde_json::from_slice(&body_bytes)
        .map_err(|e| anyhow!("invalid JSON-RPC payload: {e}"))?;
    let id = rpc_req.id.clone();

    let response = match handle_rpc(rpc_req, &state).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "result": result, "id": id }),
        Err(err) => json!({ "jsonrpc": "2.0", "error": err, "id": id }),
    };

    write_http(&mut stream, 200, response).await?;
    Ok(())
}

fn parse_http_headers(req: &str) -> Result<(&str, &str, usize, usize)> {
    let Some(header_end) = req.find("\r\n\r\n") else {
        return Err(anyhow!("malformed HTTP request"));
    };
    let head = &req[..header_end];
    let mut lines = head.lines();
    let first = lines.next().ok_or_else(|| anyhow!("missing request line"))?;
    let mut parts = first.split_whitespace();
    let method = parts.next().ok_or_else(|| anyhow!("missing HTTP method"))?;
    let path = parts.next().ok_or_else(|| anyhow!("missing HTTP path"))?;
    let mut content_len = 0usize;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_len = v.trim().parse::<usize>()?;
        }
    }
    Ok((method, path, content_len, header_end + 4))
}

async fn handle_rpc(req: RpcRequest, state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    match req.method.as_str() {
        "blink_status" => blink_status(state).await,
        "paper_summary" => paper_summary(state).await,
        "set_pause" => set_pause(req.params, state).await,
        "bullpen_health" => bullpen_health(state).await,
        "bullpen_discovery" => bullpen_discovery(state).await,
        "bullpen_convergence" => bullpen_convergence(state).await,
        "bullpen_discover" => bullpen_discover(req.params, state).await,
        "bullpen_smart_money" => bullpen_smart_money_rpc(req.params, state).await,
        _ => Err(RpcError { code: -32601, message: "Method not found".to_string() }),
    }
}

async fn blink_status(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let risk_status = state.risk_status.lock().unwrap().clone();
    let subscriptions = state.market_subscriptions.lock().unwrap().clone();
    let mut base = json!({
        "timestamp_ms": chrono::Utc::now().timestamp_millis(),
        "ws_connected": state.ws_live.load(Ordering::Relaxed),
        "trading_paused": state.trading_paused.load(Ordering::Relaxed),
        "messages_total": state.msg_count.load(Ordering::Relaxed),
        "risk_status": risk_status,
        "subscriptions": subscriptions,
    });

    if let Some(ref paper) = state.paper {
        let (cash_usdc, nav_usdc, invested_usdc, unrealized_pnl_usdc, realized_pnl_usdc, open_positions, closed_trades, total_signals, filled_orders, skipped_orders, aborted_orders) = {
            let p = paper.portfolio.lock().await;
            (
                p.cash_usdc,
                p.nav(),
                p.total_invested(),
                p.unrealized_pnl(),
                p.realized_pnl(),
                p.positions.len(),
                p.closed_trades.len(),
                p.total_signals,
                p.filled_orders,
                p.skipped_orders,
                p.aborted_orders,
            )
        };
        let summary = paper.execution_summary().await;
        base["paper"] = json!({
            "cash_usdc": cash_usdc,
            "nav_usdc": nav_usdc,
            "invested_usdc": invested_usdc,
            "unrealized_pnl_usdc": unrealized_pnl_usdc,
            "realized_pnl_usdc": realized_pnl_usdc,
            "open_positions": open_positions,
            "closed_trades": closed_trades,
            "total_signals": total_signals,
            "filled_orders": filled_orders,
            "skipped_orders": skipped_orders,
            "aborted_orders": aborted_orders,
            "fill_rate_pct": summary.fill_rate_pct,
            "reject_rate_pct": summary.reject_rate_pct
        });
    }
    Ok(base)
}

async fn paper_summary(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let Some(ref paper) = state.paper else {
        return Err(RpcError { code: -32001, message: "Paper mode not active".to_string() });
    };
    let (cash_usdc, nav_usdc, open_positions, closed_trades) = {
        let p = paper.portfolio.lock().await;
        (p.cash_usdc, p.nav(), p.positions.len(), p.closed_trades.len())
    };
    let summary = paper.execution_summary().await;
    Ok(json!({
        "cash_usdc": cash_usdc,
        "nav_usdc": nav_usdc,
        "open_positions": open_positions,
        "closed_trades": closed_trades,
        "fill_rate_pct": summary.fill_rate_pct,
        "reject_rate_pct": summary.reject_rate_pct,
        "avg_slippage_bps": summary.avg_slippage_bps,
        "avg_queue_delay_ms": summary.avg_queue_delay_ms,
        "shadow_realism_gap_bps": summary.shadow_realism_gap_bps
    }))
}

async fn set_pause(params: Value, state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let paused = params
        .get("paused")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected {\"paused\":bool}".to_string(),
        })?;
    state.trading_paused.store(paused, Ordering::Relaxed);
    Ok(json!({ "trading_paused": paused }))
}

async fn write_http(stream: &mut TcpStream, status: u16, body: Value) -> io::Result<()> {
    let payload = body.to_string();
    let status_text = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "OK",
    };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        status_text,
        payload.len(),
        payload
    );
    stream.write_all(resp.as_bytes()).await
}

// ── Bullpen RPC methods ──────────────────────────────────────────────────

async fn bullpen_health(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let bridge = state.bullpen.as_ref().ok_or(RpcError {
        code: -32000,
        message: "Bullpen bridge not enabled".into(),
    })?;
    let health = bridge.health().await;
    Ok(json!({
        "authenticated": health.authenticated,
        "consecutive_failures": health.consecutive_failures,
        "total_calls": health.total_calls,
        "total_failures": health.total_failures,
        "avg_latency_ms": health.avg_latency_ms,
    }))
}

async fn bullpen_discovery(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let store = state.discovery_store.as_ref().ok_or(RpcError {
        code: -32000,
        message: "Discovery store not available".into(),
    })?;
    let s = store.read().await;
    let summary = s.summary();
    Ok(json!({
        "total_markets": summary.total_markets,
        "smart_money_markets": summary.smart_money_markets,
        "avg_viability": summary.avg_viability,
        "scan_count": summary.scan_count,
        "last_scan_ago_secs": summary.last_scan_ago_secs,
    }))
}

async fn bullpen_convergence(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let store = state.convergence_store.as_ref().ok_or(RpcError {
        code: -32000,
        message: "Convergence store not available".into(),
    })?;
    let s = store.read().await;
    let summary = s.summary();
    let signals: Vec<Value> = s
        .active_signals
        .iter()
        .map(|sig| {
            json!({
                "market": sig.market,
                "convergence_score": sig.convergence_score,
                "net_direction": sig.net_direction,
                "total_usd": sig.total_usd,
                "wallets": sig.wallets.len(),
            })
        })
        .collect();
    Ok(json!({
        "active_signals": summary.active_signals,
        "tracked_markets": summary.tracked_markets,
        "signals": signals,
    }))
}

async fn bullpen_discover(params: Value, state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let bridge = state.bullpen.as_ref().ok_or(RpcError {
        code: -32000,
        message: "Bullpen bridge not enabled".into(),
    })?;
    let lens = params["lens"].as_str().unwrap_or("all");
    match bridge.discover_markets(lens).await {
        Ok(resp) => Ok(json!({
            "lens": resp.lens,
            "events": resp.events.len(),
        })),
        Err(e) => Err(RpcError {
            code: -32000,
            message: format!("Discover failed: {e}"),
        }),
    }
}

async fn bullpen_smart_money_rpc(params: Value, state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let bridge = state.bullpen.as_ref().ok_or(RpcError {
        code: -32000,
        message: "Bullpen bridge not enabled".into(),
    })?;
    let signal_type = params["type"].as_str().unwrap_or("aggregated");
    match bridge.smart_money(signal_type).await {
        Ok(json) => Ok(json.0),
        Err(e) => Err(RpcError {
            code: -32000,
            message: format!("Smart money failed: {e}"),
        }),
    }
}
