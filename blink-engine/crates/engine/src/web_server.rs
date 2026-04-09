//! Axum-based web server for the Blink Engine dashboard UI.
//!
//! Provides REST endpoints and a WebSocket feed for real-time engine state.
//! Activated via `WEB_UI=true` environment variable.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    extract::ws::{Message, WebSocket},
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::activity_log::ActivityLog;
use crate::blink_twin::TwinSnapshot;
use crate::latency_tracker::LatencyTracker;
use crate::live_engine::LiveEngine;
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
    pub latency: Option<Arc<LatencyTracker>>,
    pub market_subscriptions: Arc<Mutex<Vec<String>>>,
    pub broadcast_tx: broadcast::Sender<String>,
    pub started_at: Arc<std::time::Instant>,
    /// Optional execution provider (custodial adapter) — None in paper/read-only modes.
    pub provider: Option<Arc<dyn crate::execution_provider::ExecutionProvider>>,
    /// Optional live engine — present only in live trading mode.
    pub live_engine: Option<Arc<LiveEngine>>,
    /// Optional Bullpen bridge for enrichment data.
    pub bullpen: Option<Arc<crate::bullpen_bridge::BullpenBridge>>,
    /// Optional discovery store from Bullpen scanner.
    pub discovery_store: Option<Arc<tokio::sync::RwLock<crate::bullpen_discovery::DiscoveryStore>>>,
    /// Optional convergence store from smart money monitor.
    pub convergence_store: Option<Arc<tokio::sync::RwLock<crate::bullpen_smart_money::ConvergenceStore>>>,
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
    opened_age_secs: u64,
    fee_category: String,
    fee_rate: f64,
    event_start_time: Option<i64>,
    event_end_time: Option<i64>,
}

#[derive(Serialize)]
struct ClosedTradeJson {
    token_id: String,
    market_title: Option<String>,
    side: String,
    entry_price: f64,
    exit_price: f64,
    shares: f64,
    realized_pnl: f64,
    fees_paid_usdc: f64,
    reason: String,
    opened_at: String,
    closed_at: String,
    duration_secs: u64,
    slippage_bps: f64,
    event_start_time: Option<i64>,
    event_end_time: Option<i64>,
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
        .route("/health", get(get_health))
        .route("/api/status", get(get_status))
        .route("/api/portfolio", get(get_portfolio))
        .route("/api/history", get(get_history))
        .route("/api/activity", get(get_activity))
        .route("/api/orderbook/{token_id}", get(get_orderbook))
        .route("/api/orderbooks", get(get_all_orderbooks))
        .route("/api/risk", get(get_risk))
        .route("/api/wallet", get(get_wallet_status))
        .route("/api/wallet/prepare_settlement", post(post_prepare_settlement))
        .route("/api/wallet/submit_signed_tx", post(post_submit_signed_tx))
        .route("/api/twin", get(get_twin))
        .route("/api/latency", get(get_latency))
        .route("/api/failsafe", get(get_failsafe))
        .route("/api/mode", get(get_mode))
        .route("/api/live/portfolio", get(get_live_portfolio))
        .route("/api/pause", post(post_pause))
        .route("/api/risk/reset_circuit_breaker", post(post_reset_circuit_breaker))
        .route("/api/config", post(post_update_config))
        .route("/api/debug/seed_position", post(post_seed_position))
        .route("/api/positions/{id}/sell", post(post_sell_position))
        .route("/api/metrics", get(get_metrics))
        .route("/api/fill-window", get(get_fill_window))
        .route("/api/bullpen/health", get(get_bullpen_health))
        .route("/api/bullpen/discovery", get(get_bullpen_discovery))
        .route("/api/bullpen/convergence", get(get_bullpen_convergence))
        .route("/ws", get(ws_handler))
        .with_state(state)
        .layer(cors);

    if let Some(dir) = static_dir {
        let index_path = format!("{dir}/index.html");
        api
            .route_service("/", ServeFile::new(index_path))
            .fallback_service(ServeDir::new(dir))
    } else {
        api
    }
}

