//! Axum-based web server for the Blink Engine dashboard UI.
//!
//! Provides REST endpoints and a WebSocket feed for real-time engine state.
//! Activated via `WEB_UI=true` environment variable.

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use axum::{
    extract::ws::{Message, WebSocket},
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
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
use crate::latency_tracker::LatencyTracker;
use crate::live_engine::LiveEngine;
use crate::order_book::OrderBookStore;
use crate::paper_engine::PaperEngine;
use crate::paper_portfolio::{ClosedTrade, PaperPortfolio};
use crate::postgres_logger;
use crate::risk_manager::RiskManager;
use crate::strategy::{StrategyController, StrategyMode, StrategySwitchError};
use crate::timed_mutex::TimedMutex;
use crate::ws_client::WsHealthMetrics;

type SlugCache = Arc<Mutex<std::collections::HashMap<String, String>>>;

const POLYMARKET_PUSD_PROXY: &str = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB";
const DATA_API_PAGE_LIMIT: usize = 500;
const DEFAULT_POLYGON_RPCS: &[&str] = &[
    "https://polygon-bor-rpc.publicnode.com",
    "https://polygon.drpc.org",
    "https://1rpc.io/matic",
];

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
    signal_source: String,
    analysis_id: Option<String>,
}

#[derive(Serialize)]
struct LiveExecutionJson {
    transaction_hash: Option<String>,
    token_id: String,
    condition_id: Option<String>,
    market_title: Option<String>,
    market_outcome: Option<String>,
    side: String,
    price: f64,
    shares: f64,
    usdc_size: f64,
    timestamp: i64,
    traded_at: String,
    execution_type: String,
    source: String,
}

struct ExchangePositionsSnapshot {
    value_usdc: f64,
    initial_value_usdc: f64,
    cash_pnl_usdc: f64,
    positions_count: usize,
    preview: Vec<serde_json::Value>,
    open_positions: Vec<PositionJson>,
    asset_ids: HashSet<String>,
    checked_at_ms: u64,
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

