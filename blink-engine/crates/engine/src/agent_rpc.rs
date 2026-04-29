//! Lightweight JSON-RPC control/status server for agent orchestration.
//!
//! Exposes a local HTTP endpoint (`POST /rpc`) with JSON-RPC 2.0 methods:
//! - `blink_status`
//! - `paper_summary`
//! - `set_pause`
//! - `get_strategy_mode`
//! - `set_strategy_mode`
//! - `rollback_strategy_mode`

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::alpha_signal::{
    AlphaAnalytics, AlphaCycleReport, AlphaRiskConfig, AlphaSignal, AlphaSignalRecord,
};
use crate::paper_engine::PaperEngine;
use crate::strategy::{StrategyController, StrategyMode};

#[derive(Clone)]
pub struct AgentRpcState {
    pub ws_live: Arc<AtomicBool>,
    pub trading_paused: Arc<AtomicBool>,
    pub msg_count: Arc<AtomicU64>,
    pub risk_status: Arc<Mutex<String>>,
    pub market_subscriptions: Arc<Mutex<Vec<String>>>,
    pub shutdown: Arc<AtomicBool>,
    pub paper: Option<Arc<PaperEngine>>,
    /// Channel for submitting AI-generated alpha signals into the engine.
    pub alpha_signal_tx: Option<tokio::sync::mpsc::Sender<AlphaSignal>>,
    /// Alpha trading analytics (accept/reject counts, P&L attribution).
    pub alpha_analytics: Option<Arc<Mutex<AlphaAnalytics>>>,
    /// Alpha-specific risk configuration.
    pub alpha_risk_config: Option<AlphaRiskConfig>,
    pub strategy_controller: Arc<StrategyController>,
    pub live_active: bool,
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
        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    let body = &req[body_start..];
    if body.len() < content_len {
        // Incomplete body, but for small RPCs this is fine
    }

    let rpc_req: RpcRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let err = json!({
                "jsonrpc": "2.0",
                "error": { "code": -32700, "message": format!("Parse error: {}", e) },
                "id": null
            });
            send_json_response(&mut stream, &err).await?;
            return Ok(());
        }
    };

    let rpc_id = rpc_req.id.clone();
    let result = handle_rpc(rpc_req, &state).await;
    let resp_json = match result {
        Ok(res) => json!({
            "jsonrpc": "2.0",
            "result": res,
            "id": rpc_id
        }),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "error": { "code": e.code, "message": e.message },
            "id": rpc_id
        }),
    };

    send_json_response(&mut stream, &resp_json).await?;
    Ok(())
}

async fn handle_rpc(
    req: RpcRequest,
    state: &AgentRpcState,
) -> std::result::Result<Value, RpcError> {
    match req.method.as_str() {
        "blink_status" => blink_status(state).await,
        "paper_summary" => paper_summary(state).await,
        "set_pause" => set_pause(req.params, state).await,
        "submit_alpha_signal" => submit_alpha_signal(req.params, state).await,
        "report_alpha_cycle" => report_alpha_cycle(req.params, state).await,
        "report_alpha_calibration" => report_alpha_calibration(req.params, state).await,
        "alpha_status" => alpha_status(state).await,
        "get_strategy_mode" => get_strategy_mode(state).await,
        "set_strategy_mode" => set_strategy_mode(req.params, state).await,
        "rollback_strategy_mode" => rollback_strategy_mode(req.params, state).await,
        "get_strategy_history" => get_strategy_history(state).await,
        _ => Err(RpcError {
            code: -32601,
            message: "Method not found".to_string(),
        }),
    }
}

async fn blink_status(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let risk_status = state
        .risk_status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let subscriptions = state
        .market_subscriptions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    Ok(json!({
        "ws_live": state.ws_live.load(Ordering::Relaxed),
        "trading_paused": state.trading_paused.load(Ordering::Relaxed),
        "msg_count": state.msg_count.load(Ordering::Relaxed),
        "risk_status": risk_status,
        "market_subscriptions": subscriptions,
        "live_active": state.live_active,
    }))
}

async fn paper_summary(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    if let Some(ref paper) = state.paper {
        let p = paper.portfolio.lock().await;
        Ok(json!({
            "nav": p.nav(),
            "cash": p.cash_usdc,
            "open_positions": p.positions.len(),
            "closed_trades": p.closed_trades.len(),
            "realized_pnl": p.realized_pnl(),
            "unrealized_pnl": p.unrealized_pnl(),
            "win_rate": if p.closed_trades.is_empty() { 0.0 } else {
                (p.closed_trades.iter().filter(|t| t.realized_pnl > 0.0).count() as f64 / p.closed_trades.len() as f64) * 100.0
            }
        }))
    } else {
        Err(RpcError {
            code: -32000,
            message: "Paper engine not enabled".to_string(),
        })
    }
}

