//! Axum-based web server for the Blink Engine dashboard UI.
//!
//! Provides REST endpoints and a WebSocket feed for real-time engine state.
//! Activated via `WEB_UI=true` environment variable.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use axum::{
    extract::ws::{Message, WebSocket},
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::activity_log::ActivityLog;
use crate::alpha_signal::AlphaAnalytics;
use crate::backtest_engine::{
    load_ticks_csv, run_parameter_sweep, run_walk_forward, BacktestConfig, BacktestEngine,
    SweepAxes, WalkForwardAggregate,
};
use crate::blink_twin::TwinSnapshot;
use crate::clickhouse_logger;
use crate::latency_tracker::LatencyTracker;
use crate::live_engine::LiveEngine;
use crate::order_book::OrderBookStore;
use crate::paper_engine::PaperEngine;
use crate::paper_portfolio::PaperPortfolio;
use crate::risk_manager::RiskManager;
use crate::strategy::{StrategyController, StrategyMode, StrategySwitchError};
use crate::timed_mutex::TimedMutex;
use crate::ws_client::WsHealthMetrics;

type SlugCache = Arc<Mutex<std::collections::HashMap<String, String>>>;

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
    pub risk: Option<Arc<TimedMutex<RiskManager>>>,
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
    pub convergence_store:
        Option<Arc<tokio::sync::RwLock<crate::bullpen_smart_money::ConvergenceStore>>>,
    /// In-memory cache of token_id → Polymarket event slug.
    pub slug_cache: SlugCache,
    /// Last successfully-built portfolio JSON for the WS snapshot.
    /// Written whenever the portfolio mutex is free; used as a fallback
    /// when try_lock fails so the UI always receives non-empty portfolio data.
    pub portfolio_cache: Arc<std::sync::RwLock<Option<serde_json::Value>>>,
    /// Optional ClickHouse URL — enables historical equity queries via /api/analytics/equity.
    pub clickhouse_url: Option<String>,
    /// Monotonically-increasing snapshot sequence number.
    pub snapshot_seq: Arc<AtomicU64>,
    /// Unix-millis timestamp of the last successful portfolio cache write.
    pub portfolio_cached_at_ms: Arc<AtomicU64>,
    /// Alpha analytics — present when ALPHA_ENABLED=true. Shared with agent_rpc.
    pub alpha_analytics: Option<Arc<Mutex<AlphaAnalytics>>>,
    pub strategy_controller: Arc<StrategyController>,
    pub strategy_live_active: bool,
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
    secs_to_event: Option<i64>,
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

#[derive(Debug, Deserialize, Default)]
struct RejectionsQuery {
    reason: Option<String>,
    since_hours: Option<u64>,
    limit: Option<usize>,
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
        .route("/api/rejections", get(get_rejections))
        .route("/api/orderbook/{token_id}", get(get_orderbook))
        .route("/api/orderbooks", get(get_all_orderbooks))
        .route("/api/risk", get(get_risk))
        .route("/api/wallet", get(get_wallet_status))
        .route(
            "/api/wallet/prepare_settlement",
            post(post_prepare_settlement),
        )
        .route("/api/wallet/submit_signed_tx", post(post_submit_signed_tx))
        .route("/api/twin", get(get_twin))
        .route("/api/latency", get(get_latency))
        .route("/api/failsafe", get(get_failsafe))
        .route("/api/mode", get(get_mode))
        .route("/api/strategy", post(post_strategy))
        .route("/api/strategy/rollback", post(post_strategy_rollback))
        .route("/api/live/portfolio", get(get_live_portfolio))
        .route("/api/pause", post(post_pause))
        .route(
            "/api/risk/reset_circuit_breaker",
            post(post_reset_circuit_breaker),
        )
        .route("/api/config", post(post_update_config))
        .route("/api/debug/seed_position", post(post_seed_position))
        .route("/api/positions/{id}/sell", post(post_sell_position))
        .route("/api/metrics", get(get_metrics))
        .route("/api/fill-window", get(get_fill_window))
        .route("/api/bullpen/health", get(get_bullpen_health))
        .route("/api/bullpen/discovery", get(get_bullpen_discovery))
        .route("/api/bullpen/convergence", get(get_bullpen_convergence))
        .route("/api/bullpen/short_markets", get(get_bullpen_short_markets))
        .route("/api/market-url/{token_id}", get(get_market_url))
        .route("/api/pnl-attribution", get(get_pnl_attribution))
        .route("/api/backtest", post(post_backtest))
        .route("/api/backtest/sweep", post(post_backtest_sweep))
        .route(
            "/api/backtest/walk-forward",
            post(post_backtest_walk_forward),
        )
        .route("/api/analytics/equity", get(get_analytics_equity))
        .route("/api/alpha", get(get_alpha_status))
        .route("/api/alpha/calibration", get(get_alpha_calibration))
        .route("/api/project-inventory", get(get_project_inventory))
        .route("/api/gates", get(get_gates))
        .route("/ws", get(ws_handler))
        .with_state(state)
        .layer(cors);

    if let Some(dir) = static_dir {
        let index_path = format!("{dir}/index.html");
        api.route_service("/", ServeFile::new(index_path))
            .fallback_service(ServeDir::new(dir))
    } else {
        api
    }
}

/// Builds a portfolio JSON snapshot from a locked `PaperPortfolio`.
///
/// `max_equity_points` caps the equity curve length so WS payloads stay small.
/// Pass `usize::MAX` to include the full curve (e.g. for the HTTP endpoint cache).
fn build_portfolio_json(
    p: &PaperPortfolio,
    uptime_secs: u64,
    max_equity_points: usize,
) -> serde_json::Value {
    let attempts = (p.filled_orders + p.aborted_orders + p.skipped_orders).max(1) as f64;
    let fill_rate_pct = (p.filled_orders as f64 / attempts) * 100.0;
    let reject_rate_pct = ((p.skipped_orders + p.aborted_orders) as f64 / attempts) * 100.0;
    let wins = p
        .closed_trades
        .iter()
        .filter(|t| t.realized_pnl > 0.0)
        .count();
    let win_rate_pct = if p.closed_trades.is_empty() {
        0.0
    } else {
        (wins as f64 / p.closed_trades.len() as f64) * 100.0
    };
    let avg_slippage_bps = if p.closed_trades.is_empty() {
        0.0
    } else {
        p.closed_trades
            .iter()
            .map(|t| t.scorecard.slippage_bps)
            .sum::<f64>()
            / p.closed_trades.len() as f64
    };

    let equity_len = p.equity_curve.len();
    let (equity_curve, equity_timestamps) = if equity_len <= max_equity_points {
        (p.equity_curve.clone(), p.equity_timestamps.clone())
    } else {
        // Downsample but always include the very last point so the chart
        // reflects the most recent NAV, not data from minutes ago.
        let n = max_equity_points.max(2);
        let step = equity_len as f64 / (n - 1) as f64;
        let mut indices: Vec<usize> = (0..n - 1)
            .map(|i| ((i as f64 * step) as usize).min(equity_len - 1))
            .collect();
        indices.push(equity_len - 1); // always include last
        indices.dedup(); // remove duplicate if last was already included
        let curve: Vec<f64> = indices.iter().map(|&i| p.equity_curve[i]).collect();
        let ts: Vec<i64> = if p.equity_timestamps.len() == equity_len {
            indices.iter().map(|&i| p.equity_timestamps[i]).collect()
        } else {
            vec![]
        };
        (curve, ts)
    };

    let positions: Vec<serde_json::Value> = p
        .positions
        .iter()
        .map(|pos| {
            let now_ts = chrono::Utc::now().timestamp();
            let secs_to_event = pos
                .event_start_time
                .or(pos.event_end_time)
                .map(|ts| ts - now_ts);
            json!({
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
                "opened_at": pos.opened_at_wall.to_rfc3339(),
                "opened_age_secs": pos.opened_at.elapsed().as_secs(),
                "fee_category": pos.fee_category,
                "fee_rate": pos.fee_rate,
                "event_start_time": pos.event_start_time,
                "event_end_time": pos.event_end_time,
                "secs_to_event": secs_to_event,
            })
        })
        .collect();

    json!({
        "cash_usdc": p.cash_usdc,
        "nav_usdc": p.nav(),
        "invested_usdc": p.total_invested(),
        "unrealized_pnl_usdc": p.unrealized_pnl(),
        "realized_pnl_usdc": p.realized_pnl(),
        "fees_paid_usdc": p.total_fees_paid_usdc,
        "open_positions": positions,
        "closed_trades_count": p.closed_trades.len(),
        "total_signals": p.total_signals,
        "filled_orders": p.filled_orders,
        "skipped_orders": p.skipped_orders,
        "aborted_orders": p.aborted_orders,
        "equity_curve": equity_curve,
        "equity_timestamps": equity_timestamps,
        "fill_rate_pct": fill_rate_pct,
        "reject_rate_pct": reject_rate_pct,
        "avg_slippage_bps": avg_slippage_bps,
        "win_rate_pct": win_rate_pct,
        "uptime_secs": uptime_secs,
    })
}