    // Portfolio cache refresher. In live mode the live engine owns the same
    // PaperPortfolio-shaped accounting object, so feed the dashboard from it.
    let portfolio_source = state
        .paper
        .as_ref()
        .map(|paper| Arc::clone(&paper.portfolio))
        .or_else(|| {
            state
                .live_engine
                .as_ref()
                .map(|live| Arc::clone(&live.portfolio))
        });
    if let Some(portfolio) = portfolio_source {
        let cache = Arc::clone(&state.portfolio_cache);
        let started_at = Arc::clone(&state.started_at);
        let cached_at = Arc::clone(&state.portfolio_cached_at_ms);
        let cache_is_live = state.live_engine.is_some();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
            loop {
                interval.tick().await;
                let p = match tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    portfolio.lock(),
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(_) => continue,
                };
                let uptime_secs = started_at.elapsed().as_secs();
                let mut portfolio_json = build_portfolio_json(&p, uptime_secs, 300);
                drop(p);
                if cache_is_live {
                    portfolio_json = mark_live_cache_unverified(portfolio_json);
                }
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

    // Broadcast state snapshots
    let broadcast_state = state.clone();
    tokio::spawn(async move {
        let interval_ms = if broadcast_interval_secs <= 1 {
            250
        } else {
            broadcast_interval_secs * 1000
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
        loop {
            interval.tick().await;
            if let Ok(snapshot) = build_snapshot(&broadcast_state).await {
                let _ = broadcast_state.broadcast_tx.send(snapshot);
            }
        }
    });

    let _ = axum::serve(listener, router).await;
}

pub fn build_router(state: AppState, static_dir: Option<String>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    let api = Router::new()
        .route("/health", get(get_health))
        .route("/api/status", get(get_status))
        .route("/api/geoblock", get(get_geoblock))
        .route("/api/portfolio", get(get_portfolio))
        .route("/api/history", get(get_history))
        .route("/api/live/history", get(get_live_history))
        .route("/api/live/executions", get(get_live_executions))
        .route("/api/activity", get(get_activity))
        .route("/api/rejections", get(get_rejections))
        .route("/api/orderbook/{token_id}", get(get_orderbook))
        .route("/api/orderbooks", get(get_all_orderbooks))
        .route("/api/market-url/{token_id}", get(get_market_url))
        .route("/api/market-meta/{token_id}", get(get_market_meta))
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
        .route("/api/emergency_stop", post(post_emergency_stop))
        .route("/api/emergency-stop", post(post_emergency_stop))
        .route(
            "/api/risk/reset_circuit_breaker",
            post(post_reset_circuit_breaker),
        )
        .route("/api/config", post(post_update_config))
        .route("/api/debug/seed_position", post(post_seed_position))
        .route("/api/positions/{id}/sell", post(post_sell_position))
        .route("/api/metrics", get(get_metrics))
        .route("/api/fill-window", get(get_fill_window))
        .route("/api/analytics/equity", get(get_analytics_equity))
        .route("/api/analytics/quant", get(get_analytics_quant))
        .route("/api/pnl-attribution", get(get_pnl_attribution))
        .route("/api/bullpen/health", get(get_bullpen_health))
        .route("/api/bullpen/discovery", get(get_bullpen_discovery))
        .route("/api/bullpen/convergence", get(get_bullpen_convergence))
        .route("/api/backtest", post(post_backtest))
        .route("/api/backtest/sweep", post(post_backtest_sweep))
        .route(
            "/api/backtest/walk-forward",
            post(post_backtest_walk_forward),
        )
        .route("/api/alpha", get(get_alpha_status))
        .route("/api/alpha/calibration", get(get_alpha_calibration))
        .route("/api/project-inventory", get(get_project_inventory))
        .route("/api/gates", get(get_gates))
        .route("/ws", get(ws_handler));

    Router::new()
        .merge(api)
        .fallback_service(
            static_dir
                .map(|dir| {
                    ServeDir::new(dir.clone())
                        .not_found_service(ServeFile::new(format!("{}/index.html", dir)))
                })
                .unwrap_or_else(|| {
                    ServeDir::new("static/ui")
                        .not_found_service(ServeFile::new("static/ui/index.html"))
                }),
        )
        .layer(cors)
        .with_state(state)
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

fn build_portfolio_json(
    p: &PaperPortfolio,
    uptime_secs: u64,
    _limit_trades: usize,
) -> serde_json::Value {
    let positions: Vec<PositionJson> = p
        .positions
        .iter()
        .map(|pos| {
            let now_ts = chrono::Utc::now().timestamp();
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

    let attempts = (p.filled_orders + p.aborted_orders + p.skipped_orders).max(1) as f64;
    let fill_rate_pct = (p.filled_orders as f64 / attempts) * 100.0;
    let reject_rate_pct = ((p.skipped_orders + p.aborted_orders) as f64 / attempts) * 100.0;
    let avg_slippage_bps = if p.closed_trades.is_empty() {
        0.0
    } else {
        p.closed_trades
            .iter()
            .map(|t| t.scorecard.slippage_bps)
            .sum::<f64>()
            / p.closed_trades.len() as f64
    };

    let wins = p
        .closed_trades
        .iter()
        .filter(|t| t.realized_pnl > 0.0)
        .count();
    let win_rate_pct = if !p.closed_trades.is_empty() {
        (wins as f64 / p.closed_trades.len() as f64) * 100.0
    } else {
        0.0
    };

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
        "equity_curve": p.equity_curve,
        "equity_timestamps": p.equity_timestamps,
        "fill_rate_pct": fill_rate_pct,
        "reject_rate_pct": reject_rate_pct,
        "avg_slippage_bps": avg_slippage_bps,
        "win_rate_pct": win_rate_pct,
        "uptime_secs": uptime_secs,
    })
}

fn mark_live_cache_unverified(mut value: serde_json::Value) -> serde_json::Value {
    let Some(obj) = value.as_object_mut() else {
        return value;
    };

    let blink_cash = obj.get("cash_usdc").cloned().unwrap_or(json!(0.0));
    let blink_nav = obj.get("nav_usdc").cloned().unwrap_or(json!(0.0));

    obj.insert("mode".to_string(), json!("live"));
    obj.insert(
        "accounting_source".to_string(),
        json!("live_ws_cache_unverified"),
    );
    obj.insert("balance_source".to_string(), json!("unverified"));
    obj.insert("cash_source".to_string(), json!("unverified"));
    obj.insert("reality_status".to_string(), json!("unverified"));
    obj.insert(
        "reality_issues".to_string(),
        json!(["live_wallet_truth_requires_api_poll"]),
    );
    obj.insert("wallet_truth_verified".to_string(), json!(false));
    obj.insert("exchange_positions_verified".to_string(), json!(false));
    obj.insert("onchain_cash_verified".to_string(), json!(false));
    obj.insert("blink_cash_usdc".to_string(), blink_cash);
    obj.insert("blink_nav_usdc".to_string(), blink_nav);
    obj.insert("cash_usdc".to_string(), serde_json::Value::Null);
    obj.insert("nav_usdc".to_string(), serde_json::Value::Null);
    obj.insert("wallet_nav_usdc".to_string(), serde_json::Value::Null);
    obj.insert("invested_usdc".to_string(), json!(0.0));
    obj.insert("unrealized_pnl_usdc".to_string(), json!(0.0));
    obj.insert("wallet_open_pnl_usdc".to_string(), serde_json::Value::Null);
    obj.insert(
        "wallet_unrealized_pnl_usdc".to_string(),
        serde_json::Value::Null,
    );
    obj.insert(
        "wallet_position_value_usdc".to_string(),
        serde_json::Value::Null,
    );
    obj.insert(
        "wallet_position_initial_value_usdc".to_string(),
        serde_json::Value::Null,
    );
    obj.insert("wallet_positions_count".to_string(), json!(0));
    obj.insert("exchange_positions_count".to_string(), json!(0));
    obj.insert("open_positions".to_string(), json!([]));
    obj.insert("equity_curve".to_string(), json!([]));
    obj.insert("equity_timestamps".to_string(), json!([]));
    value
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

async fn get_geoblock() -> Json<serde_json::Value> {
    if !crate::geo_guard::guard_enabled() {
        return Json(json!({
            "guard_enabled": false,
            "launch_status": "DISABLED_KEEP_KILL_SWITCH_OFF",
        }));
    }
    match crate::geo_guard::check_geoblock().await {
        Ok(status) => Json(json!({
            "guard_enabled": true,
            "launch_status": if status.blocked { "BLOCKED_KEEP_KILL_SWITCH_OFF" } else { "ELIGIBLE" },
            "geoblock": status.public_json(),
        })),
        Err(e) => Json(json!({
            "guard_enabled": true,
            "launch_status": "UNVERIFIED_KEEP_KILL_SWITCH_OFF",
            "error": e.to_string(),
        })),
    }
}

async fn get_portfolio(State(state): State<AppState>) -> Json<serde_json::Value> {
    if state.live_engine.is_some() {
        return get_live_portfolio(State(state)).await;
    }

    let portfolio = state
        .paper
        .as_ref()
        .map(|paper| Arc::clone(&paper.portfolio));
    let Some(portfolio) = portfolio else {
        return Json(json!({"error": "Portfolio not active"}));
    };

    // Use try_lock first for a fresh response; fall back to the portfolio cache
    // (populated every 2s by the background refresher task) when the signal loop
    // holds the mutex.
    let Ok(p) = portfolio.try_lock() else {
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
    let is_live = state.live_engine.is_some();

    Json(json!({
        "mode": if is_live { "live" } else { "paper" },
        "accounting_source": if is_live { "exchange_confirmed_fills_only" } else { "paper_simulation" },
        "balance_source": if is_live { "onchain_pusd_seed_plus_confirmed_exchange_fills" } else { "paper_simulation" },
        "confirmed_only": is_live,
        "queued_orders_affect_nav": false,
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
    range: Option<String>,
}

async fn get_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let range = params.range.as_deref().unwrap_or("all");
    let per_page = params.per_page.unwrap_or(50).clamp(1, 5000);
    let page = params.page.unwrap_or(1);

    // In live mode the History tab is an exchange-backed ledger. Do not let
    // paper/backtest/DB rows leak into the current live history surface.
    if let Some(ref live) = state.live_engine {
        let p = live.portfolio.lock().await;
        return Json(history_response_from_closed_trades(
            &p.closed_trades,
            range,
            page,
            per_page,
            "live_memory",
        ));
    }

    // ── Attempt Database Query (Postgres) ─────────────────────────────────────
    if let Some(ref url) = state.clickhouse_url {
        let minutes: Option<u64> = match range {
            "24h" => Some(1440),
            "7d" => Some(10080),
            "30d" => Some(43200),
            _ => None,
        };

        match fetch_history_from_db(url, minutes, page, per_page).await {
            Ok((trades, total_count)) => {
                let total_pages = (total_count as f64 / per_page as f64).ceil() as i64;
                let total_pages = total_pages.max(1);
                return Json(json!({
                    "trades": trades,
                    "total": total_count,
                    "page": page,
                    "per_page": per_page,
                    "total_pages": total_pages,
                    "source": "postgres"
                }));
            }
            Err(e) => {
                tracing::warn!(err = %e, "get_history: DB query failed — falling back to memory");
            }
        }
    }

    // ── Fallback: in-memory trades ────────────────────────────────────────────
    let Some(ref paper) = state.paper else {
        return Json(json!({"error": "Paper mode not active"}));
    };

    let Ok(p) = paper.portfolio.try_lock() else {
        return Json(json!({"error": "Portfolio busy", "retry": true}));
    };

    Json(history_response_from_closed_trades(
        &p.closed_trades,
        range,
        page,
        per_page,
        "paper_memory",
    ))
}

async fn get_live_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let range = params.range.as_deref().unwrap_or("all");
    let per_page = params.per_page.unwrap_or(50).clamp(1, 5000);
    let page = params.page.unwrap_or(1);

    let Some(ref live) = state.live_engine else {
        return Json(json!({
            "trades": [],
            "total": 0,
            "page": 1,
            "per_page": per_page,
            "total_pages": 1,
            "source": "live_unavailable",
            "range": range,
        }));
    };

    let p = live.portfolio.lock().await;
    Json(history_response_from_closed_trades(
        &p.closed_trades,
        range,
        page,
        per_page,
        "live_memory",
    ))
}

async fn get_live_executions(Query(params): Query<HistoryQuery>) -> Json<serde_json::Value> {
    let range = params.range.as_deref().unwrap_or("all");
    let per_page = params.per_page.unwrap_or(50).clamp(1, 5000);
    let page = params.page.unwrap_or(1);

    let Some(user) = std::env::var("POLYMARKET_FUNDER_ADDRESS").ok() else {
        return Json(json!({
            "executions": [],
            "total": 0,
            "page": 1,
            "per_page": per_page,
            "total_pages": 1,
            "source": "live_wallet_unavailable",
            "range": range,
            "reality_status": "unverified",
            "truth_checked_at_ms": null,
            "reality_issues": ["missing_POLYMARKET_FUNDER_ADDRESS"],
        }));
    };

    let max_executions = page.saturating_mul(per_page).clamp(1, 10_000);
    let executions = match fetch_polymarket_activity_executions(&user, max_executions).await {
        Some(executions) => executions,
        None => {
            return Json(json!({
                "executions": [],
                "total": 0,
                "page": 1,
                "per_page": per_page,
                "total_pages": 1,
                "source": "live_wallet_unverified",
                "range": range,
                "reality_status": "unverified",
                "truth_checked_at_ms": null,
                "reality_issues": ["polymarket_activity_unverified"],
            }));
        }
    };

    Json(live_executions_response(
        executions,
        range,
        page,
        per_page,
        "polymarket_data_api_activity",
    ))
}

fn history_response_from_closed_trades(
    closed_trades: &[ClosedTrade],
    range: &str,
    page: usize,
    per_page: usize,
    source: &str,
) -> serde_json::Value {
    let cutoff = history_cutoff(range);
    let filtered: Vec<&ClosedTrade> = closed_trades
        .iter()
        .filter(|t| cutoff.map(|c| t.closed_at_wall >= c).unwrap_or(true))
        .collect();

    let total = filtered.len();
    let total_pages = ((total as f64 / per_page as f64).ceil() as usize).max(1);
    let page = page.clamp(1, total_pages);
    let skip = (page - 1) * per_page;
    let trades: Vec<ClosedTradeJson> = filtered
        .into_iter()
        .rev()
        .skip(skip)
        .take(per_page)
        .map(closed_trade_json)
        .collect();

    json!({
        "trades": trades,
        "total": total,
        "page": page,
        "per_page": per_page,
        "total_pages": total_pages,
        "source": source,
        "range": range,
    })
}

fn history_cutoff(range: &str) -> Option<chrono::DateTime<chrono::Local>> {
    match range {
        "24h" => Some(chrono::Local::now() - chrono::Duration::hours(24)),
        "7d" => Some(chrono::Local::now() - chrono::Duration::days(7)),
        "30d" => Some(chrono::Local::now() - chrono::Duration::days(30)),
        _ => None,
    }
}

fn history_cutoff_unix_secs(range: &str) -> Option<i64> {
    history_cutoff(range).map(|dt| dt.timestamp())
}

async fn fetch_polymarket_activity_executions(
    user: &str,
    max_items: usize,
) -> Option<Vec<LiveExecutionJson>> {
    let max_items = max_items.clamp(1, data_api_max_items());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for offset in (0..max_items).step_by(DATA_API_PAGE_LIMIT) {
        let limit = DATA_API_PAGE_LIMIT.min(max_items - offset);
        let url = format!(
            "https://data-api.polymarket.com/activity?user={user}&limit={limit}&offset={offset}"
        );
        let body = fetch_data_api_json_with_retry(&url, 2_000).await?;
        let page_entries = data_api_entries_from_body(body);
        let page_len = page_entries.len();
        let mut added = 0usize;

        for entry in page_entries {
            if seen.insert(data_api_entry_key(&entry)) {
                entries.push(entry);
                added += 1;
            }
        }

        if page_len < limit || added == 0 {
            break;
        }
    }

    Some(live_executions_from_activity_entries(&entries))
}

fn data_api_max_items() -> usize {
    std::env::var("BLINK_DATA_API_MAX_ITEMS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(DATA_API_PAGE_LIMIT, 50_000))
        .unwrap_or(10_000)
}

async fn fetch_data_api_json_with_retry(url: &str, timeout_ms: u64) -> Option<serde_json::Value> {
    let attempts = std::env::var("BLINK_DATA_API_RETRIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(1, 5))
        .unwrap_or(2);
    let client = reqwest::Client::new();

    for attempt in 1..=attempts {
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            client.get(url).header("accept", "application/json").send(),
        )
        .await;

        match response {
            Ok(Ok(resp)) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(body) => return Some(body),
                    Err(e) => {
                        tracing::warn!(attempt, attempts, err = %e, "Data API response was not JSON");
                    }
                }
            }
            Ok(Ok(resp)) => {
                tracing::warn!(
                    attempt,
                    attempts,
                    status = %resp.status(),
                    "Data API returned non-success status"
                );
            }
            Ok(Err(e)) => {
                tracing::warn!(attempt, attempts, err = %e, "Data API request failed");
            }
            Err(_) => {
                tracing::warn!(attempt, attempts, timeout_ms, "Data API request timed out");
            }
        }

        if attempt < attempts {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }

    None
}

#[cfg(test)]
fn live_executions_from_activity_body(body: serde_json::Value) -> Vec<LiveExecutionJson> {
    let entries = data_api_entries_from_body(body);
    live_executions_from_activity_entries(&entries)
}

fn live_executions_from_activity_entries(entries: &[serde_json::Value]) -> Vec<LiveExecutionJson> {
    let mut executions = entries
        .iter()
        .filter(|entry| {
            json_text(entry, &["type"])
                .map(|kind| kind.eq_ignore_ascii_case("TRADE"))
                .unwrap_or(false)
        })
        .map(live_execution_json)
        .collect::<Vec<_>>();
    executions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    executions
}

fn data_api_entries_from_body(body: serde_json::Value) -> Vec<serde_json::Value> {
    if let Some(arr) = body.as_array() {
        arr.clone()
    } else if let Some(arr) = body.get("data").and_then(|v| v.as_array()) {
        arr.clone()
    } else {
        body.get("activity")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }
}

fn data_api_entry_key(entry: &serde_json::Value) -> String {
    let tx = json_text(entry, &["transactionHash", "transaction_hash", "hash"])
        .unwrap_or_else(|| "no_tx".to_string());
    let asset = json_text(entry, &["asset", "token_id", "tokenId", "conditionId"])
        .unwrap_or_else(|| "no_asset".to_string());
    let timestamp = json_i64(entry, &["timestamp", "time"]).unwrap_or_default();
    let side = json_text(entry, &["side"]).unwrap_or_else(|| "no_side".to_string());
    let kind = json_text(entry, &["type"]).unwrap_or_else(|| "no_type".to_string());
    format!("{tx}:{asset}:{timestamp}:{side}:{kind}")
}

fn live_executions_response(
    executions: Vec<LiveExecutionJson>,
    range: &str,
    page: usize,
    per_page: usize,
    source: &str,
) -> serde_json::Value {
    let cutoff = history_cutoff_unix_secs(range);
    let filtered = executions
        .into_iter()
        .filter(|execution| cutoff.map(|c| execution.timestamp >= c).unwrap_or(true))
        .collect::<Vec<_>>();
    let total = filtered.len();
    let total_pages = ((total as f64 / per_page as f64).ceil() as usize).max(1);
    let page = page.clamp(1, total_pages);
    let skip = (page - 1) * per_page;
    let executions = filtered
        .into_iter()
        .skip(skip)
        .take(per_page)
        .collect::<Vec<_>>();

    json!({
        "executions": executions,
        "total": total,
        "page": page,
        "per_page": per_page,
        "total_pages": total_pages,
        "source": source,
        "range": range,
        "reality_status": "matched",
        "truth_checked_at_ms": postgres_logger::now_ms(),
    })
}

fn live_execution_json(entry: &serde_json::Value) -> LiveExecutionJson {
    let timestamp = json_i64(entry, &["timestamp", "time"]).unwrap_or(0);
    let traded_at = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    LiveExecutionJson {
        transaction_hash: json_text(entry, &["transactionHash", "transaction_hash", "hash"]),
        token_id: json_text(entry, &["asset", "token_id", "tokenId", "conditionId"])
            .unwrap_or_default(),
        condition_id: json_text(entry, &["conditionId", "condition_id"]),
        market_title: json_text(entry, &["title", "market", "eventTitle"]),
        market_outcome: json_text(entry, &["outcome", "side"]),
        side: json_text(entry, &["side"])
            .unwrap_or_else(|| "UNKNOWN".to_string())
            .to_ascii_uppercase(),
        price: json_f64(entry, &["price", "avgPrice", "avg_price"]).unwrap_or(0.0),
        shares: json_f64(entry, &["size", "tokens", "quantity"]).unwrap_or(0.0),
        usdc_size: json_f64(entry, &["usdcSize", "usdc_size", "value"]).unwrap_or(0.0),
        timestamp,
        traded_at,
        execution_type: json_text(entry, &["type"]).unwrap_or_else(|| "TRADE".to_string()),
        source: "polymarket_data_api_activity".to_string(),
    }
}

fn closed_trade_json(t: &ClosedTrade) -> ClosedTradeJson {
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
        signal_source: t.signal_source.clone(),
        analysis_id: t.analysis_id.clone(),
    }
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
            let market_title: Option<String> = title_map.get(token_id).cloned();
            let ob = state.book_store.get_book_snapshot(token_id);
            json!({
                "token_id": token_id,
                "title": market_title,
                "best_bid": ob.as_ref().and_then(|o| o.best_bid()).map(|p| p as f64 / 1000.0),
                "best_ask": ob.as_ref().and_then(|o| o.best_ask()).map(|p| p as f64 / 1000.0),
                "spread_bps": ob.as_ref().and_then(|o| o.spread_bps()),
            })
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
    let blink_wallet_truth_last_sync_ms = live.wallet_truth_last_sync_ms();
    let blink_wallet_truth_sync_age_ms = live.wallet_truth_sync_age_ms();
    let p = live.portfolio.lock().await;
    let local_cash = p.cash_usdc;
    let nav = p.nav();
    let realized = p.realized_pnl();
    let fees_paid = p.total_fees_paid_usdc;
    let local_open_positions_count = p.positions.len();
    let local_position_ids: HashSet<String> =
        p.positions.iter().map(|pos| pos.token_id.clone()).collect();
    drop(p);
    let funder_address = std::env::var("POLYMARKET_FUNDER_ADDRESS").ok();
    let onchain_cash = match funder_address.as_deref() {
        Some(user) => tokio::time::timeout(
            std::time::Duration::from_millis(1_500),
            fetch_onchain_pusd_cash(user),
        )
        .await
        .ok()
        .flatten(),
        None => None,
    };
    let onchain_cash_verified = onchain_cash.is_some();
    let wallet_cash = onchain_cash;
    let cash_for_blink_nav = onchain_cash.unwrap_or(local_cash);
    let blink_nav = if onchain_cash_verified {
        nav - local_cash + cash_for_blink_nav
    } else {
        nav
    };
    let mut exchange_positions_snapshot = match funder_address.as_deref() {
        Some(user) => fetch_polymarket_positions_value(user).await,
        None => None,
    };
    if local_open_positions_count > 0
        && exchange_positions_snapshot
            .as_ref()
            .map(|snapshot| snapshot.positions_count == 0)
            .unwrap_or(false)
    {
        tracing::warn!(
            local_open_positions_count,
            "Polymarket positions returned empty while local ledger had positions; confirming once"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        exchange_positions_snapshot = match funder_address.as_deref() {
            Some(user) => fetch_polymarket_positions_value(user).await,
            None => None,
        };
    }
    let exchange_positions_verified = exchange_positions_snapshot.is_some();
    let (
        exchange_position_value_usdc,
        exchange_position_initial_value_usdc,
        exchange_cash_pnl_usdc,
        exchange_positions_count,
        exchange_positions_preview,
        exchange_open_positions,
        exchange_asset_ids,
        truth_checked_at_ms,
    ) = exchange_positions_snapshot
        .map(|snapshot| {
            (
                snapshot.value_usdc,
                snapshot.initial_value_usdc,
                snapshot.cash_pnl_usdc,
                snapshot.positions_count,
                snapshot.preview,
                snapshot.open_positions,
                snapshot.asset_ids,
                Some(snapshot.checked_at_ms),
            )
        })
        .unwrap_or_else(|| {
            (
                0.0,
                0.0,
                0.0,
                0usize,
                Vec::new(),
                Vec::new(),
                HashSet::new(),
                None,
            )
        });
    let wallet_nav_verified = exchange_positions_verified && onchain_cash_verified;
    let wallet_nav = wallet_cash.map(|cash| cash + exchange_position_value_usdc);
    let reported_nav = wallet_nav_verified.then_some(wallet_nav.unwrap_or(0.0));
    let external_only_positions_count = exchange_asset_ids.difference(&local_position_ids).count();
    let local_only_positions_count = local_position_ids.difference(&exchange_asset_ids).count();
    let open_positions_count = if exchange_positions_verified {
        exchange_positions_count
    } else {
        0
    };
    let wallet_positions_count = if exchange_positions_verified {
        exchange_positions_count
    } else {
        0
    };
    let invested = if exchange_positions_verified {
        exchange_position_initial_value_usdc
    } else {
        0.0
    };
    let unrealized = if exchange_positions_verified {
        exchange_cash_pnl_usdc
    } else {
        0.0
    };
    let wallet_position_value = exchange_positions_verified.then_some(exchange_position_value_usdc);
    let wallet_position_initial_value =
        exchange_positions_verified.then_some(exchange_position_initial_value_usdc);
    let wallet_open_pnl = exchange_positions_verified.then_some(exchange_cash_pnl_usdc);
    let mut reality_issues = Vec::<String>::new();
    if funder_address.is_none() {
        reality_issues.push("missing_POLYMARKET_FUNDER_ADDRESS".to_string());
    }
    if !onchain_cash_verified {
        reality_issues.push("onchain_cash_unverified".to_string());
    }
    if !exchange_positions_verified {
        reality_issues.push("polymarket_positions_unverified".to_string());
    }
    if external_only_positions_count > 0 {
        reality_issues.push(format!(
            "{external_only_positions_count}_exchange_position_not_in_blink_ledger"
        ));
    }
    if local_only_positions_count > 0 {
        reality_issues.push(format!(
            "{local_only_positions_count}_blink_position_not_on_exchange"
        ));
    }
    let reality_status = if !exchange_positions_verified || !onchain_cash_verified {
        "unverified"
    } else if external_only_positions_count > 0 || local_only_positions_count > 0 {
        "mismatch"
    } else {
        "matched"
    };
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
    let mut payload = serde_json::Map::new();
    macro_rules! put {
        ($key:literal, $value:expr) => {
            payload.insert($key.to_string(), json!($value));
        };
    }
    put!("mode", "live");
    put!("pending_orders", pending_count);
    put!("confirmed_fills", failsafe.confirmed_fills);
    put!("no_fills", failsafe.no_fills);
    put!("stale_orders", failsafe.stale_orders);
    put!("confirmation_rate_pct", failsafe.confirmation_rate_pct);
    put!("daily_pnl_usdc", daily_pnl);
    put!("max_daily_loss_pct", max_daily_loss_pct);
    put!("circuit_breaker_tripped", cb_tripped);
    put!("trading_enabled", trading_enabled);
    put!(
        "accounting_source",
        if wallet_nav_verified {
            "onchain_pusd_plus_polymarket_position_value"
        } else {
            "unverified"
        }
    );
    put!(
        "balance_source",
        if wallet_nav_verified {
            "onchain_pusd_cash_plus_data_api_positions"
        } else {
            "unverified"
        }
    );
    put!(
        "cash_source",
        if onchain_cash_verified {
            "onchain_pusd_balance"
        } else {
            "unverified"
        }
    );
    put!("reality_status", reality_status);
    put!("reality_issues", reality_issues);
    put!("truth_checked_at_ms", truth_checked_at_ms);
    put!("exchange_positions_verified", exchange_positions_verified);
    put!("onchain_cash_verified", onchain_cash_verified);
    put!("wallet_truth_verified", wallet_nav_verified);
    put!(
        "blink_wallet_truth_last_sync_ms",
        blink_wallet_truth_last_sync_ms
    );
    put!(
        "blink_wallet_truth_sync_age_ms",
        blink_wallet_truth_sync_age_ms
    );
    put!(
        "external_only_positions_count",
        external_only_positions_count
    );
    put!("local_only_positions_count", local_only_positions_count);
    put!("local_open_positions_count", local_open_positions_count);
    put!("confirmed_only", false);
    put!("queued_orders_affect_nav", false);
    put!("cash_usdc", wallet_cash);
    put!("nav_usdc", reported_nav);
    put!("blink_cash_usdc", local_cash);
    put!("blink_nav_usdc", blink_nav);
    put!(
        "wallet_nav_usdc",
        wallet_nav_verified.then_some(wallet_nav.unwrap_or(0.0))
    );
    put!("invested_usdc", invested);
    put!("unrealized_pnl_usdc", unrealized);
    put!("wallet_open_pnl_usdc", wallet_open_pnl);
    put!("wallet_unrealized_pnl_usdc", wallet_open_pnl);
    put!(
        "wallet_position_initial_value_usdc",
        wallet_position_initial_value
    );
    put!("wallet_position_value_usdc", wallet_position_value);
    put!(
        "wallet_pnl_source",
        if exchange_positions_verified {
            "polymarket_data_api_cashPnl"
        } else {
            "unverified"
        }
    );
    put!(
        "pnl_source",
        if exchange_positions_verified {
            "polymarket_data_api_cashPnl"
        } else {
            "unverified"
        }
    );
    put!("realized_pnl_usdc", realized);
    put!("fees_paid_usdc", fees_paid);
    put!("open_positions", exchange_open_positions);
    put!("open_positions_count", open_positions_count);
    put!("wallet_positions_count", wallet_positions_count);
    put!("exchange_position_value_usdc", exchange_position_value_usdc);
    put!("external_position_value_usdc", exchange_position_value_usdc);
    put!("exchange_positions_count", exchange_positions_count);
    put!("exchange_positions_preview", exchange_positions_preview);
    put!("emergency_stop_endpoint", "/api/emergency_stop");
    put!("heartbeat_ok", failsafe.heartbeat_ok_count);
    put!("heartbeat_fail", failsafe.heartbeat_fail_count);
    put!("trigger_count", failsafe.trigger_count);
    put!("uptime_secs", uptime_secs);
    Json(serde_json::Value::Object(payload))
}

async fn fetch_polymarket_positions_value(user: &str) -> Option<ExchangePositionsSnapshot> {
    let checked_at_ms = postgres_logger::now_ms();
    let max_items = data_api_max_items();
    let mut positions = Vec::new();
    let mut seen = HashSet::new();

    for offset in (0..max_items).step_by(DATA_API_PAGE_LIMIT) {
        let limit = DATA_API_PAGE_LIMIT.min(max_items - offset);
        let url = format!(
            "https://data-api.polymarket.com/positions?user={user}&limit={limit}&offset={offset}"
        );
        let body = fetch_data_api_json_with_retry(&url, 2_000).await?;
        let page_positions = data_api_entries_from_body(body);
        let page_len = page_positions.len();
        let mut added = 0usize;

        for position in page_positions {
            if seen.insert(data_api_position_key(&position)) {
                positions.push(position);
                added += 1;
            }
        }

        if page_len < limit || added == 0 {
            break;
        }
    }

    Some(exchange_positions_snapshot_from_entries(
        &positions,
        checked_at_ms,
    ))
}

#[cfg(test)]
fn exchange_positions_snapshot_from_body(
    body: serde_json::Value,
    checked_at_ms: u64,
) -> ExchangePositionsSnapshot {
    let positions = data_api_entries_from_body(body);
    exchange_positions_snapshot_from_entries(&positions, checked_at_ms)
}

fn exchange_positions_snapshot_from_entries(
    positions: &[serde_json::Value],
    checked_at_ms: u64,
) -> ExchangePositionsSnapshot {
    let asset_ids = positions
        .iter()
        .filter_map(|p| json_text(p, &["asset", "token_id", "tokenId", "conditionId"]))
        .collect::<HashSet<_>>();
    let preview = positions
        .iter()
        .take(5)
        .map(|p| {
            json!({
                "title": p.get("title").or_else(|| p.get("market")).or_else(|| p.get("eventTitle")).cloned(),
                "outcome": p.get("outcome").cloned(),
                "size": p.get("size").or_else(|| p.get("tokens")).or_else(|| p.get("quantity")).cloned(),
                "current_value_usdc": p.get("currentValue").or_else(|| p.get("current_value")).or_else(|| p.get("value")).cloned(),
                "cash_pnl_usdc": p.get("cashPnl").or_else(|| p.get("cash_pnl")).cloned(),
            })
        })
        .collect::<Vec<_>>();
    let open_positions = positions
        .iter()
        .enumerate()
        .map(|(idx, p)| exchange_position_json(idx, p))
        .collect::<Vec<_>>();
    let value = open_positions
        .iter()
        .map(|pos| pos.usdc_spent + pos.unrealized_pnl)
        .sum::<f64>();
    let initial_value = open_positions.iter().map(|pos| pos.usdc_spent).sum::<f64>();
    let cash_pnl = open_positions
        .iter()
        .map(|pos| pos.unrealized_pnl)
        .sum::<f64>();
    ExchangePositionsSnapshot {
        value_usdc: value,
        initial_value_usdc: initial_value,
        cash_pnl_usdc: cash_pnl,
        positions_count: positions.len(),
        preview,
        open_positions,
        asset_ids,
        checked_at_ms,
    }
}

fn data_api_position_key(position: &serde_json::Value) -> String {
    let asset = json_text(position, &["asset", "token_id", "tokenId", "conditionId"])
        .unwrap_or_else(|| "no_asset".to_string());
    let outcome =
        json_text(position, &["outcome", "side"]).unwrap_or_else(|| "no_outcome".to_string());
    format!("{asset}:{outcome}")
}

fn exchange_position_json(index: usize, p: &serde_json::Value) -> PositionJson {
    let token_id = json_text(p, &["asset", "token_id", "tokenId", "conditionId"])
        .unwrap_or_else(|| format!("exchange-position-{index}"));
    let title = json_text(p, &["title", "market", "eventTitle"]);
    let outcome = json_text(p, &["outcome", "side"]);
    let shares = json_f64(p, &["size", "tokens", "quantity"]).unwrap_or(0.0);
    let entry_price = json_f64(p, &["avgPrice", "avg_price", "averagePrice"]).unwrap_or(0.0);
    let current_price = json_f64(p, &["curPrice", "currentPrice", "price"]).unwrap_or(entry_price);
    let initial_value =
        json_f64(p, &["initialValue", "initial_value"]).unwrap_or_else(|| shares * entry_price);
    let current_value = json_f64(p, &["currentValue", "current_value", "value"])
        .unwrap_or_else(|| shares * current_price);
    let pnl = json_f64(p, &["cashPnl", "cash_pnl"]).unwrap_or(current_value - initial_value);
    let pnl_pct = json_f64(p, &["percentPnl", "percent_pnl"]).unwrap_or_else(|| {
        if initial_value.abs() > f64::EPSILON {
            (pnl / initial_value) * 100.0
        } else {
            0.0
        }
    });

    PositionJson {
        id: index + 1,
        token_id,
        market_title: title,
        market_outcome: outcome,
        side: "Buy".to_string(),
        entry_price,
        shares,
        usdc_spent: initial_value,
        current_price,
        unrealized_pnl: pnl,
        unrealized_pnl_pct: pnl_pct,
        opened_at: String::new(),
        opened_age_secs: 0,
        fee_category: "exchange".to_string(),
        fee_rate: 0.0,
        event_start_time: None,
        event_end_time: None,
        secs_to_event: None,
    }
}

fn json_f64(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))
    })
}

fn json_i64(value: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
                .or_else(|| v.as_f64().map(|n| n as i64))
                .or_else(|| v.as_str()?.parse::<i64>().ok())
        })
    })
}

