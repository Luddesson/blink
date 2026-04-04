//! Axum-based web server for the Blink Engine dashboard UI.
//!
//! Provides REST endpoints and a WebSocket feed for real-time engine state.
//! Activated via `WEB_UI=true` environment variable.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use axum::{
    extract::{Path, State, WebSocketUpgrade},
    extract::ws::{Message, WebSocket},
    http::Method,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

use crate::activity_log::ActivityLog;
use crate::blink_twin::TwinSnapshot;
use crate::order_book::OrderBookStore;
use crate::paper_engine::PaperEngine;
use crate::risk_manager::RiskManager;
use crate::ws_client::WsHealthMetrics;

// ─── Shared application state ───────────────────────────────────────────────

/// Shared state passed to every axum handler via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    pub ws_live: Arc<AtomicBool>,
    pub trading_paused: Arc<AtomicBool>,
    pub msg_count: Arc<AtomicU64>,
    pub book_store: Arc<OrderBookStore>,
    pub activity_log: ActivityLog,
    pub paper: Option<Arc<PaperEngine>>,
    pub risk: Option<Arc<Mutex<RiskManager>>>,
    pub twin_snapshot: Option<Arc<Mutex<Option<TwinSnapshot>>>>,
    pub ws_health: Option<Arc<Mutex<WsHealthMetrics>>>,
    pub market_subscriptions: Arc<Mutex<Vec<String>>>,
    pub broadcast_tx: broadcast::Sender<String>,
}

// ─── JSON response types ────────────────────────────────────────────────────

#[derive(Serialize)]
struct PositionJson {
    id: usize,
    token_id: String,
    market_title: Option<String>,
    market_outcome: Option<String>,
    side: String,
    entry_price: f64,
    shares: f64,
    usdc_spent: f64,
    current_price: f64,
    unrealized_pnl: f64,
    unrealized_pnl_pct: f64,
    opened_at: String,
}

#[derive(Serialize)]
struct ClosedTradeJson {
    token_id: String,
    side: String,
    entry_price: f64,
    exit_price: f64,
    shares: f64,
    realized_pnl: f64,
    reason: String,
    opened_at: String,
    closed_at: String,
    duration_secs: u64,
    slippage_bps: f64,
}

#[derive(Serialize)]
struct ActivityEntryJson {
    timestamp: String,
    kind: String,
    message: String,
}

// ─── Router ─────────────────────────────────────────────────────────────────

/// Builds the axum router with all API endpoints.
pub fn build_router(state: AppState, static_dir: Option<String>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    let api = Router::new()
        .route("/api/status", get(get_status))
        .route("/api/portfolio", get(get_portfolio))
        .route("/api/history", get(get_history))
        .route("/api/activity", get(get_activity))
        .route("/api/orderbook/{token_id}", get(get_orderbook))
        .route("/api/orderbooks", get(get_all_orderbooks))
        .route("/api/risk", get(get_risk))
        .route("/api/twin", get(get_twin))
        .route("/api/pause", post(post_pause))
        .route("/ws", get(ws_handler))
        .with_state(state)
        .layer(cors);

    if let Some(dir) = static_dir {
        api.fallback_service(ServeDir::new(dir))
    } else {
        api
    }
}

/// Starts the web server on the given address.
pub async fn run_web_server(addr: &str, state: AppState, static_dir: Option<String>) {
    let router = build_router(state.clone(), static_dir);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind web UI address");
    tracing::info!(addr, "Web UI server listening");

    // Spawn a background task that broadcasts state snapshots every 2 seconds
    let broadcast_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            if let Ok(snapshot) = build_snapshot(&broadcast_state).await {
                let _ = broadcast_state.broadcast_tx.send(snapshot);
            }
        }
    });

    axum::serve(listener, router).await.unwrap();
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn get_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let subs = state.market_subscriptions.lock().unwrap().clone();
    let risk_status = if let Some(ref risk) = state.risk {
        let r = risk.lock().unwrap();
        if r.is_circuit_breaker_tripped() {
            "CIRCUIT_BREAKER".to_string()
        } else if !r.config().trading_enabled {
            "KILL_SWITCH_OFF".to_string()
        } else {
            "OK".to_string()
        }
    } else {
        "N/A".to_string()
    };

    Json(json!({
        "timestamp_ms": chrono::Utc::now().timestamp_millis(),
        "ws_connected": state.ws_live.load(Ordering::Relaxed),
        "trading_paused": state.trading_paused.load(Ordering::Relaxed),
        "messages_total": state.msg_count.load(Ordering::Relaxed),
        "subscriptions": subs,
        "risk_status": risk_status,
    }))
}