/// Starts the web server on the given address.
pub async fn run_web_server(
    addr: &str,
    state: AppState,
    static_dir: Option<String>,
    broadcast_interval_secs: u64,
) {
    let router = build_router(state.clone(), static_dir);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind web UI address");
    tracing::info!(addr, broadcast_interval_secs, "Web UI server listening");

    // Broadcast state snapshots at the configured interval (default 10s).
    let broadcast_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(
            std::time::Duration::from_secs(broadcast_interval_secs.max(1))
        );
        loop {
            interval.tick().await;
            if let Ok(snapshot) = build_snapshot(&broadcast_state).await {
                let _ = broadcast_state.broadcast_tx.send(snapshot);
            }
        }
    });

    match axum::serve(listener, router).await {
        Ok(_) => {}
        Err(e) => tracing::error!("Web UI server error: {e}"),
    }
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn get_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let uptime_secs = state.started_at.elapsed().as_secs();
    let mode = if state.paper.is_some() { "paper" } else { "live" };
    Json(json!({ "status": "ok", "mode": mode, "uptime_secs": uptime_secs }))
}

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

    // Use try_lock to avoid deadlocking when the engine holds the portfolio
    // mutex during signal processing. Return a partial response instead of
    // hanging the HTTP request indefinitely.
    let Ok(p) = paper.portfolio.try_lock() else {
        return Json(json!({"error": "Portfolio busy — engine processing signal", "retry": true}));
    };
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
            opened_age_secs: pos.opened_at.elapsed().as_secs(),
            fee_category: pos.fee_category.clone(),
            fee_rate: pos.fee_rate,
            event_start_time: pos.event_start_time,
            event_end_time: pos.event_end_time,
        }
    }).collect();
    let fees_paid = p.total_fees_paid_usdc;
    let cash = p.cash_usdc;
    let nav = p.nav();
    let invested = p.total_invested();
    let unrealized = p.unrealized_pnl();
    let realized = p.realized_pnl();
    let closed_count = p.closed_trades.len();
    let wins = p.closed_trades.iter().filter(|t| t.realized_pnl > 0.0).count();
    let win_rate_pct = if closed_count > 0 { (wins as f64 / closed_count as f64) * 100.0 } else { 0.0 };
    let total_signals = p.total_signals;
    let filled = p.filled_orders;
    let skipped = p.skipped_orders;
    let aborted = p.aborted_orders;
    let equity_curve = p.equity_curve.clone();
    let equity_timestamps = p.equity_timestamps.clone();
    // Compute summary stats inline to avoid re-locking via execution_summary()
    let attempts = (filled + aborted + skipped).max(1) as f64;
    let fill_rate_pct = (filled as f64 / attempts) * 100.0;
    let reject_rate_pct = ((skipped + aborted) as f64 / attempts) * 100.0;
    let avg_slippage_bps = if p.closed_trades.is_empty() {
        0.0
    } else {
        p.closed_trades.iter().map(|t| t.scorecard.slippage_bps).sum::<f64>()
            / p.closed_trades.len() as f64
    };
    drop(p);
    let uptime_secs = state.started_at.elapsed().as_secs();

    Json(json!({
        "cash_usdc": cash,
        "nav_usdc": nav,
        "invested_usdc": invested,
        "unrealized_pnl_usdc": unrealized,
        "realized_pnl_usdc": realized,
        "fees_paid_usdc": fees_paid,
        "open_positions": positions,
        "closed_trades_count": closed_count,
        "total_signals": total_signals,
        "filled_orders": filled,
        "skipped_orders": skipped,
        "aborted_orders": aborted,
        "equity_curve": equity_curve,
        "equity_timestamps": equity_timestamps,
        "fill_rate_pct": fill_rate_pct,
        "reject_rate_pct": reject_rate_pct,
        "avg_slippage_bps": avg_slippage_bps,
        "win_rate_pct": win_rate_pct,
        "uptime_secs": uptime_secs,
    }))
}

// ─── History pagination query params ────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
struct HistoryQuery {
    page: Option<usize>,
    per_page: Option<usize>,
}