async fn set_pause(params: Value, state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let paused = params["paused"].as_bool().ok_or(RpcError {
        code: -32602,
        message: "Missing 'paused' boolean parameter".to_string(),
    })?;

    state.trading_paused.store(paused, Ordering::SeqCst);
    tracing::info!(paused, "Trading pause state updated via RPC");

    Ok(json!({ "paused": paused }))
}

async fn submit_alpha_signal(
    params: Value,
    state: &AgentRpcState,
) -> std::result::Result<Value, RpcError> {
    let tx = state.alpha_signal_tx.as_ref().ok_or(RpcError {
        code: -32001,
        message: "Alpha pipeline not enabled".into(),
    })?;

    let signal: AlphaSignal = serde_json::from_value(params).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid alpha signal format: {}", e),
    })?;

    if let Err(e) = tx.try_send(signal) {
        return Err(RpcError {
            code: -32002,
            message: format!("Failed to queue alpha signal: {}", e),
        });
    }

    Ok(json!({ "status": "queued" }))
}

async fn report_alpha_cycle(
    params: Value,
    state: &AgentRpcState,
) -> std::result::Result<Value, RpcError> {
    let report: AlphaCycleReport = serde_json::from_value(params).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid cycle report format: {}", e),
    })?;

    if let Some(ref analytics) = state.alpha_analytics {
        let mut a = analytics.lock().unwrap_or_else(|e| e.into_inner());
        a.add_cycle_report(report);
        Ok(json!({ "status": "recorded" }))
    } else {
        Err(RpcError {
            code: -32001,
            message: "Alpha pipeline not enabled".into(),
        })
    }
}

async fn report_alpha_calibration(
    params: Value,
    state: &AgentRpcState,
) -> std::result::Result<Value, RpcError> {
    let record: AlphaSignalRecord = serde_json::from_value(params).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid calibration record format: {}", e),
    })?;

    if let Some(ref analytics) = state.alpha_analytics {
        let mut a = analytics.lock().unwrap_or_else(|e| e.into_inner());
        a.add_signal_calibration(record);
        Ok(json!({ "status": "recorded" }))
    } else {
        Err(RpcError {
            code: -32001,
            message: "Alpha pipeline not enabled".into(),
        })
    }
}

async fn alpha_status(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    if let Some(ref analytics) = state.alpha_analytics {
        let a = analytics.lock().unwrap_or_else(|e| e.into_inner());
        Ok(json!(a.clone()))
    } else {
        Err(RpcError {
            code: -32001,
            message: "Alpha pipeline not enabled".into(),
        })
    }
}

async fn get_strategy_mode(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let snapshot = state.strategy_controller.snapshot();
    Ok(json!(snapshot))
}

async fn set_strategy_mode(
    params: Value,
    state: &AgentRpcState,
) -> std::result::Result<Value, RpcError> {
    let mode_str = params["mode"].as_str().ok_or(RpcError {
        code: -32602,
        message: "Missing 'mode' string parameter".to_string(),
    })?;

    let reason = params["reason"].as_str().map(|s| s.to_string());

    let mode = match mode_str.to_lowercase().as_str() {
        "mirror" => StrategyMode::Mirror,
        "aggressive" => StrategyMode::Aggressive,
        _ => {
            return Err(RpcError {
                code: -32602,
                message: format!("Unknown strategy mode: {}", mode_str),
            })
        }
    };

    match state
        .strategy_controller
        .switch_mode(mode, reason, "agent_rpc", state.live_active)
    {
        Ok(snapshot) => Ok(json!({ "status": "switched", "snapshot": snapshot })),
        Err(e) => Err(RpcError {
            code: -32004,
            message: format!("Strategy switch failed: {:?}", e),
        }),
    }
}

async fn rollback_strategy_mode(
    params: Value,
    state: &AgentRpcState,
) -> std::result::Result<Value, RpcError> {
    let reason = params["reason"].as_str().map(|s| s.to_string());

    let snapshot = state
        .strategy_controller
        .rollback_to_mirror(reason, "agent_rpc");
    Ok(json!({ "status": "rolled_back", "snapshot": snapshot }))
}

async fn get_strategy_history(state: &AgentRpcState) -> std::result::Result<Value, RpcError> {
    let history = state.strategy_controller.history();
    Ok(json!(history))
}

async fn send_json_response(stream: &mut TcpStream, value: &Value) -> Result<()> {
    let body = serde_json::to_string(value)?;
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes()).await?;
    Ok(())
}

fn parse_http_headers(req: &str) -> Result<(&str, &str, usize, usize)> {
    let header_end = req
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow!("Malformed HTTP request (no header end)"))?;
    let headers = &req[..header_end];

    let mut lines = headers.lines();
    let first_line = lines
        .next()
        .ok_or_else(|| anyhow!("Malformed HTTP request (empty)"))?;
    let mut parts = first_line.split_whitespace();

    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let mut content_len = 0;
    for line in lines {
        if line.to_lowercase().starts_with("content-length:") {
            content_len = line["content-length:".len()..].trim().parse().unwrap_or(0);
        }
    }

    Ok((method, path, content_len, header_end + 4))
}