async fn get_portfolio(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper mode not active"}));
    };

    let p = paper.portfolio.lock().await;
    let (positions, cash, nav, invested, unrealized, realized, closed_count,
         total_signals, filled, skipped, aborted, equity_curve) = {
        let positions: Vec<PositionJson> = p.positions.iter().map(|pos| {
            PositionJson {
                id: pos.id,
                token_id: pos.token_id.clone(),
                market_title: pos.market_title.clone(),
                market_outcome: pos.market_outcome.clone(),
                side: pos.side.to_string(),
                entry_price: pos.entry_price,
                shares: pos.shares,
                usdc_spent: pos.usdc_spent,
                current_price: pos.current_price,
                unrealized_pnl: pos.unrealized_pnl(),
                unrealized_pnl_pct: pos.unrealized_pnl_pct(),
                opened_at: pos.opened_at_wall.to_rfc3339(),
            }
        }).collect();
        (positions, p.cash_usdc, p.nav(), p.total_invested(), p.unrealized_pnl(),
         p.realized_pnl(), p.closed_trades.len(), p.total_signals,
         p.filled_orders, p.skipped_orders, p.aborted_orders, p.equity_curve.clone())
    };
    drop(p);

    let summary = paper.execution_summary().await;
    Json(json!({
        "cash_usdc": cash,
        "nav_usdc": nav,
        "invested_usdc": invested,
        "unrealized_pnl_usdc": unrealized,
        "realized_pnl_usdc": realized,
        "open_positions": positions,
        "closed_trades_count": closed_count,
        "total_signals": total_signals,
        "filled_orders": filled,
        "skipped_orders": skipped,
        "aborted_orders": aborted,
        "equity_curve": equity_curve,
        "fill_rate_pct": summary.fill_rate_pct,
        "reject_rate_pct": summary.reject_rate_pct,
        "avg_slippage_bps": summary.avg_slippage_bps,
    }))
}

async fn get_history(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper mode not active"}));
    };

    let p = paper.portfolio.lock().await;
    let trades: Vec<ClosedTradeJson> = p.closed_trades.iter().map(|t| {
        ClosedTradeJson {
            token_id: t.token_id.clone(),
            side: t.side.to_string(),
            entry_price: t.entry_price,
            exit_price: t.exit_price,
            shares: t.shares,
            realized_pnl: t.realized_pnl,
            reason: t.reason.clone(),
            opened_at: t.opened_at_wall.to_rfc3339(),
            closed_at: t.closed_at_wall.to_rfc3339(),
            duration_secs: t.duration_secs,
            slippage_bps: t.scorecard.slippage_bps,
        }
    }).collect();

    Json(json!({ "trades": trades }))
}

async fn get_activity(State(state): State<AppState>) -> Json<serde_json::Value> {
    let entries: Vec<ActivityEntryJson> = {
        let log = state.activity_log.lock().unwrap();
        log.iter().rev().take(100).map(|e| {
            ActivityEntryJson {
                timestamp: e.timestamp.clone(),
                kind: format!("{:?}", e.kind),
                message: e.message.clone(),
            }
        }).collect()
    };
    Json(json!({ "entries": entries }))
}

async fn get_orderbook(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
) -> Json<serde_json::Value> {
    let book = state.book_store.get_book_snapshot(&token_id);
    match book {
        Some(ob) => {
            let bids: Vec<[f64; 2]> = ob.bids.iter().rev().take(20).map(|(&p, &s)| {
                [p as f64 / 1000.0, s as f64 / 1000.0]
            }).collect();
            let asks: Vec<[f64; 2]> = ob.asks.iter().take(20).map(|(&p, &s)| {
                [p as f64 / 1000.0, s as f64 / 1000.0]
            }).collect();
            Json(json!({
                "token_id": token_id,
                "bids": bids,
                "asks": asks,
                "best_bid": ob.best_bid().map(|p| p as f64 / 1000.0),
                "best_ask": ob.best_ask().map(|p| p as f64 / 1000.0),
                "spread_bps": ob.spread_bps(),
            }))
        }
        None => Json(json!({"error": "Order book not found", "token_id": token_id})),
    }
}

async fn get_all_orderbooks(State(state): State<AppState>) -> Json<serde_json::Value> {
    let subs = state.market_subscriptions.lock().unwrap().clone();
    let books: Vec<serde_json::Value> = subs.iter().map(|token_id| {
        if let Some(ob) = state.book_store.get_book_snapshot(token_id) {
            json!({
                "token_id": token_id,
                "best_bid": ob.best_bid().map(|p| p as f64 / 1000.0),
                "best_ask": ob.best_ask().map(|p| p as f64 / 1000.0),
                "spread_bps": ob.spread_bps(),
                "bid_depth": ob.bids.len(),
                "ask_depth": ob.asks.len(),
            })
        } else {
            json!({
                "token_id": token_id,
                "best_bid": null,
                "best_ask": null,
                "spread_bps": null,
                "bid_depth": 0,
                "ask_depth": 0,
            })
        }
    }).collect();
    Json(json!({ "orderbooks": books }))
}