async fn get_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper mode not active"}));
    };

    let Ok(p) = paper.portfolio.try_lock() else {
        return Json(json!({"error": "Portfolio busy", "retry": true}));
    };
    let per_page = params.per_page.unwrap_or(50).clamp(1, 500);
    let total = p.closed_trades.len();
    let total_pages = total.div_ceil(per_page).max(1);
    let page = params.page.unwrap_or(1).clamp(1, total_pages);
    let skip = (page - 1) * per_page;

    // Serve newest trades first so page 1 always has the most recent results.
    let trades: Vec<ClosedTradeJson> = p.closed_trades.iter().rev().skip(skip).take(per_page).map(|t| {
        ClosedTradeJson {
            token_id: t.token_id.clone(),
            market_title: t.market_title.clone(),
            side: t.side.to_string(),
            entry_price: t.entry_price,
            exit_price: t.exit_price,
            shares: t.shares,
            realized_pnl: t.realized_pnl,
            fees_paid_usdc: t.fees_paid_usdc,
            reason: t.reason.clone(),
            opened_at: t.opened_at_wall.to_rfc3339(),
            closed_at: t.closed_at_wall.to_rfc3339(),
            duration_secs: t.duration_secs,
            slippage_bps: t.scorecard.slippage_bps,
            event_start_time: t.event_start_time,
            event_end_time: t.event_end_time,
        }
    }).collect();

    Json(json!({
        "trades": trades,
        "total": total,
        "page": page,
        "per_page": per_page,
        "total_pages": total_pages,
    }))
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

    // Build token→title map from open positions
    let title_map: std::collections::HashMap<String, String> = if let Some(ref pe) = state.paper {
        if let Ok(p) = pe.portfolio.try_lock() {
            p.positions.iter()
                .filter_map(|pos| pos.market_title.as_ref().map(|t: &String| (pos.token_id.clone(), t.clone())))
                .collect()
        } else {
            std::collections::HashMap::new()
        }
    } else {
        std::collections::HashMap::new()
    };

    let books: Vec<serde_json::Value> = subs.iter().map(|token_id| {
<<<<<<< Updated upstream
        let title = title_map.get(token_id).cloned();
=======
        // Look up market title from discovery store if available
        let market_title: Option<String> = state.discovery_store.as_ref().and_then(|store| {
            store.try_read().ok().and_then(|s| {
                s.get(token_id).and_then(|m| m.title.clone())
            })
        });

>>>>>>> Stashed changes
        if let Some(ob) = state.book_store.get_book_snapshot(token_id) {
            let bids: Vec<[f64; 2]> = ob.bids.iter().rev().take(15)
                .map(|(&p, &s)| [p as f64 / 1000.0, s as f64 / 1000.0])
                .collect();
            let asks: Vec<[f64; 2]> = ob.asks.iter().take(15)
                .map(|(&p, &s)| [p as f64 / 1000.0, s as f64 / 1000.0])
                .collect();
            json!({
                "token_id": token_id,
<<<<<<< Updated upstream
                "market_title": title,
=======
                "market_title": market_title,
>>>>>>> Stashed changes
                "best_bid": ob.best_bid().map(|p| p as f64 / 1000.0),
                "best_ask": ob.best_ask().map(|p| p as f64 / 1000.0),
                "spread_bps": ob.spread_bps(),
                "bids": bids,
                "asks": asks,
            })
        } else {
            json!({
                "token_id": token_id,
<<<<<<< Updated upstream
                "market_title": title,
=======
                "market_title": market_title,
>>>>>>> Stashed changes
                "best_bid": null,
                "best_ask": null,
                "spread_bps": null,
                "bids": [],
                "asks": [],
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
    let stop_loss_enabled = std::env::var("STOP_LOSS_ENABLED").map(|v| v.eq_ignore_ascii_case("true")).unwrap_or(false);
    let stop_loss_pct = std::env::var("STOP_LOSS_PCT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);

    Json(json!({
        "trading_enabled": cfg.trading_enabled,
        "circuit_breaker_tripped": r.is_circuit_breaker_tripped(),
        "circuit_breaker_reason": r.circuit_breaker_reason(),
        "daily_pnl": r.daily_pnl(),
        "max_daily_loss_pct": cfg.max_daily_loss_pct,
        "max_concurrent_positions": cfg.max_concurrent_positions,
        "max_single_order_usdc": cfg.max_single_order_usdc,
        "max_orders_per_second": cfg.max_orders_per_second,
        "var_threshold_pct": cfg.var_threshold_pct,
        "stop_loss_enabled": stop_loss_enabled,
        "stop_loss_pct": stop_loss_pct,
    }))
}

async fn get_wallet_status(State(_state): State<AppState>) -> Json<serde_json::Value> {
    // Lightweight status endpoint for live-integration planning. Does not access secrets.
    let live_mode = std::env::var("LIVE_MODE").map(|v| v.eq_ignore_ascii_case("true") || v == "1").unwrap_or(false);
    let provider = std::env::var("CUSTODIAL_PROVIDER").unwrap_or_else(|_| "none".to_string());
    let provider_ready = std::env::var("CUSTODIAL_PROVIDER_READY").map(|v| v.eq_ignore_ascii_case("true") || v == "1").unwrap_or(false);
    let note = if provider == "none" {
        "No provider configured. Set CUSTODIAL_PROVIDER and follow onboarding steps."
    } else if !provider_ready {
        "Provider configured but not marked ready. Provide credentials via secure vault and set CUSTODIAL_PROVIDER_READY=1 once tested."
    } else {
        "Provider configured and marked ready. Proceed with a small test/pilot on testnet/mainnet as appropriate."
    };

    Json(json!({
        "live_mode": live_mode,
        "provider": provider,
        "provider_ready": provider_ready,
        "note": note,
    }))
}

/// Prepare an unsigned settlement transaction for client-side signing with Phantom.
///
/// POST /api/wallet/prepare_settlement
/// Body: { amount_usdc: number, recipient: string, token: string (optional), position_id: number (optional) }
async fn post_prepare_settlement(
    State(_state): State<AppState>,
    body: Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let amount = body.get("amount_usdc").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let token = body.get("token").and_then(|v| v.as_str()).unwrap_or("USDC").to_string();
    let recipient = body.get("recipient").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let position_id = body.get("position_id").and_then(|v| v.as_u64());

    // Build a generic unsigned payload. For Phantom/Solana the client will
    // fill recent blockhash and sign; for EVM the client will set nonce and gas.
    let chain = std::env::var("SETTLEMENT_CHAIN").unwrap_or_else(|_| "solana".to_string());
    let decimals = std::env::var("TOKEN_DECIMALS").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(6u32);
    let unsigned_payload = json!({
        "chain": chain,
        "type": "transfer",
        "token": token,
        "amount_usdc": amount,
        "decimals": decimals,
        "to": recipient,
        // Server does not provide recent blockhash/nonce for safety; client should fill before signing.
        "recent_blockhash": null,
        "nonce_hint": null,
        "gas_limit_hint": null,
        "note": "Unsigned settlement payload: sign with Phantom and broadcast from the client. Server keeps no keys."
    });

    Json(json!({
        "ok": true,
        "unsigned_tx": unsigned_payload,
        "position_id": position_id,
    }))
}

/// Accept a client-signed transaction payload and optionally broadcast it (stubbed).
/// POST /api/wallet/submit_signed_tx
/// Body: { chain: string, signed_tx: object|string, raw_tx: object (optional), position_id: number (optional) }
async fn post_submit_signed_tx(
    State(state): State<AppState>,
    body: Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let chain = body.get("chain").and_then(|v| v.as_str()).unwrap_or("solana");
    let signed_tx = body.get("signed_tx").cloned().unwrap_or(json!(null));
    let raw_tx = body.get("raw_tx").cloned().unwrap_or(json!(null));
    let position_id = body.get("position_id").and_then(|v| v.as_u64());

    // Log to activity log for auditability
    let msg = format!("Received signed tx for chain={}: position={:?}", chain, position_id);
    crate::activity_log::push(&state.activity_log, crate::activity_log::EntryKind::Engine, msg);

    // In a real implementation this would broadcast to the network and return a txid.
    // Here we stub: accept the payload, return a generated tx_id and echo the signed payload.
    let tx_id = format!("stubbed-{}", chrono::Utc::now().timestamp_millis());

    Json(json!({
        "ok": true,
        "tx_id": tx_id,
        "echo": { "signed_tx": signed_tx, "raw_tx": raw_tx },
    }))
}

async fn get_twin(State(state): State<AppState>) -> impl IntoResponse {
    let Some(ref twin_lock) = state.twin_snapshot else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "Twin not available"}))).into_response();
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
        })).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "No twin snapshot yet"}))).into_response(),
    }
}