fn json_text(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    })
}

async fn fetch_onchain_pusd_cash(user: &str) -> Option<f64> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(900))
        .build()
        .ok()?;

    for rpc_url in polygon_rpc_urls() {
        let decimals = rpc_eth_call(&client, &rpc_url, POLYMARKET_PUSD_PROXY, "0x313ce567")
            .await
            .and_then(|raw| parse_quantity_u64(&raw))
            .map(|v| v.min(u8::MAX as u64) as u8)
            .unwrap_or(6);
        let balance_call = format!("0x70a08231{}", abi_address(user)?);
        if let Some(raw_balance) =
            rpc_eth_call(&client, &rpc_url, POLYMARKET_PUSD_PROXY, &balance_call).await
        {
            if let Some(balance) = parse_token_amount_f64(&raw_balance, decimals) {
                return Some(balance);
            }
        }
    }

    None
}

fn polygon_rpc_urls() -> Vec<String> {
    let mut urls: Vec<String> = std::env::var("POLYGON_RPC_URL")
        .ok()
        .into_iter()
        .flat_map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    for url in DEFAULT_POLYGON_RPCS {
        if !urls.iter().any(|u| u == url) {
            urls.push((*url).to_string());
        }
    }
    urls
}

async fn rpc_eth_call(
    client: &reqwest::Client,
    rpc_url: &str,
    to: &str,
    data: &str,
) -> Option<String> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [{ "to": to, "data": data }, "latest"],
    });
    let response: serde_json::Value = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    if response.get("error").is_some() {
        return None;
    }
    response
        .get("result")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn abi_address(address: &str) -> Option<String> {
    let hex = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .unwrap_or(address);
    if hex.len() != 40 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{hex:0>64}").to_lowercase())
}