async fn get_risk(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref risk) = state.risk else {
        return Json(json!({"error": "Risk manager not available"}));
    };
    let r = risk.lock().unwrap();
    let cfg = r.config();
    Json(json!({
        "trading_enabled": cfg.trading_enabled,
        "circuit_breaker_tripped": r.is_circuit_breaker_tripped(),
        "daily_pnl": r.daily_pnl(),
        "max_daily_loss_pct": cfg.max_daily_loss_pct,
        "max_concurrent_positions": cfg.max_concurrent_positions,
        "max_single_order_usdc": cfg.max_single_order_usdc,
        "max_orders_per_second": cfg.max_orders_per_second,
        "var_threshold_pct": cfg.var_threshold_pct,
    }))
}

async fn get_twin(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref twin_lock) = state.twin_snapshot else {
        return Json(json!({"error": "Twin not available"}));
    };
    let snap = twin_lock.lock().unwrap();
    match snap.as_ref() {
        Some(t) => Json(json!({
            "generation": t.generation,
            "extra_latency_ms": t.extra_latency_ms,
            "slippage_penalty_bps": t.slippage_penalty_bps,
            "drift_multiplier": t.drift_multiplier,
            "nav": t.nav,
            "realized_pnl": t.realized_pnl,
            "unrealized_pnl": t.unrealized_pnl,
            "filled_orders": t.filled_orders,
            "aborted_orders": t.aborted_orders,
            "open_positions": t.open_positions,
            "closed_trades": t.closed_trades,
            "win_rate_pct": t.win_rate_pct,
            "nav_return_pct": t.nav_return_pct,
            "max_drawdown_pct": t.max_drawdown_pct,
        })),
        None => Json(json!({"error": "No twin snapshot yet"})),
    }
}

async fn post_pause(State(state): State<AppState>, body: Json<serde_json::Value>) -> Json<serde_json::Value> {
    let paused = body.get("paused").and_then(|v| v.as_bool()).unwrap_or(false);
    state.trading_paused.store(paused, Ordering::Relaxed);
    Json(json!({ "trading_paused": paused }))
}

// ─── WebSocket handler ──────────────────────────────────────────────────────

async fn ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.broadcast_tx.subscribe();
    // Send initial snapshot
    if let Ok(snapshot) = build_snapshot(&state).await {
        let _ = socket.send(Message::Text(snapshot.into())).await;
    }
    // Then relay broadcast messages
    while let Ok(msg) = rx.recv().await {
        if socket.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}

async fn build_snapshot(state: &AppState) -> Result<String, ()> {
    let mut snapshot = json!({
        "type": "snapshot",
        "timestamp_ms": chrono::Utc::now().timestamp_millis(),
        "ws_connected": state.ws_live.load(Ordering::Relaxed),
        "trading_paused": state.trading_paused.load(Ordering::Relaxed),
        "messages_total": state.msg_count.load(Ordering::Relaxed),
    });

    // Portfolio summary
    if let Some(ref paper) = state.paper {
        let p = paper.portfolio.lock().await;
        snapshot["portfolio"] = json!({
            "cash_usdc": p.cash_usdc,
            "nav_usdc": p.nav(),
            "invested_usdc": p.total_invested(),
            "unrealized_pnl_usdc": p.unrealized_pnl(),
            "realized_pnl_usdc": p.realized_pnl(),
            "open_positions": p.positions.len(),
            "closed_trades": p.closed_trades.len(),
            "total_signals": p.total_signals,
            "filled_orders": p.filled_orders,
            "equity_curve_last": p.equity_curve.last().copied(),
        });
    }

    // Risk status
    if let Some(ref risk) = state.risk {
        let r = risk.lock().unwrap();
        snapshot["risk"] = json!({
            "trading_enabled": r.config().trading_enabled,
            "circuit_breaker": r.is_circuit_breaker_tripped(),
            "daily_pnl": r.daily_pnl(),
        });
    }

    // Recent activity (last 5 entries)
    {
        let log = state.activity_log.lock().unwrap();
        let recent: Vec<serde_json::Value> = log.iter().rev().take(5).map(|e| {
            json!({
                "timestamp": e.timestamp,
                "kind": format!("{:?}", e.kind),
                "message": e.message,
            })
        }).collect();
        snapshot["recent_activity"] = json!(recent);
    }

    serde_json::to_string(&snapshot).map_err(|_| ())
}