async fn get_latency(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref tracker) = state.latency else {
        return Json(json!({"error": "Latency tracker not available"}));
    };
    let signal_summary = tracker.signal_age.lock().unwrap().summary();
    let msg_rate = tracker.msgs_per_sec.lock().unwrap().per_second();
    Json(json!({
        "signal_age": signal_summary,
        "ws_msg_per_sec": msg_rate,
        "bucket_labels": ["0-10µs", "10-50µs", "50-100µs", "100-500µs", "500-1000µs", "1000+µs"],
    }))
}

async fn get_failsafe(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref live) = state.live_engine else {
        return Json(json!({ "available": false, "reason": "not in live trading mode" }));
    };
    let snap = live.failsafe_metrics_snapshot();
    Json(json!({
        "available": true,
        "trigger_count": snap.trigger_count,
        "check_count": snap.check_count,
        "max_observed_drift_bps": snap.max_observed_drift_bps,
        "confirmed_fills": snap.confirmed_fills,
        "no_fills": snap.no_fills,
        "stale_orders": snap.stale_orders,
        "confirmation_rate_pct": snap.confirmation_rate_pct,
        "heartbeat_ok_count": snap.heartbeat_ok_count,
        "heartbeat_fail_count": snap.heartbeat_fail_count,
    }))
}