fn parse_quantity_u64(hex: &str) -> Option<u64> {
    let normalized = normalize_quantity(hex);
    u64::from_str_radix(normalized.trim_start_matches("0x"), 16).ok()
}

fn parse_token_amount_f64(raw_hex: &str, decimals: u8) -> Option<f64> {
    let normalized = normalize_quantity(raw_hex);
    let raw = u128::from_str_radix(normalized.trim_start_matches("0x"), 16).ok()?;
    let scale = 10u128.checked_pow(decimals as u32)? as f64;
    if scale == 0.0 {
        return None;
    }
    Some(raw as f64 / scale)
}

fn normalize_quantity(hex: &str) -> String {
    let trimmed = hex.trim();
    let body = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed)
        .trim_start_matches('0');
    if body.is_empty() {
        "0x0".to_string()
    } else {
        format!("0x{}", body.to_lowercase())
    }
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

async fn post_emergency_stop(State(state): State<AppState>) -> Json<serde_json::Value> {
    state.trading_paused.store(true, Ordering::Relaxed);

    let mut risk_disabled = false;
    if let Some(ref risk) = state.risk {
        let mut rm = risk.lock_or_recover();
        rm.config_mut().trading_enabled = false;
        risk_disabled = true;
    }

    crate::activity_log::push(
        &state.activity_log,
        crate::activity_log::EntryKind::Warn,
        "EMERGENCY STOP: trading disabled; systemd hard-stop requested".to_string(),
    );
    tracing::error!("Emergency stop requested via API; stopping blink-engine.service");

    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(75));
        let _ = std::process::Command::new("/bin/systemctl")
            .args(["stop", "--no-block", "blink-engine.service"])
            .status();
        let _ = std::process::Command::new("/bin/systemctl")
            .args(["kill", "-s", "SIGKILL", "blink-engine.service"])
            .status();
    });

    Json(json!({
        "ok": true,
        "trading_paused": true,
        "risk_trading_enabled": false,
        "risk_disabled": risk_disabled,
        "service_stop_requested": true,
        "hard_kill_signal": "SIGKILL",
    }))
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
    let wants_trading_enabled = body
        .get("trading_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut geoblock_status = None;
    if wants_trading_enabled {
        if !crate::geo_guard::guard_enabled() {
            return Json(json!({
                "ok": false,
                "error": "geo_guard_disabled",
                "message": "Refusing to enable live trading while BLINK_GEO_GUARD_ENABLED=false",
            }));
        }
        match crate::geo_guard::check_geoblock().await {
            Ok(status) if status.blocked => {
                tracing::warn!(
                    location = %status.location_label(),
                    "Geo guard blocked attempt to enable live trading"
                );
                return Json(json!({
                    "ok": false,
                    "error": "geo_guard_blocked",
                    "message": format!("Polymarket geoblock reports blocked location {}", status.location_label()),
                    "geoblock": status.public_json(),
                }));
            }
            Ok(status) => {
                geoblock_status = Some(status.public_json());
            }
            Err(e) => {
                tracing::warn!(error = ?e, "Geo guard could not verify eligibility");
                return Json(json!({
                    "ok": false,
                    "error": "geo_guard_unverified",
                    "message": format!("Could not verify Polymarket geoblock before enabling live trading: {e}"),
                }));
            }
        }
    }
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
    Json(json!({ "ok": true, "updated": changed, "geoblock": geoblock_status }))
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
    if state.paper.is_some() || state.live_engine.is_some() {
        if let Ok(cached) = state.portfolio_cache.read() {
            if let Some(ref portfolio_json) = *cached {
                snapshot["portfolio"] = portfolio_json.clone();
            }
        }
        if let Some(ref paper) = state.paper {
            // Engine-level metrics available without locking portfolio.
            snapshot["vol_bps"] = json!(paper.vol_bps());
        }
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

    // Recent activity (last 50 entries)
    {
        let log = state.activity_log.lock().unwrap_or_else(|e| e.into_inner());
        let recent: Vec<serde_json::Value> = log
            .iter()
            .rev()
            .take(50)
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

async fn get_market_meta(Path(token_id): Path<String>) -> Json<serde_json::Value> {
    let gamma_url = format!(
        "https://gamma-api.polymarket.com/markets?clob_token_ids={}",
        token_id
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    match client.get(&gamma_url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(data) => Json(gamma_market_meta_payload(&token_id, &data)),
            Err(e) => Json(json!({
                "available": false,
                "token_id": token_id,
                "error": format!("JSON parse error: {e}"),
                "source": "gamma_api",
                "truth_checked_at_ms": postgres_logger::now_ms(),
            })),
        },
        Ok(resp) => Json(json!({
            "available": false,
            "token_id": token_id,
            "error": format!("Gamma API returned {}", resp.status()),
            "source": "gamma_api",
            "truth_checked_at_ms": postgres_logger::now_ms(),
        })),
        Err(e) => Json(json!({
            "available": false,
            "token_id": token_id,
            "error": format!("HTTP error: {e}"),
            "source": "gamma_api",
            "truth_checked_at_ms": postgres_logger::now_ms(),
        })),
    }
}

fn gamma_market_meta_payload(token_id: &str, data: &serde_json::Value) -> serde_json::Value {
    let Some(market) = gamma_first_market(data) else {
        return json!({
            "available": false,
            "token_id": token_id,
            "error": "market not found in Gamma response",
            "source": "gamma_api",
            "truth_checked_at_ms": postgres_logger::now_ms(),
        });
    };

    json!({
        "available": true,
        "token_id": token_id,
        "image": gamma_string(market, &["image", "icon"]).unwrap_or_default(),
        "question": gamma_string(market, &["question", "title"]).unwrap_or_default(),
        "volume": gamma_number_or_string(market, &["volume24hr", "volume24hrClob", "volume_24h", "volume"]).unwrap_or_else(|| "0".to_string()),
        "category": gamma_market_category(market),
        "url": gamma_market_url(market),
        "active": market.get("active").and_then(|v| v.as_bool()),
        "closed": market.get("closed").and_then(|v| v.as_bool()),
        "accepting_orders": market.get("acceptingOrders").and_then(|v| v.as_bool()),
        "end_date": gamma_string(market, &["endDate", "endDateIso"]),
        "source": "gamma_api",
        "truth_checked_at_ms": postgres_logger::now_ms(),
    })
}

fn gamma_first_market(data: &serde_json::Value) -> Option<&serde_json::Value> {
    data.as_array()?.first()
}

fn gamma_market_url(market: &serde_json::Value) -> Option<String> {
    let slug = market
        .get("events")
        .and_then(|e| e.as_array())
        .and_then(|arr| arr.first())
        .and_then(|ev| {
            ev.get("slug")
                .or_else(|| ev.get("event_slug"))
                .or_else(|| ev.get("eventSlug"))
        })
        .and_then(|s| s.as_str())
        .or_else(|| {
            market
                .get("market_slug")
                .or_else(|| market.get("slug"))
                .or_else(|| market.get("marketSlug"))
                .and_then(|s| s.as_str())
        })?;
    Some(format!("https://polymarket.com/event/{slug}"))
}

fn gamma_market_category(market: &serde_json::Value) -> String {
    gamma_string(market, &["category"])
        .or_else(|| {
            market
                .get("events")
                .and_then(|e| e.as_array())
                .and_then(|arr| arr.first())
                .and_then(|event| {
                    event
                        .get("series")
                        .and_then(|s| s.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|series| gamma_string(series, &["title", "slug"]))
                })
        })
        .unwrap_or_default()
}

fn gamma_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    })
}