fn strategy_json(state: &AppState) -> serde_json::Value {
    let snapshot = state.strategy_controller.snapshot();
    let mut value = json!(snapshot);
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "rollback".to_string(),
            json!({
                "target_mode": "mirror",
                "available": true,
                "active": snapshot.current_mode == StrategyMode::Mirror,
                "required": snapshot.current_mode != StrategyMode::Mirror,
                "api_path": "/api/strategy/rollback",
                "api_alt_path": "/api/strategy with {mode:\"mirror\",force_rollback_to_mirror:true}",
            }),
        );
    }
    value
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
        .unwrap_or_else(|e| panic!("Failed to bind web UI on {addr}: {e}"));
    tracing::info!(addr, broadcast_interval_secs, "Web UI server listening");

    // Portfolio cache refresher — properly awaits the tokio Mutex every 2s so
    // the UI always has fresh portfolio data regardless of signal-loop contention.
    // build_snapshot() and get_portfolio() both read from this cache.
    if let Some(ref paper) = state.paper {
        let paper = Arc::clone(paper);
        let cache = Arc::clone(&state.portfolio_cache);
        let started_at = Arc::clone(&state.started_at);
        let cached_at = Arc::clone(&state.portfolio_cached_at_ms);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                let p = match tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    paper.portfolio.lock(),
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(_) => {
                        tracing::warn!(
                            "portfolio cache refresher: lock timeout (2s) — skipping refresh"
                        );
                        continue;
                    }
                };
                let uptime_secs = started_at.elapsed().as_secs();
                let portfolio_json = build_portfolio_json(&p, uptime_secs, 300);
                drop(p);
                if let Ok(mut c) = cache.write() {
                    *c = Some(portfolio_json);
                    cached_at.store(
                        chrono::Utc::now().timestamp_millis() as u64,
                        Ordering::Relaxed,
                    );
                }
            }
        });
    }

    // Broadcast state snapshots at the configured interval (default 10s).
    let broadcast_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            broadcast_interval_secs.max(1),
        ));
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
    let mode = if state.paper.is_some() {
        "paper"
    } else {
        "live"
    };
    Json(json!({ "status": "ok", "mode": mode, "uptime_secs": uptime_secs }))
}

async fn get_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let subs = state
        .market_subscriptions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let risk_status = if let Some(ref risk) = state.risk {
        let r = risk.lock_or_recover();
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
        "strategy": strategy_json(&state),
    }))
}

async fn get_portfolio(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper mode not active"}));
    };

    // Use try_lock first for a fresh response; fall back to the portfolio cache
    // (populated every 2s by the background refresher task) when the signal loop
    // holds the mutex.
    let Ok(p) = paper.portfolio.try_lock() else {
        if let Ok(cached) = state.portfolio_cache.read() {
            if let Some(ref v) = *cached {
                return Json(v.clone());
            }
        }
        return Json(json!({"error": "Portfolio busy — engine processing signal", "retry": true}));
    };
    let positions: Vec<PositionJson> = p
        .positions
        .iter()
        .map(|pos| {
            let now_ts = chrono::Utc::now().timestamp();
            // Prefer event_start_time (game kickoff) for sports bets;
            // fall back to event_end_time (market resolution deadline).
            let secs_to_event = pos
                .event_start_time
                .or(pos.event_end_time)
                .map(|ts| ts - now_ts);
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
                secs_to_event,
            }
        })
        .collect();
    let fees_paid = p.total_fees_paid_usdc;
    let cash = p.cash_usdc;
    let nav = p.nav();
    let invested = p.total_invested();
    let unrealized = p.unrealized_pnl();
    let realized = p.realized_pnl();
    let closed_count = p.closed_trades.len();
    let wins = p
        .closed_trades
        .iter()
        .filter(|t| t.realized_pnl > 0.0)
        .count();
    let win_rate_pct = if closed_count > 0 {
        (wins as f64 / closed_count as f64) * 100.0
    } else {
        0.0
    };
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
        p.closed_trades
            .iter()
            .map(|t| t.scorecard.slippage_bps)
            .sum::<f64>()
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
    let per_page = params.per_page.unwrap_or(50).clamp(1, 5000);
    let total = p.closed_trades.len();
    let total_pages = total.div_ceil(per_page).max(1);
    let page = params.page.unwrap_or(1).clamp(1, total_pages);
    let skip = (page - 1) * per_page;

    // Serve newest trades first so page 1 always has the most recent results.
    let trades: Vec<ClosedTradeJson> = p
        .closed_trades
        .iter()
        .rev()
        .skip(skip)
        .take(per_page)
        .map(|t| ClosedTradeJson {
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
        })
        .collect();

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
        let log = state.activity_log.lock().unwrap_or_else(|e| e.into_inner());
        log.iter()
            .rev()
            .take(100)
            .map(|e| ActivityEntryJson {
                timestamp: e.timestamp.clone(),
                kind: format!("{:?}", e.kind),
                message: e.message.clone(),
            })
            .collect()
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
            let bids: Vec<[f64; 2]> = ob
                .bids
                .iter()
                .rev()
                .take(20)
                .map(|(&p, &s)| [p as f64 / 1000.0, s as f64 / 1000.0])
                .collect();
            let asks: Vec<[f64; 2]> = ob
                .asks
                .iter()
                .take(20)
                .map(|(&p, &s)| [p as f64 / 1000.0, s as f64 / 1000.0])
                .collect();
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
    let subs = state
        .market_subscriptions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    // Build token→title map from open positions
    let title_map: std::collections::HashMap<String, String> = if let Some(ref pe) = state.paper {
        if let Ok(p) = pe.portfolio.try_lock() {
            p.positions
                .iter()
                .filter_map(|pos| {
                    pos.market_title
                        .as_ref()
                        .map(|t: &String| (pos.token_id.clone(), t.clone()))
                })
                .collect()
        } else {
            std::collections::HashMap::new()
        }
    } else {
        std::collections::HashMap::new()
    };

    let books: Vec<serde_json::Value> = subs
        .iter()
        .map(|token_id| {
            // Prefer discovery title, fallback to open-position map.
            let market_title: Option<String> = state
                .discovery_store
                .as_ref()
                .and_then(|store| {
                    store
                        .try_read()
                        .ok()
                        .and_then(|s| s.get(token_id).and_then(|m| m.title.clone()))
                })
                .or_else(|| title_map.get(token_id).cloned());
            if let Some(ob) = state.book_store.get_book_snapshot(token_id) {
                let bids: Vec<[f64; 2]> = ob
                    .bids
                    .iter()
                    .rev()
                    .take(15)
                    .map(|(&p, &s)| [p as f64 / 1000.0, s as f64 / 1000.0])
                    .collect();
                let asks: Vec<[f64; 2]> = ob
                    .asks
                    .iter()
                    .take(15)
                    .map(|(&p, &s)| [p as f64 / 1000.0, s as f64 / 1000.0])
                    .collect();
                json!({
                    "token_id": token_id,
                    "market_title": market_title,
                    "best_bid": ob.best_bid().map(|p| p as f64 / 1000.0),
                    "best_ask": ob.best_ask().map(|p| p as f64 / 1000.0),
                    "spread_bps": ob.spread_bps(),
                    "bids": bids,
                    "asks": asks,
                })
            } else {
                json!({
                    "token_id": token_id,
                    "market_title": market_title,
                    "best_bid": null,
                    "best_ask": null,
                    "spread_bps": null,
                    "bids": [],
                    "asks": [],
                })
            }
        })
        .collect();
    Json(json!({ "orderbooks": books }))
}