async fn get_mode(State(state): State<AppState>) -> Json<serde_json::Value> {
    let live_trading = std::env::var("LIVE_TRADING")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let mode = if state.live_engine.is_some() && live_trading {
        "live"
    } else if state.paper.is_some() {
        "paper"
    } else {
        "readonly"
    };
    Json(json!({
        "mode": mode,
        "live_trading_env": live_trading,
        "paper_active": state.paper.is_some(),
        "live_active": state.live_engine.is_some(),
    }))
}

async fn get_live_portfolio(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref live) = state.live_engine else {
        return Json(json!({ "error": "not in live trading mode" }));
    };
    let failsafe = live.failsafe_metrics_snapshot();
    let pending_count = live.pending_orders_count().await;
    let (daily_pnl, cb_tripped, max_daily_loss_pct, trading_enabled) =
        if let Some(ref risk) = state.risk {
            let r = risk.lock().unwrap();
            (r.daily_pnl(), r.is_circuit_breaker_tripped(), r.config().max_daily_loss_pct, r.config().trading_enabled)
        } else {
            (0.0, false, 0.1, false)
        };
    let uptime_secs = state.started_at.elapsed().as_secs();
    Json(json!({
        "mode": "live",
        "pending_orders": pending_count,
        "confirmed_fills": failsafe.confirmed_fills,
        "no_fills": failsafe.no_fills,
        "stale_orders": failsafe.stale_orders,
        "confirmation_rate_pct": failsafe.confirmation_rate_pct,
        "daily_pnl_usdc": daily_pnl,
        "max_daily_loss_pct": max_daily_loss_pct,
        "circuit_breaker_tripped": cb_tripped,
        "trading_enabled": trading_enabled,
        "heartbeat_ok": failsafe.heartbeat_ok_count,
        "heartbeat_fail": failsafe.heartbeat_fail_count,
        "trigger_count": failsafe.trigger_count,
        "uptime_secs": uptime_secs,
    }))
}

async fn post_pause(State(state): State<AppState>, body: Json<serde_json::Value>) -> Json<serde_json::Value> {
    let paused = body.get("paused").and_then(|v| v.as_bool()).unwrap_or(false);
    state.trading_paused.store(paused, Ordering::Relaxed);
    Json(json!({ "trading_paused": paused }))
}

async fn post_reset_circuit_breaker(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref risk) = state.risk else {
        return Json(json!({ "error": "Risk manager not available" }));
    };
    risk.lock().unwrap().reset_circuit_breaker();
    tracing::warn!("Circuit breaker manually reset via API");
    Json(json!({ "ok": true, "circuit_breaker_tripped": false }))
}