fn gamma_number_or_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_f64().map(|n| n.to_string()))
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
    })
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

async fn fetch_history_from_db(
    url: &str,
    minutes: Option<u64>,
    page: usize,
    per_page: usize,
) -> Result<(Vec<ClosedTradeJson>, i64), anyhow::Error> {
    let connector = TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()?;
    let connector = MakeTlsConnector::new(connector);

    let (client, connection) = tokio_postgres::connect(url, connector).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("get_history DB connection error: {}", e);
        }
    });

    let offset = (page.saturating_sub(1) * per_page) as i64;
    let limit = per_page as i64;

    let mut sql = "SELECT * FROM blink.closed_trades_full".to_string();
    let mut count_sql = "SELECT COUNT(*) FROM blink.closed_trades_full".to_string();

    if let Some(m) = minutes {
        let cutoff_ms = postgres_logger::now_ms().saturating_sub(m * 60 * 1_000);
        sql.push_str(&format!(" WHERE timestamp_ms >= {}", cutoff_ms));
        count_sql.push_str(&format!(" WHERE timestamp_ms >= {}", cutoff_ms));
    }

    sql.push_str(" ORDER BY timestamp_ms DESC LIMIT $1 OFFSET $2");

    let total_count: i64 = client.query_one(&count_sql, &[]).await?.get(0);
    let rows = client.query(&sql, &[&limit, &offset]).await?;

    let trades = rows
        .iter()
        .map(|row| {
            let ts_ms: i64 = row.get("timestamp_ms");
            let dur: i64 = row.get("duration_secs");
            let closed_at = chrono::DateTime::from_timestamp_millis(ts_ms).unwrap_or_default();
            let opened_at =
                chrono::DateTime::from_timestamp_millis(ts_ms - (dur * 1000)).unwrap_or_default();

            ClosedTradeJson {
                token_id: row.get("token_id"),
                market_title: row.get("market_title"),
                side: row.get("side"),
                entry_price: row.get("entry_price"),
                exit_price: row.get("exit_price"),
                shares: row.get("shares"),
                realized_pnl: row.get("realized_pnl"),
                fees_paid_usdc: row.get("fees_paid_usdc"),
                reason: row.get("reason"),
                opened_at: opened_at.to_rfc3339(),
                closed_at: closed_at.to_rfc3339(),
                duration_secs: dur as u64,
                slippage_bps: 0.0, // Not stored in DB atm
                event_start_time: None,
                event_end_time: None,
                signal_source: "historical_db".to_string(),
                analysis_id: None,
            }
        })
        .collect();

    Ok((trades, total_count))
}