async fn get_risk(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref risk) = state.risk else {
        return Json(json!({"error": "Risk manager not available"}));
    };
    let r = risk.lock_or_recover();
    let cfg = r.config();
    let stop_loss_enabled = std::env::var("STOP_LOSS_ENABLED")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let stop_loss_pct = std::env::var("STOP_LOSS_PCT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);

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
    let live_mode = std::env::var("LIVE_MODE")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    let provider = std::env::var("CUSTODIAL_PROVIDER").unwrap_or_else(|_| "none".to_string());
    let provider_ready = std::env::var("CUSTODIAL_PROVIDER_READY")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
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
    let amount = body
        .get("amount_usdc")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or("USDC")
        .to_string();
    let recipient = body
        .get("recipient")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let position_id = body.get("position_id").and_then(|v| v.as_u64());

    // Build a generic unsigned payload. For Phantom/Solana the client will
    // fill recent blockhash and sign; for EVM the client will set nonce and gas.
    let chain = std::env::var("SETTLEMENT_CHAIN").unwrap_or_else(|_| "solana".to_string());
    let decimals = std::env::var("TOKEN_DECIMALS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(6u32);
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
    let chain = body
        .get("chain")
        .and_then(|v| v.as_str())
        .unwrap_or("solana");
    let signed_tx = body.get("signed_tx").cloned().unwrap_or(json!(null));
    let raw_tx = body.get("raw_tx").cloned().unwrap_or(json!(null));
    let position_id = body.get("position_id").and_then(|v| v.as_u64());

    // Log to activity log for auditability
    let msg = format!(
        "Received signed tx for chain={}: position={:?}",
        chain, position_id
    );
    crate::activity_log::push(
        &state.activity_log,
        crate::activity_log::EntryKind::Engine,
        msg,
    );

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
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Twin not available"})),
        )
            .into_response();
    };
    let snap = twin_lock.lock().unwrap_or_else(|e| e.into_inner());
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
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "No twin snapshot yet"})),
        )
            .into_response(),
    }
}

async fn get_latency(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref tracker) = state.latency else {
        return Json(json!({"error": "Latency tracker not available"}));
    };
    let signal_summary = tracker
        .signal_age
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .summary();
    let msg_rate = tracker
        .msgs_per_sec
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .per_second();
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
        "strategy": strategy_json(&state),
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
            let r = risk.lock_or_recover();
            (
                r.daily_pnl(),
                r.is_circuit_breaker_tripped(),
                r.config().max_daily_loss_pct,
                r.config().trading_enabled,
            )
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

async fn post_pause(
    State(state): State<AppState>,
    body: Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let paused = body
        .get("paused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    state.trading_paused.store(paused, Ordering::Relaxed);
    Json(json!({ "trading_paused": paused }))
}

async fn post_strategy(
    State(state): State<AppState>,
    body: Json<serde_json::Value>,
) -> impl IntoResponse {
    let mode_raw = match body.get("mode").and_then(|v| v.as_str()) {
        Some(mode) => mode,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": "invalid params: expected mode=mirror|conservative|aggressive"}),
                ),
            )
                .into_response();
        }
    };
    let mode = match mode_raw.parse::<StrategyMode>() {
        Ok(mode) => mode,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": err}))).into_response();
        }
    };
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string());
    let force_rollback_to_mirror = body
        .get("force_rollback_to_mirror")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let source = body
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("web_api");

    if force_rollback_to_mirror && mode != StrategyMode::Mirror {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "force_rollback_to_mirror requires mode=mirror"})),
        )
            .into_response();
    }

    let result = if force_rollback_to_mirror {
        Ok(state
            .strategy_controller
            .rollback_to_mirror(reason, &format!("{source}:rollback")))
    } else {
        state
            .strategy_controller
            .switch_mode(mode, reason, source, state.strategy_live_active)
    };
    match result {
        Ok(snapshot) => {
            if force_rollback_to_mirror {
                tracing::warn!(
                    from_mode = %snapshot
                        .history
                        .last()
                        .map(|record| record.from.to_string())
                        .unwrap_or_else(|| "mirror".to_string()),
                    to_mode = %snapshot.current_mode,
                    switch_seq = snapshot.switch_seq,
                    "Strategy rollback-to-mirror applied via API"
                );
            } else {
                tracing::info!(
                    mode = %snapshot.current_mode,
                    switch_seq = snapshot.switch_seq,
                    "Strategy mode updated via API"
                );
            }
            (StatusCode::OK, Json(json!(snapshot))).into_response()
        }
        Err(err) => {
            let status = match err {
                StrategySwitchError::RuntimeSwitchDisabled
                | StrategySwitchError::LiveSwitchNotAllowed
                | StrategySwitchError::ReasonRequired => StatusCode::FORBIDDEN,
                StrategySwitchError::CooldownActive { .. } => StatusCode::TOO_MANY_REQUESTS,
            };
            (status, Json(json!({ "error": err.message() }))).into_response()
        }
    }
}

async fn post_strategy_rollback(
    State(state): State<AppState>,
    body: Json<serde_json::Value>,
) -> impl IntoResponse {
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string());
    let source = body
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("web_api")
        .to_string();

    let snapshot = state
        .strategy_controller
        .rollback_to_mirror(reason, &format!("{source}:rollback"));
    tracing::warn!(
        from_mode = %snapshot
            .history
            .last()
            .map(|record| record.from.to_string())
            .unwrap_or_else(|| "mirror".to_string()),
        to_mode = %snapshot.current_mode,
        switch_seq = snapshot.switch_seq,
        "Strategy rollback-to-mirror applied via dedicated endpoint"
    );
    (StatusCode::OK, Json(json!(snapshot))).into_response()
}

async fn post_reset_circuit_breaker(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref risk) = state.risk else {
        return Json(json!({ "error": "Risk manager not available" }));
    };
    risk.lock_or_recover().reset_circuit_breaker();
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
    let mut rm = risk.lock_or_recover();
    let cfg = rm.config_mut();
    let mut changed = Vec::new();

    if let Some(v) = body.get("max_daily_loss_pct").and_then(|v| v.as_f64()) {
        cfg.max_daily_loss_pct = v.clamp(0.01, 1.0);
        changed.push("max_daily_loss_pct");
    }
    if let Some(v) = body
        .get("max_concurrent_positions")
        .and_then(|v| v.as_u64())
    {
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
    if let Some(v) = body.get("trading_enabled").and_then(|v| v.as_bool()) {
        cfg.trading_enabled = v;
        changed.push("trading_enabled");
    }

    tracing::warn!(fields = ?changed, "Risk config updated via API");
    Json(json!({ "ok": true, "updated": changed }))
}