async fn post_update_config(
    State(state): State<AppState>,
    body: Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let Some(ref risk) = state.risk else {
        return Json(json!({ "error": "Risk manager not available" }));
    };
    let mut rm = risk.lock().unwrap();
    let cfg = rm.config_mut();
    let mut changed = Vec::new();

    if let Some(v) = body.get("max_daily_loss_pct").and_then(|v| v.as_f64()) {
        cfg.max_daily_loss_pct = v.clamp(0.01, 1.0);
        changed.push("max_daily_loss_pct");
    }
    if let Some(v) = body.get("max_concurrent_positions").and_then(|v| v.as_u64()) {
        cfg.max_concurrent_positions = (v as usize).clamp(1, 100);
        changed.push("max_concurrent_positions");
    }
    if let Some(v) = body.get("max_single_order_usdc").and_then(|v| v.as_f64()) {
        cfg.max_single_order_usdc = v.clamp(1.0, 10_000.0);
        changed.push("max_single_order_usdc");
    }
    if let Some(v) = body.get("max_orders_per_second").and_then(|v| v.as_u64()) {
        cfg.max_orders_per_second = (v as u32).clamp(1, 100);
        changed.push("max_orders_per_second");
    }
    if let Some(v) = body.get("var_threshold_pct").and_then(|v| v.as_f64()) {
        cfg.var_threshold_pct = v.clamp(0.01, 1.0);
        changed.push("var_threshold_pct");
    }

    tracing::warn!(fields = ?changed, "Risk config updated via API");
    Json(json!({ "ok": true, "updated": changed }))
}

async fn post_sell_position(
    State(state): State<AppState>,
    Path(id): Path<usize>,
    body: Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let fraction = body.get("fraction").and_then(|v| v.as_f64()).unwrap_or(1.0).clamp(0.01, 1.0);

    let Some(ref paper) = state.paper else {
        return Json(json!({ "error": "Paper engine not available" }));
    };

    let mut p = paper.portfolio.lock().await;
    let pos_index = p.positions.iter().position(|pos| pos.id == id);
    let Some(idx) = pos_index else {
        return Json(json!({ "error": "Position not found" }));
    };

    let reason = format!("manual_sell@{:.0}%", fraction * 100.0);
    let removed = p.close_position_fraction(idx, fraction, reason.clone());

    // Record realized P&L from the close in the risk manager
    if let Some(ref risk) = state.risk {
        if let Some(last_trade) = p.closed_trades.last() {
            risk.lock().unwrap().record_close(last_trade.realized_pnl);
        }
    }

    // Log to activity log (use AppState's shared log, not paper's private field)
    {
        let msg = format!("MANUAL SELL: pos #{id} {:.0}% — {reason}", fraction * 100.0);
        crate::activity_log::push(&state.activity_log, crate::activity_log::EntryKind::Engine, msg);
    }

    let pnl = p.closed_trades.last().map(|t| t.realized_pnl).unwrap_or(0.0);
    let fees = p.closed_trades.last().map(|t| t.fees_paid_usdc).unwrap_or(0.0);
    Json(json!({
        "ok": true,
        "position_id": id,
        "fraction": fraction,
        "fully_closed": removed,
        "realized_pnl": pnl,
        "fees_paid_usdc": fees,
        "cash_usdc": p.cash_usdc,
    }))
}

// ─── WebSocket handler ──────────────────────────────────────────────────────