/// GET /api/analytics/equity?range=30m|1h|6h|24h|7d|30d
///
/// Returns the NAV curve for the requested time window.
/// In live mode this only serves verified wallet truth; it never falls back to
/// paper/DB equity. Paper mode queries Postgres when available, then memory.
async fn get_analytics_equity(
    Query(params): Query<EquityRangeParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let range = params.range.as_deref().unwrap_or("30m");
    let window_ms = equity_window_ms(range);
    let bucket_ms = equity_bucket_ms(window_ms);

    let live_trading = std::env::var("LIVE_TRADING")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if state.live_engine.is_some() && live_trading {
        let Some(user) = std::env::var("POLYMARKET_FUNDER_ADDRESS").ok() else {
            return Json(equity_series_payload(
                "live_wallet_unverified",
                range,
                window_ms,
                bucket_ms,
                Vec::<EquityPoint>::new(),
                json!({
                    "reality_status": "unverified",
                    "reality_issues": ["missing_POLYMARKET_FUNDER_ADDRESS"],
                    "wallet_truth_verified": false,
                }),
            ))
            .into_response();
        };

        let onchain_cash = tokio::time::timeout(
            std::time::Duration::from_millis(1_500),
            fetch_onchain_pusd_cash(&user),
        )
        .await
        .ok()
        .flatten();
        let exchange_positions_snapshot = fetch_polymarket_positions_value(&user).await;

        match (onchain_cash, exchange_positions_snapshot) {
            (Some(cash), Some(snapshot)) => {
                let wallet_nav = cash + snapshot.value_usdc;
                let point = EquityPoint {
                    timestamp_ms: snapshot.checked_at_ms,
                    nav_usdc: wallet_nav,
                };
                return Json(equity_series_payload(
                    "live_wallet_truth",
                    range,
                    window_ms,
                    bucket_ms,
                    vec![point],
                    json!({
                        "reality_status": "matched",
                        "truth_checked_at_ms": snapshot.checked_at_ms,
                        "exchange_positions_verified": true,
                        "onchain_cash_verified": true,
                        "wallet_truth_verified": true,
                        "cash_usdc": cash,
                        "wallet_nav_usdc": wallet_nav,
                        "wallet_position_value_usdc": snapshot.value_usdc,
                        "wallet_position_initial_value_usdc": snapshot.initial_value_usdc,
                        "wallet_open_pnl_usdc": snapshot.cash_pnl_usdc,
                        "wallet_pnl_source": "polymarket_data_api_cashPnl",
                    }),
                ))
                .into_response();
            }
            (cash, snapshot) => {
                let mut issues = Vec::new();
                if cash.is_none() {
                    issues.push("onchain_cash_unverified");
                }
                if snapshot.is_none() {
                    issues.push("polymarket_positions_unverified");
                }
                return Json(equity_series_payload(
                    "live_wallet_unverified",
                    range,
                    window_ms,
                    bucket_ms,
                    Vec::<EquityPoint>::new(),
                    json!({
                        "reality_status": "unverified",
                        "reality_issues": issues,
                        "exchange_positions_verified": snapshot.is_some(),
                        "onchain_cash_verified": cash.is_some(),
                        "wallet_truth_verified": false,
                    }),
                ))
                .into_response();
            }
        }
    }

    // ── Attempt Database Query (Postgres) ─────────────────────────────────────
    if let Some(ref url) = state.clickhouse_url {
        let cutoff_ms = postgres_logger::now_ms().saturating_sub(window_ms);

        // Connect and query Postgres
        let points = match query_postgres_equity(url, cutoff_ms).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(err = %e, "get_analytics_equity: DB query failed — falling back to memory");
                Vec::new()
            }
        };

        if !points.is_empty() {
            return Json(equity_series_payload(
                "postgres",
                range,
                window_ms,
                bucket_ms,
                points,
                json!({}),
            ))
            .into_response();
        }
    }

    // ── Fallback: in-memory equity curve ──────────────────────────────────────
    if let Some(ref paper) = state.paper {
        let p =
            match tokio::time::timeout(std::time::Duration::from_secs(2), paper.portfolio.lock())
                .await
            {
                Ok(guard) => guard,
                Err(_) => {
                    return Json(equity_series_payload(
                        "timeout",
                        range,
                        window_ms,
                        bucket_ms,
                        Vec::<EquityPoint>::new(),
                        json!({}),
                    ))
                    .into_response();
                }
            };
        let cutoff_ms = postgres_logger::now_ms().saturating_sub(window_ms);
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
        return Json(equity_series_payload(
            "memory",
            range,
            window_ms,
            bucket_ms,
            points,
            json!({}),
        ))
        .into_response();
    }

    let empty: Vec<EquityPoint> = Vec::new();
    Json(equity_series_payload(
        "none",
        range,
        window_ms,
        bucket_ms,
        empty,
        json!({}),
    ))
    .into_response()
}

fn equity_window_ms(range: &str) -> u64 {
    match range {
        "1h" => 60 * 60 * 1_000,
        "6h" => 6 * 60 * 60 * 1_000,
        "24h" => 24 * 60 * 60 * 1_000,
        "7d" => 7 * 24 * 60 * 60 * 1_000,
        "30d" => 30 * 24 * 60 * 60 * 1_000,
        _ => 30 * 60 * 1_000,
    }
}

fn equity_bucket_ms(window_ms: u64) -> u64 {
    if window_ms > 48 * 60 * 60 * 1_000 {
        10 * 60 * 1_000
    } else if window_ms > 6 * 60 * 60 * 1_000 {
        60 * 1_000
    } else {
        10 * 1_000
    }
}

fn equity_series_payload(
    source: &str,
    range: &str,
    window_ms: u64,
    bucket_ms: u64,
    points: Vec<EquityPoint>,
    extra: serde_json::Value,
) -> serde_json::Value {
    let end_ms = postgres_logger::now_ms();
    let start_ms = end_ms.saturating_sub(window_ms);
    let first_ms = points.first().map(|point| point.timestamp_ms);
    let last_ms = points.last().map(|point| point.timestamp_ms);
    let truncated = first_ms
        .map(|ts| ts > start_ms.saturating_add(bucket_ms))
        .unwrap_or(false);

    let mut payload = json!({
        "source": source,
        "range": range,
        "bucket_ms": bucket_ms,
        "window_ms": window_ms,
        "start_ms": start_ms,
        "end_ms": end_ms,
        "first_ms": first_ms,
        "last_ms": last_ms,
        "truncated": truncated,
        "points": points,
    });

    if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            payload.insert(key.clone(), value.clone());
        }
    }

    payload
}