async fn post_sell_position(
    State(state): State<AppState>,
    Path(id): Path<usize>,
    body: Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let fraction = body
        .get("fraction")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0)
        .clamp(0.01, 1.0);

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
            risk.lock_or_recover().record_close(last_trade.realized_pnl);
        }
    }

    // Log to activity log (use AppState's shared log, not paper's private field)
    {
        let msg = format!("MANUAL SELL: pos #{id} {:.0}% — {reason}", fraction * 100.0);
        crate::activity_log::push(
            &state.activity_log,
            crate::activity_log::EntryKind::Engine,
            msg,
        );
    }

    let pnl = p
        .closed_trades
        .last()
        .map(|t| t.realized_pnl)
        .unwrap_or(0.0);
    let fees = p
        .closed_trades
        .last()
        .map(|t| t.fees_paid_usdc)
        .unwrap_or(0.0);
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
    let token_id = body
        .get("token_id")
        .and_then(|v| v.as_str())
        .unwrap_or("asset-1")
        .to_string();
    let market_title = body
        .get("market_title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let side_str = body.get("side").and_then(|v| v.as_str()).unwrap_or("BUY");
    let side = if side_str.eq_ignore_ascii_case("SELL") {
        crate::types::OrderSide::Sell
    } else {
        crate::types::OrderSide::Buy
    };
    let entry_price = body
        .get("entry_price")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);
    let usdc_size = body
        .get("usdc_size")
        .and_then(|v| v.as_f64())
        .unwrap_or(5.0);

    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper engine not available"}));
    };
    let mut p = paper.portfolio.lock().await;
    let id = p.open_position_with_meta(
        token_id.clone(),
        market_title.clone(),
        None,
        side,
        entry_price,
        usdc_size,
        "debug".to_string(),
        0.0,
        0,
        "debug",
        None,
        None,
        "debug",
        None,
    );
    let pos_json = p.positions.iter().find(|x| x.id == id).map(|pos| {
        json!({
            "id": pos.id,
            "token_id": pos.token_id,
            "market_title": pos.market_title,
            "side": pos.side.to_string(),
            "entry_price": pos.entry_price,
            "shares": pos.shares,
            "usdc_spent": pos.usdc_spent,
            "entry_fee_paid_usdc": pos.entry_fee_paid_usdc,
            "current_price": pos.current_price,
        })
    });
    Json(json!({"ok": true, "position_id": id, "position": pos_json, "cash_usdc": p.cash_usdc }))
}

async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
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
    let now_ms = chrono::Utc::now().timestamp_millis();
    let seq = state.snapshot_seq.fetch_add(1, Ordering::Relaxed);
    let uptime_secs = state.started_at.elapsed().as_secs();

    // Portfolio cache age — how stale the portfolio data is
    let portfolio_cached_at = state.portfolio_cached_at_ms.load(Ordering::Relaxed);
    let portfolio_age_ms = if portfolio_cached_at > 0 {
        (now_ms as u64).saturating_sub(portfolio_cached_at)
    } else {
        0
    };

    let mut snapshot = json!({
        "type": "snapshot",
        "timestamp_ms": now_ms,
        "snapshot_seq": seq,
        "engine_uptime_secs": uptime_secs,
        "portfolio_age_ms": portfolio_age_ms,
        "ws_connected": state.ws_live.load(Ordering::Relaxed),
        "trading_paused": state.trading_paused.load(Ordering::Relaxed),
        "messages_total": state.msg_count.load(Ordering::Relaxed),
        "strategy": strategy_json(state),
    });

    // Portfolio summary — read from the cache populated by the background
    // refresher task (which properly awaits the tokio Mutex every 2s).
    // This avoids try_lock failures when the signal loop holds the portfolio mutex.
    if let Some(ref paper) = state.paper {
        if let Ok(cached) = state.portfolio_cache.read() {
            if let Some(ref portfolio_json) = *cached {
                snapshot["portfolio"] = portfolio_json.clone();
            }
        }
        // Engine-level metrics available without locking portfolio.
        snapshot["vol_bps"] = json!(paper.vol_bps());
    }

    // Risk status
    if let Some(ref risk) = state.risk {
        let r = risk.lock_or_recover();
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
        let log = state.activity_log.lock().unwrap_or_else(|e| e.into_inner());
        let recent: Vec<serde_json::Value> = log
            .iter()
            .rev()
            .take(5)
            .map(|e| {
                json!({
                    "timestamp": e.timestamp,
                    "kind": format!("{:?}", e.kind),
                    "message": e.message,
                })
            })
            .collect();
        snapshot["recent_activity"] = json!(recent);
    }

    // Live order book summaries for all tracked tokens (6A)
    {
        let books = state.book_store.all_snapshots();
        if !books.is_empty() {
            let mut order_books = serde_json::Map::new();
            for (token_id, book) in books {
                let bid_depth: f64 = book.bids.values().map(|&s| s as f64 / 1000.0).sum();
                let ask_depth: f64 = book.asks.values().map(|&s| s as f64 / 1000.0).sum();
                let best_bid = book.bids.keys().next_back().map(|&p| p as f64 / 1000.0);
                let best_ask = book.asks.keys().next().map(|&p| p as f64 / 1000.0);
                let spread_bps = match (best_bid, best_ask) {
                    (Some(b), Some(a)) if b > 0.0 => ((a - b) / b * 10_000.0) as i64,
                    _ => 0,
                };
                let imbalance = if bid_depth + ask_depth > 0.0 {
                    (bid_depth - ask_depth) / (bid_depth + ask_depth)
                } else {
                    0.0
                };
                order_books.insert(
                    token_id,
                    json!({
                        "bid_depth": bid_depth,
                        "ask_depth": ask_depth,
                        "best_bid": best_bid,
                        "best_ask": best_ask,
                        "spread_bps": spread_bps,
                        "imbalance": imbalance,
                    }),
                );
            }
            snapshot["order_books"] = serde_json::Value::Object(order_books);
        }
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
        let recent = timestamps
            .iter()
            .filter(|&&t| now_ms - t < window_ms)
            .count();
        total_recent += recent;
        reason_counts.insert(reason.clone(), json!(recent));
    }
    drop(analytics);
    // Live risk-adjusted metrics from portfolio
    let (sharpe, sortino, fee_drag, fee_alert) = {
        match tokio::time::timeout(std::time::Duration::from_secs(2), paper.portfolio.lock()).await
        {
            Ok(p) => {
                let s = p.live_sharpe();
                let so = p.live_sortino();
                let fd = p.fee_drag_pct();
                (s, so, fd, fd > 50.0)
            }
            Err(_) => (0.0, 0.0, 0.0, false),
        }
    };
    let uptime_secs = state.started_at.elapsed().as_secs();
    // TODO: Wire render_prom() into a dedicated /metrics endpoint returning text/plain.
    // For now, include the hot metrics in the existing JSON endpoint.
    let _hot_prom = crate::hot_metrics::render_prom();
    Json(json!({
        "available": true,
        "signals_rejected_last_60s": total_recent,
        "rejection_by_reason": reason_counts,
        "uptime_secs": uptime_secs,
        "sharpe_ratio": sharpe,
        "sortino_ratio": sortino,
        "fee_drag_pct": fee_drag,
        "fee_drag_alert": fee_alert,
        "hot_signals_in": crate::hot_metrics::counters().signals_in.load(std::sync::atomic::Ordering::Relaxed),
        "hot_dedup_hits": crate::hot_metrics::counters().dedup_hits.load(std::sync::atomic::Ordering::Relaxed),
        "hot_submits_ack": crate::hot_metrics::counters().submits_ack.load(std::sync::atomic::Ordering::Relaxed),
        "hot_submits_rejected": crate::hot_metrics::counters().submits_rejected.load(std::sync::atomic::Ordering::Relaxed),
        "hot_partial_fills": crate::hot_metrics::counters().partial_fills.load(std::sync::atomic::Ordering::Relaxed),
        "hot_full_fills": crate::hot_metrics::counters().full_fills.load(std::sync::atomic::Ordering::Relaxed),
    }))
}

async fn get_rejections(
    State(state): State<AppState>,
    Query(params): Query<RejectionsQuery>,
) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({ "available": false }));
    };

    let analytics = paper.rejection_analytics_handle();
    let analytics = analytics.lock().await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let since_hours = params.since_hours.unwrap_or(24).clamp(1, 24 * 30);
    let min_ts = now_ms - (since_hours as i64 * 3_600_000);
    let limit = params.limit.unwrap_or(200).clamp(1, 5_000);
    let reason_filter = params.reason.as_deref();

    let mut filtered_events: Vec<serde_json::Value> = analytics
        .events
        .iter()
        .filter(|event| event.timestamp_ms >= min_ts)
        .filter(|event| {
            reason_filter
                .map(|reason| event.reason == reason)
                .unwrap_or(true)
        })
        .map(|event| {
            json!({
                "timestamp_ms": event.timestamp_ms,
                "reason": event.reason,
                "token_id": event.token_id,
                "side": event.side,
                "signal_price": event.signal_price,
                "signal_size": event.signal_size,
                "signal_source": event.signal_source,
            })
        })
        .collect();

    filtered_events.sort_by(|a, b| {
        let at = a["timestamp_ms"].as_i64().unwrap_or_default();
        let bt = b["timestamp_ms"].as_i64().unwrap_or_default();
        bt.cmp(&at)
    });
    filtered_events.truncate(limit);

    let mut counts_by_reason: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for event in analytics
        .events
        .iter()
        .filter(|event| event.timestamp_ms >= min_ts)
    {
        if reason_filter
            .map(|reason| event.reason == reason)
            .unwrap_or(true)
        {
            let count = counts_by_reason
                .get(event.reason.as_str())
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            counts_by_reason.insert(event.reason.clone(), json!(count + 1));
        }
    }

    Json(json!({
        "available": true,
        "schema_version": analytics.schema_version,
        "since_hours": since_hours,
        "reason_filter": reason_filter,
        "limit": limit,
        "returned": filtered_events.len(),
        "events": filtered_events,
        "counts_by_reason": counts_by_reason,
    }))
}