async fn post_seed_position(
    State(state): State<AppState>,
    body: Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let token_id = body.get("token_id").and_then(|v| v.as_str()).unwrap_or("asset-1").to_string();
    let market_title = body.get("market_title").and_then(|v| v.as_str()).map(|s| s.to_string());
    let side_str = body.get("side").and_then(|v| v.as_str()).unwrap_or("BUY");
    let side = if side_str.eq_ignore_ascii_case("SELL") { crate::types::OrderSide::Sell } else { crate::types::OrderSide::Buy };
    let entry_price = body.get("entry_price").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let usdc_size = body.get("usdc_size").and_then(|v| v.as_f64()).unwrap_or(5.0);

    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper engine not available"}));
    };
    let mut p = paper.portfolio.lock().await;
    let id = p.open_position_with_meta(token_id.clone(), market_title.clone(), None, side, entry_price, usdc_size, "debug".to_string(), 0.0, 0, "debug", None, None);
    let pos_json = p.positions.iter().find(|x| x.id == id).map(|pos| json!({
        "id": pos.id,
        "token_id": pos.token_id,
        "market_title": pos.market_title,
        "side": pos.side.to_string(),
        "entry_price": pos.entry_price,
        "shares": pos.shares,
        "usdc_spent": pos.usdc_spent,
        "entry_fee_paid_usdc": pos.entry_fee_paid_usdc,
        "current_price": pos.current_price,
    }));
    Json(json!({"ok": true, "position_id": id, "position": pos_json, "cash_usdc": p.cash_usdc }))
}

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

    // Portfolio summary — use try_lock to avoid blocking the broadcast loop
    // when the engine holds the portfolio mutex during signal processing.
    if let Some(ref paper) = state.paper {
        let Ok(p) = paper.portfolio.try_lock() else {
            return serde_json::to_string(&snapshot).map_err(|_| ());
        };
        let attempts = (p.filled_orders + p.aborted_orders + p.skipped_orders).max(1) as f64;
        let fill_rate_pct = (p.filled_orders as f64 / attempts) * 100.0;
        let wins = p.closed_trades.iter().filter(|t| t.realized_pnl > 0.0).count();
        let win_rate_pct = if p.closed_trades.is_empty() { 0.0 } else {
            (wins as f64 / p.closed_trades.len() as f64) * 100.0
        };
        let uptime_secs = state.started_at.elapsed().as_secs();

        // Decimate equity curve to at most 150 points to keep the WS payload small.
        // The full curve is available via /api/portfolio for initial chart loads.
        let equity_len = p.equity_curve.len();
        let max_ws_equity_points: usize = 150;
        let (equity_curve_ws, equity_timestamps_ws): (Vec<f64>, Vec<i64>) = if equity_len <= max_ws_equity_points {
            (p.equity_curve.clone(), p.equity_timestamps.clone())
        } else {
            let step = equity_len as f64 / max_ws_equity_points as f64;
            let indices: Vec<usize> = (0..max_ws_equity_points)
                .map(|i| ((i as f64 * step) as usize).min(equity_len - 1))
                .collect();
            let curve: Vec<f64> = indices.iter().map(|&i| p.equity_curve[i]).collect();
            let ts: Vec<i64> = if p.equity_timestamps.len() == equity_len {
                indices.iter().map(|&i| p.equity_timestamps[i]).collect()
            } else {
                vec![]
            };
            (curve, ts)
        };

        let positions_ws: Vec<serde_json::Value> = p.positions.iter().map(|pos| json!({
            "id": pos.id,
            "token_id": pos.token_id,
            "market_title": pos.market_title,
            "market_outcome": pos.market_outcome,
            "side": pos.side.to_string(),
            "entry_price": pos.entry_price,
            "shares": pos.shares,
            "usdc_spent": pos.usdc_spent,
            "current_price": pos.current_price,
            "unrealized_pnl": pos.unrealized_pnl(),
            "unrealized_pnl_pct": pos.unrealized_pnl_pct(),
            "opened_age_secs": pos.opened_at.elapsed().as_secs(),
            "event_start_time": pos.event_start_time,
            "event_end_time": pos.event_end_time,
        })).collect();

        let avg_slippage_bps = if p.closed_trades.is_empty() { 0.0 } else {
            p.closed_trades.iter().map(|t| t.scorecard.slippage_bps).sum::<f64>()
                / p.closed_trades.len() as f64
        };

        snapshot["portfolio"] = json!({
            "cash_usdc": p.cash_usdc,
            "nav_usdc": p.nav(),
            "invested_usdc": p.total_invested(),
            "unrealized_pnl_usdc": p.unrealized_pnl(),
            "realized_pnl_usdc": p.realized_pnl(),
            "fees_paid_usdc": p.total_fees_paid_usdc,
            "open_positions": positions_ws,
            "closed_trades_count": p.closed_trades.len(),
            "total_signals": p.total_signals,
            "filled_orders": p.filled_orders,
            "skipped_orders": p.skipped_orders,
            "aborted_orders": p.aborted_orders,
            "avg_slippage_bps": avg_slippage_bps,
            "fill_rate_pct": fill_rate_pct,
            "equity_curve": equity_curve_ws,
            "equity_timestamps": equity_timestamps_ws,
            "win_rate_pct": win_rate_pct,
            "uptime_secs": uptime_secs,
        });
    }

    // Risk status
    if let Some(ref risk) = state.risk {
        let r = risk.lock().unwrap();
        let cfg = r.config();
        snapshot["risk"] = json!({
            "trading_enabled": cfg.trading_enabled,
            "circuit_breaker": r.is_circuit_breaker_tripped(),
            "circuit_breaker_tripped": r.is_circuit_breaker_tripped(),
            "daily_pnl": r.daily_pnl(),
            "max_daily_loss_pct": cfg.max_daily_loss_pct,
            "max_concurrent_positions": cfg.max_concurrent_positions,
            "max_single_order_usdc": cfg.max_single_order_usdc,
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

async fn get_metrics(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({ "available": false }));
    };
    let analytics = paper.rejection_analytics_handle();
    let analytics = analytics.lock().await;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let window_ms = 60_000i64;
    let mut reason_counts: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut total_recent = 0usize;
    for (reason, timestamps) in &analytics.reasons {
        let recent = timestamps.iter().filter(|&&t| now_ms - t < window_ms).count();
        total_recent += recent;
        reason_counts.insert(reason.clone(), json!(recent));
    }
    drop(analytics);
    let uptime_secs = state.started_at.elapsed().as_secs();
    Json(json!({
        "available": true,
        "signals_rejected_last_60s": total_recent,
        "rejection_by_reason": reason_counts,
        "uptime_secs": uptime_secs,
    }))
}

async fn get_fill_window(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({ "available": false }));
    };
    let snap = paper.fill_window.lock().unwrap().clone();
    match snap {
        None => Json(json!({ "available": false, "reason": "no active fill window" })),
        Some(s) => Json(json!({
            "available": true,
            "token_id": s.token_id,
            "side": format!("{:?}", s.side),
            "entry_price": s.entry_price,
            "current_price": s.current_price,
            "drift_pct": s.drift_pct,
            "elapsed_secs": s.elapsed.as_secs_f64(),
            "countdown_secs": s.countdown.as_secs_f64(),
        })),
    }
}