/// Connects to Postgres and fetches equity points with downsampling based on the requested window.
async fn query_postgres_equity(
    url: &str,
    cutoff_ms: u64,
) -> Result<Vec<EquityPoint>, anyhow::Error> {
    let connector = TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()?;
    let connector = MakeTlsConnector::new(connector);

    let (client, connection) = tokio_postgres::connect(url, connector).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("get_analytics_equity DB connection error: {}", e);
        }
    });

    // Determine sampling interval in ms to prevent blowing up the UI.
    // > 24h -> 10m intervals
    // > 6h  -> 1m intervals
    // else  -> 10s intervals (engine's default sampling)
    let total_window_ms = postgres_logger::now_ms().saturating_sub(cutoff_ms);
    let sample_ms: i64 = if total_window_ms > 48 * 3600 * 1000 {
        10 * 60 * 1000 // 10 minutes
    } else if total_window_ms > 6 * 3600 * 1000 {
        60 * 1000 // 1 minute
    } else {
        10 * 1000 // 10 seconds
    };

    // Use DISTINCT ON to get one point per sample bucket.
    let rows = client
        .query(
            "SELECT DISTINCT ON (timestamp_ms / $1) timestamp_ms, nav_usdc 
             FROM blink.equity_snapshots 
             WHERE timestamp_ms > $2 
             ORDER BY timestamp_ms / $1, timestamp_ms ASC",
            &[&sample_ms, &(cutoff_ms as i64)],
        )
        .await?;

    let points = rows
        .iter()
        .map(|row| {
            let ts: i64 = row.get(0);
            let nav: f64 = row.get(1);
            EquityPoint {
                timestamp_ms: ts as u64,
                nav_usdc: nav,
            }
        })
        .collect();

    Ok(points)
}

#[derive(Serialize)]
struct QuantMetrics {
    total_trades: i64,
    wins: i64,
    losses: i64,
    win_rate_pct: f64,
    profit_factor: f64,
    sharpe_ratio: f64,
    current_drawdown_pct: f64,
    net_pnl: f64,
}

async fn get_analytics_quant(State(state): State<AppState>) -> impl IntoResponse {
    let Some(ref url) = state.clickhouse_url else {
        return (StatusCode::NOT_IMPLEMENTED, "Postgres not configured").into_response();
    };

    let connector = match TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(c) => MakeTlsConnector::new(c),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "TLS error").into_response(),
    };

    let (client, connection) = match tokio_postgres::connect(url, connector).await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("DB connection error: {e}"),
            )
                .into_response()
        }
    };

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("get_analytics_quant DB connection error: {}", e);
        }
    });

    let row = match client
        .query_one("SELECT * FROM blink.v_quant_metrics", &[])
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Query error: {e}"),
            )
                .into_response()
        }
    };

    let metrics = QuantMetrics {
        total_trades: row.get("total_trades"),
        wins: row.get("wins"),
        losses: row.get("losses"),
        win_rate_pct: row.get("win_rate_pct"),
        profit_factor: row.get("profit_factor"),
        sharpe_ratio: row.get("sharpe_ratio"),
        current_drawdown_pct: row.get("current_drawdown_pct"),
        net_pnl: row.get("net_pnl"),
    };

    Json(metrics).into_response()
}

// ─── Bullpen status ──────────────────────────────────────────────────────────

async fn get_bullpen_health() -> Json<serde_json::Value> {
    Json(bullpen_health_payload(bullpen_enabled_from_env()))
}

async fn get_bullpen_discovery() -> Json<serde_json::Value> {
    Json(bullpen_discovery_payload(bullpen_enabled_from_env()))
}

async fn get_bullpen_convergence() -> Json<serde_json::Value> {
    Json(bullpen_convergence_payload(bullpen_enabled_from_env()))
}

fn bullpen_enabled_from_env() -> bool {
    std::env::var("BULLPEN_ENABLED")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1" || v.eq_ignore_ascii_case("yes"))
        .unwrap_or(false)
}

fn bullpen_health_payload(enabled: bool) -> serde_json::Value {
    if !enabled {
        return json!({
            "enabled": false,
            "authenticated": false,
            "consecutive_failures": 0,
            "total_calls": 0,
            "avg_latency_ms": 0,
            "last_error": null,
            "status": "disabled",
            "source": "blink_engine",
            "truth_checked_at_ms": postgres_logger::now_ms(),
        });
    }

    json!({
        "enabled": true,
        "authenticated": false,
        "consecutive_failures": 1,
        "total_calls": 0,
        "avg_latency_ms": 0,
        "last_error": "bullpen_backend_not_wired",
        "status": "unwired",
        "source": "blink_engine",
        "truth_checked_at_ms": postgres_logger::now_ms(),
    })
}

fn bullpen_discovery_payload(enabled: bool) -> serde_json::Value {
    json!({
        "enabled": enabled,
        "total_markets": 0,
        "scan_count": 0,
        "markets": [],
        "status": if enabled { "unwired" } else { "disabled" },
        "source": "blink_engine",
        "truth_checked_at_ms": postgres_logger::now_ms(),
    })
}