async fn get_fill_window(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({ "available": false }));
    };
    let snap = paper
        .fill_window
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
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
        let mut markets: Vec<serde_json::Value> = s
            .all_markets()
            .iter()
            .map(|m| {
                json!({
                    "token_id": m.token_id,
                    "title": m.title,
                    "lenses": m.discovery_lenses,
                    "viability_score": m.viability_score,
                    "conviction_boost": m.conviction_boost,
                    "smart_money_interest": m.smart_money_interest,
                    "seen_count": m.seen_count,
                })
            })
            .collect();
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
        }))
        .into_response()
    } else {
        Json(json!({ "enabled": false })).into_response()
    }
}

async fn get_bullpen_convergence(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref store) = state.convergence_store {
        let s = store.read().await;
        let summary = s.summary();
        let signals: Vec<serde_json::Value> = s
            .active_signals
            .iter()
            .map(|sig| {
                json!({
                    "market_title": sig.market,
                    "convergence_score": sig.convergence_score,
                    "net_direction": sig.net_direction,
                    "total_usd": sig.total_usd,
                    "wallet_count": sig.wallets.len(),
                })
            })
            .collect();
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

async fn get_bullpen_short_markets(State(state): State<AppState>) -> impl IntoResponse {
    let max_hours: u64 = std::env::var("BULLPEN_DISCOVER_MAX_RESOLVE_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);

    if let Some(ref store) = state.discovery_store {
        let s = store.read().await;
        let now_unix = chrono::Utc::now().timestamp();
        let cutoff = now_unix + max_hours as i64 * 3600;

        // Get convergence signals for direction enrichment (if available).
        let convergence: Vec<crate::bullpen_smart_money::ConvergenceSignal> =
            if let Some(ref conv) = state.convergence_store {
                conv.read().await.active_signals.clone()
            } else {
                vec![]
            };

        let markets: Vec<serde_json::Value> = s
            .short_term_markets(max_hours)
            .iter()
            .map(|m| {
                let secs_to_close = m.ends_at_ts.map(|ts| ts - now_unix);

                // Look up any SM convergence signal for this market.
                let conv = m.title.as_deref().and_then(|t| {
                    let tl = t.to_lowercase();
                    convergence.iter().find(|c| {
                        let sl = c.market.to_lowercase();
                        sl.contains(&tl) || tl.contains(&sl)
                    })
                });

                json!({
                    "token_id": m.token_id,
                    "title": m.title,
                    "category": m.category,
                    "ends_at_ts": m.ends_at_ts,
                    "secs_to_close": secs_to_close,
                    "viability_score": m.viability_score,
                    "smart_money_interest": m.smart_money_interest,
                    "lenses": m.discovery_lenses,
                    // SM enrichment — None if no convergence signal found.
                    "sm_direction": conv.map(|c| &c.net_direction),
                    "sm_convergence_score": conv.map(|c| c.convergence_score),
                    "sm_total_usd": conv.map(|c| c.total_usd),
                })
            })
            .collect();

        // Sort by time-to-close ascending (most urgent first).
        let mut markets = markets;
        markets.sort_by(|a, b| {
            let ta = a["secs_to_close"].as_i64().unwrap_or(i64::MAX);
            let tb = b["secs_to_close"].as_i64().unwrap_or(i64::MAX);
            ta.cmp(&tb)
        });

        Json(json!({
            "enabled": true,
            "max_hours": max_hours,
            "cutoff_unix": cutoff,
            "count": markets.len(),
            "markets": markets,
        }))
        .into_response()
    } else {
        Json(json!({ "enabled": false, "reason": "Bullpen discovery not running" })).into_response()
    }
}

// ─── Market URL resolver ─────────────────────────────────────────────────────

/// GET /api/market-url/:token_id
///
/// Resolves a Polymarket token ID to a live event URL via the Gamma API.
/// Results are cached in memory to avoid redundant API calls.
async fn get_market_url(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
) -> Json<serde_json::Value> {
    // Check cache first.
    {
        let cache = state.slug_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(url) = cache.get(&token_id) {
            return Json(json!({ "url": url, "cached": true }));
        }
    }

    // Call Gamma API.
    let gamma_url = format!(
        "https://gamma-api.polymarket.com/markets?clob_token_ids={}",
        token_id
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    match client.get(&gamma_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    // Gamma returns an array of markets.
                    // Prefer the event-level slug (works on polymarket.com),
                    // not the market-level slug which 404s.
                    let market = data.as_array().and_then(|arr| arr.first());

                    let event_slug = market
                        .and_then(|m| m.get("events"))
                        .and_then(|e| e.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|ev| {
                            ev.get("slug")
                                .or_else(|| ev.get("event_slug"))
                                .or_else(|| ev.get("eventSlug"))
                        })
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());

                    // Fallback: market-level slug (less reliable)
                    let slug = event_slug.or_else(|| {
                        market
                            .and_then(|m| {
                                m.get("market_slug")
                                    .or_else(|| m.get("slug"))
                                    .or_else(|| m.get("marketSlug"))
                            })
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string())
                    });

                    if let Some(slug) = slug {
                        let url = format!("https://polymarket.com/event/{slug}");
                        state
                            .slug_cache
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(token_id, url.clone());
                        Json(json!({ "url": url, "cached": false }))
                    } else {
                        Json(json!({ "url": null, "error": "slug not found in Gamma response" }))
                    }
                }
                Err(e) => Json(json!({ "url": null, "error": format!("JSON parse error: {e}") })),
            }
        }
        Ok(resp) => {
            Json(json!({ "url": null, "error": format!("Gamma API returned {}", resp.status()) }))
        }
        Err(e) => Json(json!({ "url": null, "error": format!("HTTP error: {e}") })),
    }
}

async fn get_pnl_attribution(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({ "available": false }));
    };
    let p = paper.portfolio.lock().await;
    if p.closed_trades.is_empty() {
        return Json(
            json!({ "available": true, "by_reason": {}, "by_category": {}, "by_side": {} }),
        );
    }

    let mut by_reason: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut by_category: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut by_side: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

    for trade in &p.closed_trades {
        // Normalise exit reason to prefix (strip per-trade values like "stop_loss@-25%")
        let reason_key = trade
            .reason
            .split('@')
            .next()
            .unwrap_or(&trade.reason)
            .to_string();
        *by_reason.entry(reason_key).or_insert(0.0) += trade.realized_pnl;

        // Detect fee category from market title (mirrors detect_fee_category heuristic)
        let title_lower = trade.market_title.as_deref().unwrap_or("").to_lowercase();
        let cat = if title_lower.contains("nfl")
            || title_lower.contains("nba")
            || title_lower.contains("nhl")
            || title_lower.contains("mlb")
            || title_lower.contains("premier league")
            || title_lower.contains("champions league")
            || title_lower.contains("soccer")
            || title_lower.contains("football")
            || title_lower.contains("basketball")
            || title_lower.contains("baseball")
            || title_lower.contains("tennis")
            || title_lower.contains("golf")
        {
            "sports"
        } else if title_lower.contains("bitcoin")
            || title_lower.contains("ethereum")
            || title_lower.contains("crypto")
            || title_lower.contains("btc")
            || title_lower.contains("eth")
        {
            "crypto"
        } else if title_lower.contains("elect")
            || title_lower.contains("presid")
            || title_lower.contains("senate")
            || title_lower.contains("congress")
            || title_lower.contains("trump")
            || title_lower.contains("biden")
            || title_lower.contains("harris")
        {
            "politics"
        } else if title_lower.contains("ukraine")
            || title_lower.contains("russia")
            || title_lower.contains("israel")
            || title_lower.contains("iran")
            || title_lower.contains("china")
            || title_lower.contains("taiwan")
            || title_lower.contains("nato")
            || title_lower.contains("war")
        {
            "geopolitics"
        } else {
            "other"
        };
        *by_category.entry(cat.to_string()).or_insert(0.0) += trade.realized_pnl;

        let side_key = format!("{:?}", trade.side).to_lowercase();
        *by_side.entry(side_key).or_insert(0.0) += trade.realized_pnl;
    }

    Json(json!({
        "available": true,
        "total_trades": p.closed_trades.len(),
        "by_reason": by_reason,
        "by_category": by_category,
        "by_side": by_side,
    }))
}