// ─── Bullpen API endpoints ──────────────────────────────────────────────────

async fn get_bullpen_health(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref bridge) = state.bullpen {
        let health = bridge.health().await;
        Json(json!({
            "enabled": true,
            "authenticated": health.authenticated,
            "consecutive_failures": health.consecutive_failures,
            "total_calls": health.total_calls,
            "total_failures": health.total_failures,
            "avg_latency_ms": health.avg_latency_ms,
        }))
    } else {
        Json(json!({ "enabled": false }))
    }
}

async fn get_bullpen_discovery(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref store) = state.discovery_store {
        let s = store.read().await;
        let summary = s.summary();
        let mut markets: Vec<serde_json::Value> = s.all_markets().iter().map(|m| {
            json!({
                "token_id": m.token_id,
                "title": m.title,
                "lenses": m.discovery_lenses,
                "viability_score": m.viability_score,
                "conviction_boost": m.conviction_boost,
                "smart_money_interest": m.smart_money_interest,
                "seen_count": m.seen_count,
            })
        }).collect();
        // Sort by viability descending for consistent display
        markets.sort_by(|a, b| {
            let va = a["viability_score"].as_f64().unwrap_or(0.0);
            let vb = b["viability_score"].as_f64().unwrap_or(0.0);
            vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal)
        });
        Json(json!({
            "enabled": true,
            "total_markets": summary.total_markets,
            "smart_money_markets": summary.smart_money_markets,
            "avg_viability": summary.avg_viability,
            "scan_count": summary.scan_count,
            "last_scan_ago_secs": summary.last_scan_ago_secs,
            "markets": markets,
        })).into_response()
    } else {
        Json(json!({ "enabled": false })).into_response()
    }
}

async fn get_bullpen_convergence(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref store) = state.convergence_store {
        let s = store.read().await;
        let summary = s.summary();
        let signals: Vec<serde_json::Value> = s.active_signals.iter().map(|sig| {
            json!({
                "market_title": sig.market,
                "convergence_score": sig.convergence_score,
                "net_direction": sig.net_direction,
                "total_usd": sig.total_usd,
                "wallets": sig.wallets.len(),
            })
        }).collect();
        Json(json!({
            "enabled": true,
            "active_signals": summary.active_signals,
            "tracked_markets": summary.tracked_markets,
            "signals": signals,
        }))
    } else {
        Json(json!({ "enabled": false }))
    }
}