fn bullpen_convergence_payload(enabled: bool) -> serde_json::Value {
    json!({
        "enabled": enabled,
        "active_signals": 0,
        "tracked_markets": 0,
        "signals": [],
        "status": if enabled { "unwired" } else { "disabled" },
        "source": "blink_engine",
        "truth_checked_at_ms": postgres_logger::now_ms(),
    })
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
            "win_rate_pct": a.win_rate_pct(), "avg_pnl_per_trade": a.avg_pnl_per_trade(),
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
    use super::{
        bullpen_convergence_payload, bullpen_discovery_payload, bullpen_health_payload,
        data_api_entries_from_body, data_api_entry_key, data_api_position_key, equity_bucket_ms,
        equity_series_payload, equity_window_ms, exchange_position_json,
        exchange_positions_snapshot_from_body, gamma_market_meta_payload,
        live_executions_from_activity_body, live_executions_response, mark_live_cache_unverified,
        post_strategy, post_strategy_rollback, AppState,
    };
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

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "actual={actual} expected={expected}"
        );
    }

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

    #[test]
    fn exchange_position_json_maps_data_api_position_to_blink_position() {
        let raw = json!({
            "asset": "108335214097330660216497436528140920329790228410878622712875555123360135252984",
            "conditionId": "0xf8b61bb1849d27296b9413e471bace0b49f53f87e51aea01b7ea545df52e4302",
            "size": 3.125,
            "avgPrice": 0.32,
            "initialValue": 1,
            "currentValue": 0.9531,
            "cashPnl": -0.0469,
            "percentPnl": -4.6899,
            "curPrice": 0.305,
            "title": "Club Atletico de Madrid vs. Arsenal FC: O/U 1.5",
            "outcome": "Under"
        });

        let position = exchange_position_json(0, &raw);

        assert_eq!(
            position.token_id,
            "108335214097330660216497436528140920329790228410878622712875555123360135252984"
        );
        assert_eq!(
            position.market_title.as_deref(),
            Some("Club Atletico de Madrid vs. Arsenal FC: O/U 1.5")
        );
        assert_eq!(position.market_outcome.as_deref(), Some("Under"));
        assert_eq!(position.side, "Buy");
        assert_close(position.shares, 3.125);
        assert_close(position.entry_price, 0.32);
        assert_close(position.current_price, 0.305);
        assert_close(position.usdc_spent, 1.0);
        assert_close(position.unrealized_pnl, -0.0469);
        assert_close(position.unrealized_pnl_pct, -4.6899);
    }

    #[test]
    fn exchange_positions_snapshot_sums_wallet_truth_fields() {
        let raw = json!([
            {
                "asset": "asset-a",
                "size": 3.125,
                "avgPrice": 0.32,
                "initialValue": 1,
                "currentValue": 0.9531,
                "cashPnl": -0.0469,
                "percentPnl": -4.6899,
                "curPrice": 0.305,
                "title": "Club Atletico de Madrid vs. Arsenal FC: O/U 1.5",
                "outcome": "Under"
            },
            {
                "tokenId": "asset-b",
                "size": "2",
                "averagePrice": "0.40",
                "currentPrice": "0.50",
                "cash_pnl": "0.20",
                "title": "Second market",
                "outcome": "Yes"
            }
        ]);

        let snapshot = exchange_positions_snapshot_from_body(raw, 12345);

        assert_eq!(snapshot.positions_count, 2);
        assert_eq!(snapshot.checked_at_ms, 12345);
        assert!(snapshot.asset_ids.contains("asset-a"));
        assert!(snapshot.asset_ids.contains("asset-b"));
        assert_close(snapshot.value_usdc, 1.9531);
        assert_close(snapshot.initial_value_usdc, 1.8);
        assert_close(snapshot.cash_pnl_usdc, 0.1531);
        assert_eq!(snapshot.open_positions.len(), 2);
        assert_eq!(snapshot.preview.len(), 2);
    }

    #[test]
    fn live_executions_from_activity_body_maps_wallet_trade_without_pnl_claim() {
        let raw = json!([{
            "proxyWallet": "0xca357ba96a54f8c2b95bf99a62a2c18be705986b",
            "timestamp": 1777473614,
            "conditionId": "0xf8b61bb1849d27296b9413e471bace0b49f53f87e51aea01b7ea545df52e4302",
            "type": "TRADE",
            "size": 3.125,
            "usdcSize": 1.0204,
            "transactionHash": "0x75359cfabb4531c51510088762a903a94c2e4f1282f6b2f89311eefb2220ccab",
            "price": 0.32,
            "asset": "108335214097330660216497436528140920329790228410878622712875555123360135252984",
            "side": "BUY",
            "title": "Club Atletico de Madrid vs. Arsenal FC: O/U 1.5",
            "outcome": "Under"
        }]);

        let executions = live_executions_from_activity_body(raw);

        assert_eq!(executions.len(), 1);
        let execution = &executions[0];
        assert_eq!(execution.execution_type, "TRADE");
        assert_eq!(execution.side, "BUY");
        assert_eq!(execution.timestamp, 1777473614);
        assert_eq!(
            execution.transaction_hash.as_deref(),
            Some("0x75359cfabb4531c51510088762a903a94c2e4f1282f6b2f89311eefb2220ccab")
        );
        assert_eq!(
            execution.token_id,
            "108335214097330660216497436528140920329790228410878622712875555123360135252984"
        );
        assert_eq!(
            execution.market_title.as_deref(),
            Some("Club Atletico de Madrid vs. Arsenal FC: O/U 1.5")
        );
        assert_eq!(execution.market_outcome.as_deref(), Some("Under"));
        assert_close(execution.shares, 3.125);
        assert_close(execution.price, 0.32);
        assert_close(execution.usdc_size, 1.0204);
    }

    #[test]
    fn live_executions_from_activity_body_ignores_non_trade_activity() {
        let raw = json!([
            {
                "timestamp": 1777473614,
                "type": "REDEEM",
                "size": 3.125,
                "asset": "not-a-trade"
            },
            {
                "timestamp": 1777473615,
                "size": 1.0,
                "asset": "missing-type-is-not-trusted"
            },
            {
                "timestamp": 1777473616,
                "type": "TRADE",
                "size": 2.0,
                "usdcSize": 1.0,
                "price": 0.5,
                "asset": "trade-asset",
                "side": "SELL"
            }
        ]);

        let executions = live_executions_from_activity_body(raw);

        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].token_id, "trade-asset");
        assert_eq!(executions[0].side, "SELL");
    }

    #[test]
    fn data_api_entries_from_body_accepts_common_wrappers() {
        let direct = data_api_entries_from_body(json!([{ "asset": "direct" }]));
        let data = data_api_entries_from_body(json!({ "data": [{ "asset": "data" }] }));
        let activity = data_api_entries_from_body(json!({ "activity": [{ "asset": "activity" }] }));

        assert_eq!(direct[0]["asset"], "direct");
        assert_eq!(data[0]["asset"], "data");
        assert_eq!(activity[0]["asset"], "activity");
    }

    #[test]
    fn data_api_dedupe_keys_use_wallet_truth_identifiers() {
        let trade = json!({
            "transactionHash": "0xabc",
            "asset": "asset-a",
            "timestamp": 1777473614,
            "side": "BUY",
            "type": "TRADE"
        });
        let same_trade = json!({
            "transaction_hash": "0xabc",
            "token_id": "asset-a",
            "time": "1777473614",
            "side": "BUY",
            "type": "TRADE"
        });
        let position = json!({
            "asset": "asset-a",
            "outcome": "Under"
        });

        assert_eq!(data_api_entry_key(&trade), data_api_entry_key(&same_trade));
        assert_eq!(data_api_position_key(&position), "asset-a:Under");
    }

    #[test]
    fn live_executions_response_paginates_and_reports_truth_timestamp() {
        let executions = (0..3)
            .map(|i| super::LiveExecutionJson {
                transaction_hash: Some(format!("0x{i}")),
                token_id: format!("asset-{i}"),
                condition_id: None,
                market_title: None,
                market_outcome: None,
                side: "BUY".to_string(),
                price: 0.5,
                shares: 1.0,
                usdc_size: 0.5,
                timestamp: 1777473614 - i,
                traded_at: String::new(),
                execution_type: "TRADE".to_string(),
                source: "test".to_string(),
            })
            .collect::<Vec<_>>();

        let response = live_executions_response(executions, "all", 2, 2, "test_source");

        assert_eq!(response["source"], "test_source");
        assert_eq!(response["reality_status"], "matched");
        assert_eq!(response["total"], 3);
        assert_eq!(response["page"], 2);
        assert_eq!(response["per_page"], 2);
        assert_eq!(response["total_pages"], 2);
        assert_eq!(response["executions"].as_array().unwrap().len(), 1);
        assert!(response["truth_checked_at_ms"].as_u64().unwrap() > 0);
    }

    #[test]
    fn bullpen_payloads_are_explicit_when_disabled() {
        let health = bullpen_health_payload(false);
        assert_eq!(health["enabled"], false);
        assert_eq!(health["authenticated"], false);
        assert_eq!(health["consecutive_failures"], 0);
        assert_eq!(health["last_error"], serde_json::Value::Null);
        assert_eq!(health["status"], "disabled");
        assert_eq!(health["source"], "blink_engine");

        let discovery = bullpen_discovery_payload(false);
        assert_eq!(discovery["enabled"], false);
        assert_eq!(discovery["total_markets"], 0);
        assert_eq!(discovery["scan_count"], 0);
        assert_eq!(discovery["markets"].as_array().unwrap().len(), 0);
        assert_eq!(discovery["status"], "disabled");

        let convergence = bullpen_convergence_payload(false);
        assert_eq!(convergence["enabled"], false);
        assert_eq!(convergence["active_signals"], 0);
        assert_eq!(convergence["tracked_markets"], 0);
        assert_eq!(convergence["signals"].as_array().unwrap().len(), 0);
        assert_eq!(convergence["status"], "disabled");
    }

    #[test]
    fn bullpen_payloads_do_not_fabricate_data_when_enabled_but_unwired() {
        let health = bullpen_health_payload(true);
        assert_eq!(health["enabled"], true);
        assert_eq!(health["authenticated"], false);
        assert_eq!(health["consecutive_failures"], 1);
        assert_eq!(health["total_calls"], 0);
        assert_eq!(health["last_error"], "bullpen_backend_not_wired");
        assert_eq!(health["status"], "unwired");

        let discovery = bullpen_discovery_payload(true);
        assert_eq!(discovery["enabled"], true);
        assert_eq!(discovery["total_markets"], 0);
        assert_eq!(discovery["markets"].as_array().unwrap().len(), 0);
        assert_eq!(discovery["status"], "unwired");

        let convergence = bullpen_convergence_payload(true);
        assert_eq!(convergence["enabled"], true);
        assert_eq!(convergence["active_signals"], 0);
        assert_eq!(convergence["signals"].as_array().unwrap().len(), 0);
        assert_eq!(convergence["status"], "unwired");
    }

    #[test]
    fn equity_series_payload_reports_contract_metadata() {
        let window_ms = equity_window_ms("1h");
        let bucket_ms = equity_bucket_ms(window_ms);
        let response = equity_series_payload(
            "live_wallet_truth",
            "1h",
            window_ms,
            bucket_ms,
            vec![super::EquityPoint {
                timestamp_ms: 123_456,
                nav_usdc: 42.25,
            }],
            json!({
                "reality_status": "matched",
                "wallet_truth_verified": true,
            }),
        );

        assert_eq!(response["source"], "live_wallet_truth");
        assert_eq!(response["range"], "1h");
        assert_eq!(response["bucket_ms"], 10_000);
        assert_eq!(response["window_ms"], 3_600_000);
        assert_eq!(response["first_ms"], 123_456);
        assert_eq!(response["last_ms"], 123_456);
        assert_eq!(response["points"].as_array().unwrap().len(), 1);
        assert_eq!(response["reality_status"], "matched");
        assert_eq!(response["wallet_truth_verified"], true);
        assert!(response["start_ms"].as_u64().unwrap() <= response["end_ms"].as_u64().unwrap());
    }

    #[test]
    fn gamma_market_meta_payload_maps_gamma_market_fields() {
        let payload = gamma_market_meta_payload(
            "asset-under",
            &json!([{
                "question": "Club Atletico de Madrid vs. Arsenal FC: O/U 1.5",
                "image": "https://example.com/ucl.png",
                "volume24hr": 283373.21079099993,
                "active": true,
                "closed": false,
                "acceptingOrders": true,
                "endDate": "2026-04-29T19:00:00Z",
                "events": [{
                    "slug": "ucl-atm1-ars-2026-04-29-more-markets",
                    "series": [{ "title": "UEFA Champions League 2025" }]
                }]
            }]),
        );

        assert_eq!(payload["available"], true);
        assert_eq!(payload["token_id"], "asset-under");
        assert_eq!(
            payload["question"],
            "Club Atletico de Madrid vs. Arsenal FC: O/U 1.5"
        );
        assert_eq!(payload["image"], "https://example.com/ucl.png");
        assert_eq!(payload["volume"], "283373.21079099993");
        assert_eq!(payload["category"], "UEFA Champions League 2025");
        assert_eq!(
            payload["url"],
            "https://polymarket.com/event/ucl-atm1-ars-2026-04-29-more-markets"
        );
        assert_eq!(payload["active"], true);
        assert_eq!(payload["closed"], false);
        assert_eq!(payload["accepting_orders"], true);
    }

    #[test]
    fn live_ws_cache_is_marked_unverified_and_hides_local_positions() {
        let payload = mark_live_cache_unverified(json!({
            "cash_usdc": 12.5,
            "nav_usdc": 13.5,
            "invested_usdc": 1.0,
            "unrealized_pnl_usdc": 0.5,
            "open_positions": [{ "token_id": "local-only" }],
            "equity_curve": [12.5, 13.5],
            "equity_timestamps": [1, 2],
        }));

        assert_eq!(payload["mode"], "live");
        assert_eq!(payload["reality_status"], "unverified");
        assert_eq!(payload["wallet_truth_verified"], false);
        assert_eq!(payload["exchange_positions_verified"], false);
        assert_eq!(payload["onchain_cash_verified"], false);
        assert_eq!(payload["blink_cash_usdc"], 12.5);
        assert_eq!(payload["blink_nav_usdc"], 13.5);
        assert!(payload["cash_usdc"].is_null());
        assert!(payload["nav_usdc"].is_null());
        assert_eq!(payload["open_positions"].as_array().unwrap().len(), 0);
        assert_eq!(payload["equity_curve"].as_array().unwrap().len(), 0);
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