// ─── /api/backtest ───────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct BacktestRequest {
    rn1_wallet: Option<String>,
    starting_usdc: Option<f64>,
    size_multiplier: Option<f64>,
    drift_threshold: Option<f64>,
    fill_window_ms: Option<u64>,
    slippage_bps: Option<u64>,
    tick_path: Option<String>,
}

/// Run a backtest synchronously using a local tick CSV file.
/// Accepts optional overrides; falls back to env-defaults and BacktestConfig::default().
async fn post_backtest(
    State(_state): State<AppState>,
    Json(req): Json<BacktestRequest>,
) -> Json<serde_json::Value> {
    // Resolve tick file path.
    let tick_path = req
        .tick_path
        .clone()
        .or_else(|| std::env::var("TICK_RECORD_PATH").ok())
        .unwrap_or_else(|| "logs/ticks.csv".to_string());

    let ticks = match load_ticks_csv(&tick_path) {
        Ok(t) if t.is_empty() => {
            return Json(json!({ "ok": false, "error": "tick file is empty" }));
        }
        Ok(t) => t,
        Err(e) => {
            return Json(json!({ "ok": false, "error": format!("{e}") }));
        }
    };

    let default_wallet = std::env::var("RN1_WALLET")
        .or_else(|_| {
            std::env::var("TRACK_WALLETS").map(|v| {
                v.split(',')
                    .next()
                    .unwrap_or("")
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
        })
        .unwrap_or_default();

    // Build config merging request overrides with env defaults.
    let defaults = BacktestConfig::default();
    let config = BacktestConfig {
        rn1_wallet: req.rn1_wallet.unwrap_or(default_wallet),
        starting_usdc: req.starting_usdc.unwrap_or(defaults.starting_usdc),
        size_multiplier: req.size_multiplier.unwrap_or(defaults.size_multiplier),
        drift_threshold: req.drift_threshold.unwrap_or(defaults.drift_threshold),
        fill_window_ms: req.fill_window_ms.unwrap_or(defaults.fill_window_ms),
        slippage_bps: req.slippage_bps.unwrap_or(defaults.slippage_bps),
    };

    let tick_count = ticks.len();

    // Run on a blocking thread to avoid starving the async executor.
    let results = tokio::task::spawn_blocking(move || {
        let mut engine = BacktestEngine::new(config, ticks);
        engine.run()
    })
    .await;

    match results {
        Ok(r) => Json(json!({
            "ok": true,
            "tick_count": tick_count,
            "total_return_pct": r.total_return_pct,
            "sharpe_ratio": r.sharpe_ratio,
            "sortino_ratio": r.sortino_ratio,
            "max_drawdown_pct": r.max_drawdown_pct,
            "calmar_ratio": r.calmar_ratio,
            "win_rate": r.win_rate,
            "profit_factor": r.profit_factor,
            "avg_trade_duration_ms": r.avg_trade_duration_ms,
            "total_trades": r.total_trades,
            "equity_curve": r.equity_curve,
        })),
        Err(e) => Json(json!({ "ok": false, "error": format!("spawn error: {e}") })),
    }
}

// ─── /api/backtest/sweep ─────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
struct SweepAxesJson {
    size_multiplier: Option<Vec<f64>>,
    slippage_bps: Option<Vec<u64>>,
    drift_threshold: Option<Vec<f64>>,
    fill_window_ms: Option<Vec<u64>>,
}

#[derive(serde::Deserialize)]
struct SweepRequest {
    rn1_wallet: Option<String>,
    tick_path: Option<String>,
    starting_usdc: Option<f64>,
    sweep: Option<SweepAxesJson>,
}

async fn post_backtest_sweep(
    State(_state): State<AppState>,
    Json(req): Json<SweepRequest>,
) -> Json<serde_json::Value> {
    let tick_path = req
        .tick_path
        .or_else(|| std::env::var("TICK_RECORD_PATH").ok())
        .unwrap_or_else(|| "logs/ticks.csv".to_string());

    let ticks = match load_ticks_csv(&tick_path) {
        Ok(t) if t.is_empty() => {
            return Json(json!({ "ok": false, "error": "tick file is empty" }))
        }
        Ok(t) => t,
        Err(e) => return Json(json!({ "ok": false, "error": format!("{e}") })),
    };

    let default_wallet = std::env::var("RN1_WALLET").unwrap_or_default();
    let defaults = BacktestConfig::default();
    let base = BacktestConfig {
        rn1_wallet: req.rn1_wallet.unwrap_or(default_wallet),
        starting_usdc: req.starting_usdc.unwrap_or(defaults.starting_usdc),
        ..defaults
    };

    let axes_json = req.sweep.unwrap_or_default();
    let axes = SweepAxes {
        size_multiplier: axes_json.size_multiplier.unwrap_or_default(),
        slippage_bps: axes_json.slippage_bps.unwrap_or_default(),
        drift_threshold: axes_json.drift_threshold.unwrap_or_default(),
        fill_window_ms: axes_json.fill_window_ms.unwrap_or_default(),
    };

    let tick_count = ticks.len();
    let rows = tokio::task::spawn_blocking(move || run_parameter_sweep(base, ticks, axes))
        .await
        .unwrap_or_default();

    Json(json!({
        "ok": true,
        "tick_count": tick_count,
        "combinations_run": rows.len(),
        "results": rows,
    }))
}

// ─── /api/backtest/walk-forward ──────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct WalkForwardRequest {
    rn1_wallet: Option<String>,
    tick_path: Option<String>,
    starting_usdc: Option<f64>,
    size_multiplier: Option<f64>,
    drift_threshold: Option<f64>,
    fill_window_ms: Option<u64>,
    slippage_bps: Option<u64>,
    num_windows: Option<usize>,
}

async fn post_backtest_walk_forward(
    State(_state): State<AppState>,
    Json(req): Json<WalkForwardRequest>,
) -> Json<serde_json::Value> {
    let tick_path = req
        .tick_path
        .or_else(|| std::env::var("TICK_RECORD_PATH").ok())
        .unwrap_or_else(|| "logs/ticks.csv".to_string());

    let ticks = match load_ticks_csv(&tick_path) {
        Ok(t) if t.is_empty() => {
            return Json(json!({ "ok": false, "error": "tick file is empty" }))
        }
        Ok(t) => t,
        Err(e) => return Json(json!({ "ok": false, "error": format!("{e}") })),
    };

    let default_wallet = std::env::var("RN1_WALLET").unwrap_or_default();
    let defaults = BacktestConfig::default();
    let config = BacktestConfig {
        rn1_wallet: req.rn1_wallet.unwrap_or(default_wallet),
        starting_usdc: req.starting_usdc.unwrap_or(defaults.starting_usdc),
        size_multiplier: req.size_multiplier.unwrap_or(defaults.size_multiplier),
        drift_threshold: req.drift_threshold.unwrap_or(defaults.drift_threshold),
        fill_window_ms: req.fill_window_ms.unwrap_or(defaults.fill_window_ms),
        slippage_bps: req.slippage_bps.unwrap_or(defaults.slippage_bps),
    };
    let num_windows = req.num_windows.unwrap_or(5).clamp(2, 20);
    let tick_count = ticks.len();

    let (windows, aggregate) =
        tokio::task::spawn_blocking(move || run_walk_forward(config, ticks, num_windows))
            .await
            .unwrap_or_else(|_| (Vec::new(), WalkForwardAggregate::default()));

    Json(json!({
        "ok": true,
        "tick_count": tick_count,
        "num_windows": windows.len(),
        "windows": windows,
        "aggregate": aggregate,
    }))
}

// ─── Analytics: Historical Equity ────────────────────────────────────────────

/// Query parameters for GET /api/analytics/equity
#[derive(Deserialize)]
struct EquityRangeParams {
    range: Option<String>,
}

/// Response shape for a single equity data point.
#[derive(Serialize)]
struct EquityPoint {
    timestamp_ms: u64,
    nav_usdc: f64,
}

/// GET /api/analytics/equity?range=30m|1h|6h|24h
///
/// Returns the NAV curve for the requested time window.
/// Queries ClickHouse when available; falls back to the in-memory equity curve.
async fn get_analytics_equity(
    Query(params): Query<EquityRangeParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let range = params.range.as_deref().unwrap_or("30m");
    let minutes: u64 = match range {
        "1h" => 60,
        "6h" => 360,
        "24h" => 1440,
        _ => 30,
    };

    // ── Try ClickHouse first ──────────────────────────────────────────────────
    if let Some(ref url) = state.clickhouse_url {
        let client = clickhouse::Client::default().with_url(url);
        let now_ms = clickhouse_logger::now_ms();
        let cutoff_ms = now_ms.saturating_sub(minutes * 60 * 1_000);

        #[derive(clickhouse::Row, serde::Deserialize)]
        struct ChRow {
            timestamp_ms: u64,
            nav_usdc: f64,
        }

        match client
            .query(
                "SELECT timestamp_ms, nav_usdc \
                    FROM blink.equity_snapshots \
                    WHERE timestamp_ms >= ? \
                    ORDER BY timestamp_ms",
            )
            .bind(cutoff_ms)
            .fetch::<ChRow>()
        {
            Ok(mut cursor) => {
                let mut points: Vec<EquityPoint> = Vec::new();
                loop {
                    match cursor.next().await {
                        Ok(Some(row)) => points.push(EquityPoint {
                            timestamp_ms: row.timestamp_ms,
                            nav_usdc: row.nav_usdc,
                        }),
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                if !points.is_empty() {
                    return Json(
                        json!({ "source": "clickhouse", "range": range, "points": points }),
                    )
                    .into_response();
                }
            }
            Err(_) => {}
        }
    }

    // ── Fallback: in-memory equity curve ──────────────────────────────────────
    if let Some(ref paper) = state.paper {
        let p = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            paper.portfolio.lock(),
        )
        .await
        {
            Ok(guard) => guard,
            Err(_) => {
                return Json(json!({ "source": "timeout", "range": range, "points": Vec::<EquityPoint>::new() }))
                    .into_response();
            }
        };
        let cutoff_ms = clickhouse_logger::now_ms().saturating_sub(minutes * 60 * 1_000);
        let points: Vec<EquityPoint> = p
            .equity_curve
            .iter()
            .zip(p.equity_timestamps.iter())
            .filter(|(_, &ts)| ts as u64 >= cutoff_ms)
            .map(|(&nav, &ts)| EquityPoint {
                timestamp_ms: ts as u64,
                nav_usdc: nav,
            })
            .collect();
        return Json(json!({ "source": "memory", "range": range, "points": points }))
            .into_response();
    }

    let empty: Vec<EquityPoint> = Vec::new();
    Json(json!({ "source": "none", "range": range, "points": empty })).into_response()
}

// ─── Alpha status ─────────────────────────────────────────────────────────────

/// GET /api/alpha
///
/// Returns alpha sidecar analytics — signal counts, reject reasons, and P&L.
/// Returns 404 when the alpha pipeline is not enabled (ALPHA_ENABLED=true not set).
async fn get_alpha_status(State(state): State<AppState>) -> impl IntoResponse {
    let Some(ref analytics) = state.alpha_analytics else {
        return Json(json!({
            "enabled": false,
            "reason": "Alpha pipeline not enabled — set ALPHA_ENABLED=true and restart"
        }))
        .into_response();
    };

    // Gather AI positions from the paper portfolio
    let ai_positions: Vec<serde_json::Value> = if let Some(ref paper) = state.paper {
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            paper.portfolio.lock(),
        )
        .await
        {
            Ok(p) => p
                .positions
                .iter()
                .filter(|pos| pos.signal_source == "alpha")
                .map(|pos| {
                    json!({
                        "id": pos.id,
                        "token_id": pos.token_id,
                        "market_title": pos.market_title,
                        "side": pos.side.to_string(),
                        "entry_price": pos.entry_price,
                        "current_price": pos.current_price,
                        "shares": pos.shares,
                        "usdc_spent": pos.usdc_spent,
                        "unrealized_pnl": pos.unrealized_pnl(),
                        "unrealized_pnl_pct": pos.unrealized_pnl_pct(),
                        "analysis_id": pos.analysis_id,
                        "duration_secs": pos.opened_at.elapsed().as_secs(),
                        "opened_at": pos.opened_at_wall.to_rfc3339(),
                    })
                })
                .collect(),
            Err(_) => vec![],
        }
    } else {
        vec![]
    };

    // Gather AI closed trades
    let ai_closed_trades: Vec<serde_json::Value> = if let Some(ref paper) = state.paper {
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            paper.portfolio.lock(),
        )
        .await
        {
            Ok(p) => p
                .closed_trades
                .iter()
                .filter(|t| t.signal_source == "alpha")
                .rev()
                .take(20)
                .map(|t| {
                    json!({
                        "token_id": t.token_id,
                        "market_title": t.market_title,
                        "side": t.side.to_string(),
                        "entry_price": t.entry_price,
                        "exit_price": t.exit_price,
                        "realized_pnl": t.realized_pnl,
                        "fees_paid_usdc": t.fees_paid_usdc,
                        "reason": t.reason,
                        "duration_secs": t.duration_secs,
                        "analysis_id": t.analysis_id,
                        "closed_at": t.closed_at_wall.to_rfc3339(),
                    })
                })
                .collect(),
            Err(_) => vec![],
        }
    } else {
        vec![]
    };

    let a = analytics.lock().unwrap_or_else(|e| e.into_inner());

    // Update unrealized P&L for open AI positions in signal records
    // (done inline to avoid extra lock acquisitions)
    let mut unrealized_total = 0.0;
    for pos_json in &ai_positions {
        if let Some(upnl) = pos_json.get("unrealized_pnl").and_then(|v| v.as_f64()) {
            unrealized_total += upnl;
        }
    }

    Json(json!({
        "enabled": true,
        // Core counters
        "signals_received": a.signals_received,
        "signals_accepted": a.signals_accepted,
        "signals_rejected": a.signals_rejected,
        "accept_rate_pct": if a.signals_received > 0 {
            (a.signals_accepted as f64 / a.signals_received as f64) * 100.0
        } else { 0.0 },
        "reject_reasons": a.reject_reasons,
        // P&L
        "realized_pnl_usdc": a.realized_pnl_usdc,
        "unrealized_pnl_usdc": unrealized_total,
        // Position counts
        "positions_opened": a.positions_opened,
        "positions_closed": a.positions_closed,
        // Cycle info
        "cycles_completed": a.cycles_completed,
        "last_cycle_at": a.last_cycle_at,
        "last_cycle_markets_scanned": a.last_cycle_markets_scanned,
        "last_cycle_markets_analyzed": a.last_cycle_markets_analyzed,
        "last_cycle_signals_generated": a.last_cycle_signals_generated,
        "last_cycle_signals_submitted": a.last_cycle_signals_submitted,
        "last_cycle_duration_secs": a.last_cycle_duration_secs,
        "last_cycle_top_markets": a.last_cycle_top_markets,
        // NEW: Signal history (last 50)
        "signal_history": a.signal_history,
        // NEW: Cycle history (last 30)
        "cycle_history": a.cycle_history,
        // NEW: Live AI positions
        "ai_positions": ai_positions,
        // NEW: AI closed trades
        "ai_closed_trades": ai_closed_trades,
        // NEW: Performance metrics
        "performance": {
            "win_count": a.win_count,
            "loss_count": a.loss_count,
            "win_rate_pct": a.win_rate_pct(),
            "avg_pnl_per_trade": a.avg_pnl_per_trade(),
            "best_trade_pnl": a.best_trade_pnl,
            "worst_trade_pnl": a.worst_trade_pnl,
            "total_fees_paid": a.total_fees_paid,
        },
        // Calibration data from prediction memory
        "calibration": a.calibration,
    }))
    .into_response()
}

/// GET /api/alpha/calibration
///
/// Returns calibration data from the Alpha AI prediction memory system.
/// Updated periodically by the Python sidecar via `report_alpha_calibration` RPC.
async fn get_alpha_calibration(State(state): State<AppState>) -> impl IntoResponse {
    let Some(ref analytics) = state.alpha_analytics else {
        return Json(json!({
            "enabled": false,
            "reason": "Alpha pipeline not enabled"
        }))
        .into_response();
    };

    let a = analytics.lock().unwrap_or_else(|e| e.into_inner());
    match &a.calibration {
        Some(data) => Json(json!({
            "enabled": true,
            "has_data": true,
            "calibration": data,
        }))
        .into_response(),
        None => Json(json!({
            "enabled": true,
            "has_data": false,
            "calibration": null,
            "reason": "No calibration data yet — waiting for predictions to resolve"
        }))
        .into_response(),
    }
}

async fn get_project_inventory() -> Json<serde_json::Value> {
    let candidates = [
        "../docs/generated/project-inventory.json",
        "docs/generated/project-inventory.json",
    ];

    for path in candidates {
        if let Ok(content) = std::fs::read_to_string(path) {
            return match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(payload) => Json(payload),
                Err(err) => Json(json!({
                    "available": false,
                    "error": format!("project-inventory.json is invalid JSON: {err}"),
                    "path": path,
                    "generate_command": ".\\scripts\\generate-project-inventory.ps1",
                })),
            };
        }
    }

    Json(json!({
        "available": false,
        "error": "Project inventory is not generated yet",
        "paths_checked": candidates,
        "generate_command": ".\\scripts\\generate-project-inventory.ps1",
    }))
}

/// Per-gate rejection analytics — shows which gates are blocking signals
/// and how often, enabling remote diagnosis of overly aggressive filters.
async fn get_gates(State(state): State<AppState>) -> Json<serde_json::Value> {
    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper engine not available"}));
    };

    let analytics = paper.rejection_analytics_handle();
    let ra = analytics.lock().await;

    let now_secs = chrono::Utc::now().timestamp();
    let one_hour_ago = now_secs - 3600;
    let twenty_four_hours_ago = now_secs - 86400;

    let mut gates: Vec<serde_json::Value> = Vec::new();
    let mut total_1h: usize = 0;
    let mut total_24h: usize = 0;

    for (reason, timestamps) in &ra.reasons {
        let count_1h = timestamps.iter().filter(|&&t| t >= one_hour_ago).count();
        let count_24h = timestamps
            .iter()
            .filter(|&&t| t >= twenty_four_hours_ago)
            .count();
        let count_all = timestamps.len();
        let last_triggered = timestamps.iter().max().copied();

        total_1h += count_1h;
        total_24h += count_24h;

        gates.push(json!({
            "gate": reason,
            "rejections_1h": count_1h,
            "rejections_24h": count_24h,
            "rejections_total": count_all,
            "last_triggered_epoch": last_triggered,
        }));
    }

    // Sort by 1h count descending — most active blockers first
    gates.sort_by(|a, b| {
        let a_count = a["rejections_1h"].as_u64().unwrap_or(0);
        let b_count = b["rejections_1h"].as_u64().unwrap_or(0);
        b_count.cmp(&a_count)
    });

    Json(json!({
        "total_rejections_1h": total_1h,
        "total_rejections_24h": total_24h,
        "gates": gates,
    }))
}

#[cfg(test)]
mod tests {
    use super::{post_strategy, post_strategy_rollback, AppState};
    use crate::activity_log::new_activity_log;
    use crate::order_book::OrderBookStore;
    use crate::strategy::{StrategyController, StrategyControllerConfig, StrategyMode};
    use axum::body::to_bytes;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Json;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Arc, Mutex};
    use tokio::sync::broadcast;

    fn make_test_state(controller: StrategyController, strategy_live_active: bool) -> AppState {
        let (broadcast_tx, _) = broadcast::channel(16);
        AppState {
            ws_live: Arc::new(AtomicBool::new(false)),
            trading_paused: Arc::new(AtomicBool::new(false)),
            msg_count: Arc::new(AtomicU64::new(0)),
            book_store: Arc::new(OrderBookStore::new()),
            activity_log: new_activity_log(),
            paper: None,
            risk: None,
            twin_snapshot: None,
            ws_health: None,
            latency: None,
            market_subscriptions: Arc::new(Mutex::new(Vec::new())),
            broadcast_tx,
            started_at: Arc::new(std::time::Instant::now()),
            provider: None,
            live_engine: None,
            bullpen: None,
            discovery_store: None,
            convergence_store: None,
            slug_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
            portfolio_cache: Arc::new(std::sync::RwLock::new(None)),
            clickhouse_url: None,
            snapshot_seq: Arc::new(AtomicU64::new(0)),
            portfolio_cached_at_ms: Arc::new(AtomicU64::new(0)),
            alpha_analytics: None,
            strategy_controller: Arc::new(controller),
            strategy_live_active,
        }
    }

    #[tokio::test]
    async fn post_strategy_rejects_invalid_mode() {
        let controller = StrategyController::new(StrategyControllerConfig::with_defaults(
            StrategyMode::Mirror,
            true,
            true,
            0,
            false,
        ));
        let state = make_test_state(controller, false);
        let response = post_strategy(
            State(state),
            Json(json!({"mode": "invalid", "reason": "test"})),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_strategy_maps_runtime_disabled_to_forbidden() {
        let controller = StrategyController::new(StrategyControllerConfig::with_defaults(
            StrategyMode::Mirror,
            false,
            true,
            0,
            false,
        ));
        let state = make_test_state(controller, false);
        let response = post_strategy(
            State(state),
            Json(json!({"mode": "conservative", "reason": "test"})),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should be readable");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response should be valid JSON");
        assert!(payload["error"]
            .as_str()
            .unwrap_or_default()
            .contains("runtime switching is disabled"));
    }

    #[tokio::test]
    async fn post_strategy_maps_cooldown_to_too_many_requests() {
        let controller = StrategyController::new(StrategyControllerConfig::with_defaults(
            StrategyMode::Mirror,
            true,
            true,
            300,
            false,
        ));
        let state = make_test_state(controller, false);

        let ok_response = post_strategy(
            State(state.clone()),
            Json(json!({"mode": "conservative", "reason": "initial"})),
        )
        .await
        .into_response();
        assert_eq!(ok_response.status(), StatusCode::OK);

        let cooldown_response = post_strategy(
            State(state),
            Json(json!({"mode": "aggressive", "reason": "second-switch"})),
        )
        .await
        .into_response();
        assert_eq!(cooldown_response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn post_strategy_force_rollback_bypasses_runtime_disabled() {
        let controller = StrategyController::new(StrategyControllerConfig::with_defaults(
            StrategyMode::Aggressive,
            false,
            false,
            300,
            true,
        ));
        let state = make_test_state(controller, true);

        let response = post_strategy(
            State(state),
            Json(json!({
                "mode": "mirror",
                "force_rollback_to_mirror": true
            })),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should be readable");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response should be valid JSON");
        assert_eq!(payload["current_mode"], "mirror");
    }

    #[tokio::test]
    async fn post_strategy_rollback_endpoint_switches_to_mirror() {
        let controller = StrategyController::new(StrategyControllerConfig::with_defaults(
            StrategyMode::Conservative,
            false,
            false,
            300,
            true,
        ));
        let state = make_test_state(controller, true);

        let response = post_strategy_rollback(State(state), Json(json!({})))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should be readable");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response should be valid JSON");
        assert_eq!(payload["current_mode"], "mirror");
    }
}
