//! Ratatui terminal dashboard for the Blink Engine (v2 — tabbed interface).
//!
//! Layout:
//! ```text
//! ┌─────────────────────────── HEADER ──────────────────────────────────────┐
//! │ [1] Dashboard  [2] Markets  [3] History  [4] Config                    │
//! ├────────────────────────────────────────────────────────────────────────┤
//! │                        TAB CONTENT                                     │
//! ├────────────────────────────────────────────────────────────────────────┤
//! │                        HINT BAR                                        │
//! └───────────────────────────── NOTIFICATIONS (overlay) ──────────────────┘
//! ```
//!
//! Runs in a dedicated OS thread (`spawn_blocking`) so it never blocks the
//! tokio runtime.  State is read from shared `Arc` handles every ~200 ms.

use std::collections::HashMap;
use std::io;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use chrono::Local;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Line as CanvasLine},
        Block, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, Wrap,
    },
    Frame, Terminal,
};

use crate::activity_log::{push as log_push, ActivityEntry, ActivityLog, EntryKind};
use crate::blink_twin::TwinSnapshot;
use crate::latency_tracker::LatencyStats;
use crate::order_book::OrderBookStore;
use crate::paper_engine::{
    ExecutionSummary, ExperimentMetrics, ExperimentSwitches, FillWindowSnapshot,
    RejectionTrendPoint,
};
use crate::paper_portfolio::PaperPortfolio;
use crate::risk_manager::RiskManager;
use crate::rn1_poller::Rn1PollDiagnosticsHandle;
use crate::types::OrderSide;
use crate::ws_client::WsHealthMetrics;

use bpf_probes::KernelSnapshot;

// ─── Constants ───────────────────────────────────────────────────────────────

const TAB_DASHBOARD: usize = 0;
const TAB_MARKETS: usize = 1;
const TAB_HISTORY: usize = 2;
const TAB_CONFIG: usize = 3;
const TAB_PERFORMANCE: usize = 4;
const TAB_TWIN: usize = 5;
const TAB_COUNT: usize = 6;

const NOTIFICATION_TTL: Duration = Duration::from_secs(4);
const MAX_NOTIFICATIONS: usize = 5;

// Thresholds for latency alerting (kernel telemetry)
const SYSCALL_ALERT_US: u64 = 500;
const RTT_ALERT_US: u64 = 2000;

// ─── Notification system ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Info,
    Success,
    Warning,
    Critical,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub message: String,
    pub kind: NotificationKind,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl Notification {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }

    fn color(&self) -> Color {
        match self.kind {
            NotificationKind::Info => Color::Cyan,
            NotificationKind::Success => Color::Green,
            NotificationKind::Warning => Color::Yellow,
            NotificationKind::Critical => Color::Red,
        }
    }
}

// ── Monokai Palette ──────────────────────────────────────────────────────────
const MONO_PINK: Color = Color::Rgb(249, 38, 114);
const MONO_GREEN: Color = Color::Rgb(166, 226, 46);
const MONO_GOLD: Color = Color::Rgb(230, 219, 116);
const MONO_BLUE: Color = Color::Rgb(102, 217, 239);
const MONO_PURPLE: Color = Color::Rgb(174, 129, 255);
const MONO_GRAY: Color = Color::Rgb(117, 113, 94);

// ── Snapshot types (cheaply cloneable, no locks held during render) ─────────

#[derive(Default, Clone)]
pub struct PortfolioSnapshot {
    pub cash_usdc: f64,
    pub total_invested: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub nav: f64,
    pub total_signals: usize,
    pub filled_orders: usize,
    pub aborted_orders: usize,
    pub skipped_orders: usize,
    pub positions: Vec<PositionSnap>,
    /// NAV samples for equity sparkline (newest at end).
    pub equity_curve: Vec<f64>,
    pub risk_status: String,
    /// Closed trade snapshots for the history tab.
    pub closed_trades: Vec<ClosedTradeSnap>,
    /// Max drawdown percentage.
    pub max_drawdown_pct: f64,
    /// High-water mark NAV.
    pub high_water_mark: f64,
}

#[derive(Clone)]
pub struct PositionSnap {
    pub id: usize,
    pub token_id: String,
    pub market_title: Option<String>,
    pub market_outcome: Option<String>,
    pub side: OrderSide,
    pub entry_price: f64,
    pub current_price: f64,
    pub shares: f64,
    pub usdc_spent: f64,
    pub unrealized_pnl: f64,
    pub unrealized_pct: f64,
    pub age_secs: u64,
}

#[derive(Clone)]
pub struct ClosedTradeSnap {
    pub token_id: String,
    pub side: OrderSide,
    pub entry_price: f64,
    pub exit_price: f64,
    pub shares: f64,
    pub realized_pnl: f64,
    pub reason: String,
    pub opened_at: String,
    pub closed_at: String,
    pub duration_secs: u64,
    pub scorecard_slippage_bps: f64,
    pub scorecard_queue_delay_ms: u64,
    pub scorecard_tags: Vec<String>,
}

/// Cheap snapshot of performance stats for rendering (no locks during draw).
#[derive(Default, Clone)]
pub struct PerfSnapshot {
    pub msgs_per_sec: f64,
    pub msg_total: u64,
    pub sig_avg_us: Option<u64>,
    pub sig_min_us: Option<u64>,
    pub sig_max_us: Option<u64>,
    pub sig_p99_us: Option<u64>,
    pub sig_count: usize,
    pub fill_p50_us: Option<u64>,
    pub fill_p95_us: Option<u64>,
    pub fill_p99_us: Option<u64>,
    pub rn1_diag: Option<String>,
    pub ws_reconnects: u64,
    pub ws_pongs: u64,
}

/// Snapshot of risk manager state for the Config tab.
#[derive(Default, Clone)]
pub struct RiskConfigSnap {
    pub max_daily_loss_pct: f64,
    pub max_concurrent_positions: usize,
    pub max_single_order_usdc: f64,
    pub max_orders_per_second: u32,
    pub trading_enabled: bool,
    pub var_threshold_pct: f64,
    pub circuit_breaker_tripped: bool,
    pub circuit_breaker_reason: String,
    pub daily_pnl: f64,
    pub rolling_exposure_usdc: f64,
}

#[derive(Default, Clone)]
pub struct TwinUiSnapshot {
    pub enabled: bool,
    pub generation: u32,
    pub extra_latency_ms: u64,
    pub slippage_penalty_bps: f64,
    pub drift_multiplier: f64,
    pub nav: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub total_signals: usize,
    pub filled_orders: usize,
    pub aborted_orders: usize,
    pub skipped_orders: usize,
    pub open_positions: usize,
    pub closed_trades: usize,
    pub win_rate_pct: f64,
    pub equity_curve: Vec<f64>,
    pub max_drawdown_pct: f64,
    pub high_water_mark: f64,
    pub nav_return_pct: f64,
}

/// TUI-local interactive state (not shared with engine).
struct TuiState {
    active_tab: usize,
    log_scroll: usize,
    history_scroll: usize,
    // Market switcher
    market_search: String,
    market_search_active: bool,
    selected_market_idx: usize,
    // Config editor
    config_selected: usize,
    config_editing: bool,
    config_edit_buf: String,
    // Notifications
    notifications: Vec<Notification>,
    // Rejection tracking
    rejection_counts: HashMap<String, usize>,
    rejection_trend_24h: HashMap<String, Vec<RejectionTrendPoint>>,
    execution_summary: ExecutionSummary,
    experiment_metrics: ExperimentMetrics,
    experiment_switches: ExperimentSwitches,
    modern_theme: bool,
}

impl TuiState {
    fn new() -> Self {
        Self {
            active_tab: TAB_DASHBOARD,
            log_scroll: 0,
            history_scroll: 0,
            market_search: String::new(),
            market_search_active: false,
            selected_market_idx: 0,
            config_selected: 0,
            config_editing: false,
            config_edit_buf: String::new(),
            notifications: Vec::new(),
            rejection_counts: HashMap::new(),
            rejection_trend_24h: HashMap::new(),
            execution_summary: ExecutionSummary::default(),
            experiment_metrics: ExperimentMetrics::default(),
            experiment_switches: ExperimentSwitches::default(),
            modern_theme: theme_env_default(),
        }
    }

    fn push_notification(&mut self, kind: NotificationKind, message: impl Into<String>) {
        self.notifications.push(Notification {
            message: message.into(),
            kind,
            created_at: Instant::now(),
            ttl: NOTIFICATION_TTL,
        });
        if self.notifications.len() > MAX_NOTIFICATIONS {
            self.notifications.remove(0);
        }
    }

    fn gc_notifications(&mut self) {
        self.notifications.retain(|n| !n.is_expired());
    }

    fn track_rejection(&mut self, reason: &str) {
        *self.rejection_counts.entry(reason.to_string()).or_insert(0) += 1;
    }
}

fn theme_env_default() -> bool {
    std::env::var("TUI_MODERN_THEME")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

// ─── run_tui ─────────────────────────────────────────────────────────────────

/// Entry point — blocks the calling thread until the user presses `q` / `Q`
/// or the `shutdown` flag is set.
pub fn run_tui(
    portfolio: Arc<tokio::sync::Mutex<PaperPortfolio>>,
    risk_status: Arc<Mutex<String>>,
    book_store: Arc<OrderBookStore>,
    activity: ActivityLog,
    ws_live: Arc<AtomicBool>,
    trading_paused: Arc<AtomicBool>,
    rn1_wallet: String,
    markets: Vec<String>,
    shutdown: Arc<AtomicBool>,
    msg_count: Arc<AtomicU64>,
    latency: Arc<Mutex<LatencyStats>>,
    kernel: Option<Arc<Mutex<KernelSnapshot>>>,
    risk_manager: Arc<Mutex<RiskManager>>,
    fill_window: Arc<Mutex<Option<FillWindowSnapshot>>>,
    fill_latency: Arc<Mutex<LatencyStats>>,
    market_subscriptions: Arc<Mutex<Vec<String>>>,
    ws_force_reconnect: Arc<AtomicBool>,
    rejection_trend: Arc<Mutex<HashMap<String, Vec<RejectionTrendPoint>>>>,
    execution_summary: Arc<Mutex<ExecutionSummary>>,
    experiment_data: Arc<Mutex<(ExperimentMetrics, ExperimentSwitches)>>,
    experiment_switches: Arc<Mutex<ExperimentSwitches>>,
    rn1_diagnostics: Rn1PollDiagnosticsHandle,
    twin_snapshot: Arc<tokio::sync::Mutex<TwinSnapshot>>,
    ws_health: Option<Arc<WsHealthMetrics>>,
) -> io::Result<()> {
    // ── Terminal setup ────────────────────────────────────────────────────
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    term.hide_cursor()?;

    let start = Instant::now();
    let result = tui_loop(
        &mut term,
        portfolio,
        risk_status,
        book_store,
        activity,
        ws_live,
        trading_paused,
        rn1_wallet,
        markets,
        shutdown,
        start,
        msg_count,
        latency,
        kernel,
        risk_manager,
        fill_window,
        fill_latency,
        market_subscriptions,
        ws_force_reconnect,
        rejection_trend,
        execution_summary,
        experiment_data,
        experiment_switches,
        rn1_diagnostics,
        twin_snapshot,
        ws_health,
    );

    // ── Cleanup (always runs, even on error) ──────────────────────────────
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

fn tui_loop(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    portfolio: Arc<tokio::sync::Mutex<PaperPortfolio>>,
    risk_status: Arc<Mutex<String>>,
    book_store: Arc<OrderBookStore>,
    activity: ActivityLog,
    ws_live: Arc<AtomicBool>,
    trading_paused: Arc<AtomicBool>,
    rn1_wallet: String,
    markets: Vec<String>,
    shutdown: Arc<AtomicBool>,
    start: Instant,
    msg_count: Arc<AtomicU64>,
    latency: Arc<Mutex<LatencyStats>>,
    kernel: Option<Arc<Mutex<KernelSnapshot>>>,
    risk_manager: Arc<Mutex<RiskManager>>,
    fill_window: Arc<Mutex<Option<FillWindowSnapshot>>>,
    fill_latency: Arc<Mutex<LatencyStats>>,
    market_subscriptions: Arc<Mutex<Vec<String>>>,
    ws_force_reconnect: Arc<AtomicBool>,
    rejection_trend: Arc<Mutex<HashMap<String, Vec<RejectionTrendPoint>>>>,
    execution_summary: Arc<Mutex<ExecutionSummary>>,
    experiment_data: Arc<Mutex<(ExperimentMetrics, ExperimentSwitches)>>,
    experiment_switches: Arc<Mutex<ExperimentSwitches>>,
    rn1_diagnostics: Rn1PollDiagnosticsHandle,
    twin_snapshot: Arc<tokio::sync::Mutex<TwinSnapshot>>,
    ws_health: Option<Arc<WsHealthMetrics>>,
) -> io::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tui tokio runtime");

    let mut state = TuiState::new();

    // For msgs/sec computation from the AtomicU64 monotonic counter.
    let mut last_msg_count: u64 = 0;
    let mut last_mps_check: Instant = Instant::now();
    let mut current_mps: f64 = 0.0;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // ── Poll keyboard (non-blocking) ──────────────────────────────────
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && handle_key(
                    key.code,
                    key.modifiers,
                    &mut state,
                    &trading_paused,
                    &activity,
                    &shutdown,
                    &risk_manager,
                    &book_store,
                    &markets,
                    &market_subscriptions,
                    &ws_force_reconnect,
                    &experiment_switches,
                ) {
                    break;
                }
            }
        }

        // ── Compute msgs/sec (1-second window) ────────────────────────────
        {
            let elapsed = last_mps_check.elapsed();
            if elapsed >= Duration::from_secs(1) {
                let now_count = msg_count.load(Ordering::Relaxed);
                let delta = now_count.saturating_sub(last_msg_count);
                current_mps = delta as f64 / elapsed.as_secs_f64();
                last_msg_count = now_count;
                last_mps_check = Instant::now();
            }
        }

        // ── Snapshot latency stats ────────────────────────────────────────
        let perf = {
            let stats = latency.lock().unwrap();
            let fill = fill_latency.lock().unwrap();
            let rn1 = rn1_diagnostics.lock().unwrap().clone();
            let rn1_line = if rn1.consecutive_errors == 0 {
                Some(format!(
                    "rn1: OK polls={} sigs={}",
                    rn1.total_polls, rn1.total_signals
                ))
            } else {
                let status = rn1
                    .last_http_status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "n/a".to_string());
                let ct = rn1.last_content_type.unwrap_or_else(|| "n/a".to_string());
                let err = rn1.last_error.unwrap_or_else(|| "unknown".to_string());
                let preview = rn1.last_body_preview.unwrap_or_default();
                Some(format!(
                    "rn1: ERR#{} status={} ct={} err={} preview={}",
                    rn1.consecutive_errors, status, ct, err, preview
                ))
            };
            let (ws_reconnects, ws_pongs) = if let Some(ref hm) = ws_health {
                (
                    hm.reconnect_attempts.load(Ordering::Relaxed),
                    hm.pong_recv.load(Ordering::Relaxed),
                )
            } else {
                (0, 0)
            };
            PerfSnapshot {
                msgs_per_sec: current_mps,
                msg_total: last_msg_count,
                sig_avg_us: stats.avg_us(),
                sig_min_us: stats.min_us(),
                sig_max_us: stats.max_us(),
                sig_p99_us: stats.p99_us(),
                sig_count: stats.count(),
                fill_p50_us: fill.p50_us(),
                fill_p95_us: fill.p95_us(),
                fill_p99_us: fill.p99_us(),
                rn1_diag: rn1_line,
                ws_reconnects,
                ws_pongs,
            }
        };

        // ── Snapshot portfolio state (brief async lock) ──────────────────
        let snap = rt.block_on(async {
            let mut p = portfolio.lock().await;
            // Keep portfolio marks in sync with current orderbook mid prices so
            // aggregate NAV/unrealized PnL move live in the TUI.
            let token_ids: Vec<String> =
                p.positions.iter().map(|pos| pos.token_id.clone()).collect();
            for token_id in token_ids {
                if let Some(mark) = book_store.get_mark_price(&token_id) {
                    p.update_price(&token_id, mark as f64 / 1_000.0);
                }
            }
            p.push_equity_snapshot();
            let rs = risk_status.lock().unwrap().clone();
            snapshot_portfolio(&p, &book_store, rs)
        });

        let activity_snap: Vec<ActivityEntry> = {
            let deque = activity.lock().unwrap();
            deque.iter().cloned().collect()
        };

        // ── Snapshot risk config ─────────────────────────────────────────
        let risk_snap = {
            let rm = risk_manager.lock().unwrap();
            let cfg = rm.config();
            RiskConfigSnap {
                max_daily_loss_pct: cfg.max_daily_loss_pct,
                max_concurrent_positions: cfg.max_concurrent_positions,
                max_single_order_usdc: cfg.max_single_order_usdc,
                max_orders_per_second: cfg.max_orders_per_second,
                trading_enabled: cfg.trading_enabled,
                var_threshold_pct: cfg.var_threshold_pct,
                circuit_breaker_tripped: rm.is_circuit_breaker_tripped(),
                circuit_breaker_reason: rm.config().trading_enabled.to_string(), // placeholder
                daily_pnl: rm.daily_pnl(),
                rolling_exposure_usdc: 0.0, // can't call mutable method here
            }
        };

        let ws_connected = ws_live.load(Ordering::Relaxed);
        let paused = trading_paused.load(Ordering::Relaxed);
        let uptime = start.elapsed().as_secs();

        let kernel_snap = kernel
            .as_ref()
            .map(|k| k.lock().unwrap().clone())
            .unwrap_or_default();
        let fill_window_snap = { fill_window.lock().unwrap().clone() };
        let fill_latency_samples = {
            let stats = fill_latency.lock().unwrap();
            stats.samples_us()
        };
        state.rejection_trend_24h = rejection_trend.lock().unwrap().clone();
        state.execution_summary = execution_summary.lock().unwrap().clone();
        let (em, es) = experiment_data.lock().unwrap().clone();
        state.experiment_metrics = em;
        state.experiment_switches = es;
        let subscribed_markets = { market_subscriptions.lock().unwrap().clone() };
        let twin_ui = {
            let t = rt.block_on(async { twin_snapshot.lock().await.clone() });
            TwinUiSnapshot {
                enabled: t.generation > 0
                    || t.total_signals > 0
                    || t.closed_trades > 0
                    || t.open_positions > 0,
                generation: t.generation,
                extra_latency_ms: t.extra_latency_ms,
                slippage_penalty_bps: t.slippage_penalty_bps,
                drift_multiplier: t.drift_multiplier,
                nav: t.nav,
                realized_pnl: t.realized_pnl,
                unrealized_pnl: t.unrealized_pnl,
                total_signals: t.total_signals,
                filled_orders: t.filled_orders,
                aborted_orders: t.aborted_orders,
                skipped_orders: t.skipped_orders,
                open_positions: t.open_positions,
                closed_trades: t.closed_trades,
                win_rate_pct: t.win_rate_pct,
                equity_curve: t.equity_curve,
                max_drawdown_pct: t.max_drawdown_pct,
                high_water_mark: t.high_water_mark,
                nav_return_pct: t.nav_return_pct,
            }
        };

        // ── Latency alerting — flash notification on threshold breach ────
        if kernel_snap.available {
            if kernel_snap.rtt.p99_us > RTT_ALERT_US && kernel_snap.rtt.samples > 10 {
                state.push_notification(
                    NotificationKind::Warning,
                    format!(
                        "TCP RTT p99 {}us > {}us threshold",
                        kernel_snap.rtt.p99_us, RTT_ALERT_US
                    ),
                );
            }
            if kernel_snap.syscall.send_avg_us > SYSCALL_ALERT_US {
                state.push_notification(
                    NotificationKind::Warning,
                    format!(
                        "Syscall send {}us > {}us threshold",
                        kernel_snap.syscall.send_avg_us, SYSCALL_ALERT_US
                    ),
                );
            }
        }

        // ── Track rejections from activity log ───────────────────────────
        for entry in &activity_snap {
            if entry.kind == EntryKind::Skip {
                let reason = if entry.message.contains("Size too small") {
                    "Size too small"
                } else if entry.message.contains("Risk") || entry.message.contains("risk") {
                    "Risk blocked"
                } else if entry.message.contains("drift") || entry.message.contains("Drift") {
                    "Drift abort"
                } else if entry.message.contains("No cash") || entry.message.contains("no cash") {
                    "No cash"
                } else {
                    "Other"
                };
                state.track_rejection(reason);
            }
        }

        // ── Garbage collect expired notifications ────────────────────────
        state.gc_notifications();

        // ── Render ────────────────────────────────────────────────────────
        term.draw(|f| {
            render(
                f,
                &snap,
                &activity_snap,
                &rn1_wallet,
                &markets,
                ws_connected,
                paused,
                uptime,
                &state,
                &perf,
                &kernel_snap,
                &risk_snap,
                &book_store,
                &fill_window_snap,
                &fill_latency_samples,
                &subscribed_markets,
                &twin_ui,
            );
        })?;

        std::thread::sleep(Duration::from_millis(150));
    }
    Ok(())
}

// ─── Keyboard handling ───────────────────────────────────────────────────────

/// Returns `true` if the TUI should exit.
fn handle_key(
    code: KeyCode,
    _modifiers: KeyModifiers,
    state: &mut TuiState,
    trading_paused: &Arc<AtomicBool>,
    activity: &ActivityLog,
    shutdown: &Arc<AtomicBool>,
    risk_manager: &Arc<Mutex<RiskManager>>,
    book_store: &Arc<OrderBookStore>,
    markets: &[String],
    market_subscriptions: &Arc<Mutex<Vec<String>>>,
    ws_force_reconnect: &Arc<AtomicBool>,
    experiment_switches: &Arc<Mutex<ExperimentSwitches>>,
) -> bool {
    // ── Config editing mode intercepts all keys ──────────────────────────
    if state.config_editing {
        match code {
            KeyCode::Esc => {
                state.config_editing = false;
                state.config_edit_buf.clear();
            }
            KeyCode::Enter => {
                apply_config_edit(state, risk_manager, activity);
                state.config_editing = false;
                state.config_edit_buf.clear();
            }
            KeyCode::Backspace => {
                state.config_edit_buf.pop();
            }
            KeyCode::Char(c) => {
                state.config_edit_buf.push(c);
            }
            _ => {}
        }
        return false;
    }

    // ── Market search mode ───────────────────────────────────────────────
    if state.market_search_active {
        match code {
            KeyCode::Esc => {
                state.market_search_active = false;
                state.market_search.clear();
            }
            KeyCode::Backspace => {
                state.market_search.pop();
            }
            KeyCode::Char(' ') => {
                state.market_search_active = false;
            }
            KeyCode::Char(c) => {
                state.market_search.push(c);
            }
            KeyCode::Enter => {
                state.market_search_active = false;
            }
            KeyCode::Down => {
                state.selected_market_idx = state.selected_market_idx.saturating_add(1);
            }
            KeyCode::Up => {
                state.selected_market_idx = state.selected_market_idx.saturating_sub(1);
            }
            _ => {}
        }
        return false;
    }

    // ── Global keys ──────────────────────────────────────────────────────
    match code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            shutdown.store(true, Ordering::Relaxed);
            return true;
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            let was_paused = trading_paused.load(Ordering::Relaxed);
            trading_paused.store(!was_paused, Ordering::Relaxed);
            let msg = if was_paused {
                "Trading RESUMED"
            } else {
                "Trading PAUSED"
            };
            log_push(activity, EntryKind::Engine, msg.to_string());
        }
        KeyCode::Char('t') | KeyCode::Char('T') => {
            state.modern_theme = !state.modern_theme;
            let msg = if state.modern_theme {
                "TUI theme: MODERN"
            } else {
                "TUI theme: NATIVE"
            };
            state.push_notification(NotificationKind::Info, msg);
            log_push(activity, EntryKind::Engine, msg.to_string());
        }
        // Tab switching
        KeyCode::Char('1') => state.active_tab = TAB_DASHBOARD,
        KeyCode::Char('2') => state.active_tab = TAB_MARKETS,
        KeyCode::Char('3') => state.active_tab = TAB_HISTORY,
        KeyCode::Char('4') => state.active_tab = TAB_CONFIG,
        KeyCode::Char('5') => state.active_tab = TAB_PERFORMANCE,
        KeyCode::Char('6') => state.active_tab = TAB_TWIN,
        KeyCode::Tab => {
            state.active_tab = (state.active_tab + 1) % TAB_COUNT;
        }
        KeyCode::BackTab => {
            state.active_tab = if state.active_tab == 0 {
                TAB_COUNT - 1
            } else {
                state.active_tab - 1
            };
        }
        // Tab-specific keys
        _ => match state.active_tab {
            TAB_DASHBOARD => handle_dashboard_key(code, state),
            TAB_MARKETS => handle_markets_key(
                code,
                state,
                book_store,
                markets,
                market_subscriptions,
                ws_force_reconnect,
                activity,
            ),
            TAB_HISTORY => handle_history_key(code, state),
            TAB_CONFIG => {
                handle_config_key(code, state, risk_manager, activity, experiment_switches)
            }
            TAB_PERFORMANCE => {}
            TAB_TWIN => {}
            _ => {}
        },
    }
    false
}

fn handle_dashboard_key(code: KeyCode, state: &mut TuiState) {
    match code {
        KeyCode::Down => state.log_scroll = state.log_scroll.saturating_add(1),
        KeyCode::Up => state.log_scroll = state.log_scroll.saturating_sub(1),
        KeyCode::PageDown => state.log_scroll = state.log_scroll.saturating_add(10),
        KeyCode::PageUp => state.log_scroll = state.log_scroll.saturating_sub(10),
        KeyCode::End => state.log_scroll = usize::MAX,
        _ => {}
    }
}

fn handle_markets_key(
    code: KeyCode,
    state: &mut TuiState,
    book_store: &Arc<OrderBookStore>,
    markets: &[String],
    market_subscriptions: &Arc<Mutex<Vec<String>>>,
    ws_force_reconnect: &Arc<AtomicBool>,
    activity: &ActivityLog,
) {
    match code {
        KeyCode::Char('/') | KeyCode::Char('m') => {
            state.market_search_active = true;
            state.market_search.clear();
        }
        KeyCode::Down => state.selected_market_idx = state.selected_market_idx.saturating_add(1),
        KeyCode::Up => state.selected_market_idx = state.selected_market_idx.saturating_sub(1),
        KeyCode::Char(' ') => {
            let all_markets: Vec<String> = {
                let mut m = markets.to_vec();
                for tid in book_store.token_ids() {
                    if !m.contains(&tid) {
                        m.push(tid);
                    }
                }
                m
            };
            let filtered: Vec<&String> = if state.market_search.is_empty() {
                all_markets.iter().collect()
            } else {
                let q = state.market_search.to_lowercase();
                all_markets
                    .iter()
                    .filter(|m| m.to_lowercase().contains(&q))
                    .collect()
            };
            let idx = state
                .selected_market_idx
                .min(filtered.len().saturating_sub(1));
            let Some(token) = filtered.get(idx) else {
                return;
            };
            let token = (*token).clone();

            let mut subs = market_subscriptions.lock().unwrap();
            if let Some(pos) = subs.iter().position(|m| m == &token) {
                subs.remove(pos);
                log_push(
                    activity,
                    EntryKind::Engine,
                    format!("Market unsubscribed: {}", shorten_token(&token, 16)),
                );
            } else {
                subs.push(token.clone());
                log_push(
                    activity,
                    EntryKind::Engine,
                    format!("Market subscribed: {}", shorten_token(&token, 16)),
                );
            }
            ws_force_reconnect.store(true, Ordering::Relaxed);
        }
        _ => {}
    }
}

fn handle_history_key(code: KeyCode, state: &mut TuiState) {
    match code {
        KeyCode::Down => state.history_scroll = state.history_scroll.saturating_add(1),
        KeyCode::Up => state.history_scroll = state.history_scroll.saturating_sub(1),
        KeyCode::PageDown => state.history_scroll = state.history_scroll.saturating_add(10),
        KeyCode::PageUp => state.history_scroll = state.history_scroll.saturating_sub(10),
        _ => {}
    }
}

fn handle_config_key(
    code: KeyCode,
    state: &mut TuiState,
    risk_manager: &Arc<Mutex<RiskManager>>,
    activity: &ActivityLog,
    experiment_switches: &Arc<Mutex<ExperimentSwitches>>,
) {
    match code {
        KeyCode::Down => state.config_selected = (state.config_selected + 1).min(5),
        KeyCode::Up => state.config_selected = state.config_selected.saturating_sub(1),
        KeyCode::Enter | KeyCode::Char('e') => {
            state.config_editing = true;
            // Pre-fill buffer with current value
            let rm = risk_manager.lock().unwrap();
            let cfg = rm.config();
            state.config_edit_buf = match state.config_selected {
                0 => format!("{:.1}", cfg.max_daily_loss_pct * 100.0),
                1 => format!("{}", cfg.max_concurrent_positions),
                2 => format!("{:.0}", cfg.max_single_order_usdc),
                3 => format!("{}", cfg.max_orders_per_second),
                4 => {
                    if cfg.trading_enabled {
                        "true".into()
                    } else {
                        "false".into()
                    }
                }
                5 => format!("{:.1}", cfg.var_threshold_pct * 100.0),
                _ => String::new(),
            };
        }
        KeyCode::Char('r') => {
            // Reset circuit breaker
            let mut rm = risk_manager.lock().unwrap();
            if rm.is_circuit_breaker_tripped() {
                rm.reset_circuit_breaker();
                drop(rm);
                log_push(
                    activity,
                    EntryKind::Engine,
                    "Circuit breaker RESET by operator".to_string(),
                );
            }
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            let mut ex = experiment_switches.lock().unwrap();
            ex.sizing_variant_b = !ex.sizing_variant_b;
            log_push(
                activity,
                EntryKind::Engine,
                format!("Experiment sizing B={}", ex.sizing_variant_b),
            );
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            let mut ex = experiment_switches.lock().unwrap();
            ex.autoclaim_variant_b = !ex.autoclaim_variant_b;
            log_push(
                activity,
                EntryKind::Engine,
                format!("Experiment autoclaim B={}", ex.autoclaim_variant_b),
            );
        }
        KeyCode::Char('v') | KeyCode::Char('V') => {
            let mut ex = experiment_switches.lock().unwrap();
            ex.drift_variant_b = !ex.drift_variant_b;
            log_push(
                activity,
                EntryKind::Engine,
                format!("Experiment drift B={}", ex.drift_variant_b),
            );
        }
        _ => {}
    }
}

fn apply_config_edit(
    state: &mut TuiState,
    risk_manager: &Arc<Mutex<RiskManager>>,
    activity: &ActivityLog,
) {
    let mut rm = risk_manager.lock().unwrap();
    let cfg = rm.config_mut();
    let val = &state.config_edit_buf;

    let label = match state.config_selected {
        0 => {
            if let Ok(v) = val.parse::<f64>() {
                cfg.max_daily_loss_pct = v / 100.0;
                "Max Daily Loss"
            } else {
                return;
            }
        }
        1 => {
            if let Ok(v) = val.parse::<usize>() {
                cfg.max_concurrent_positions = v;
                "Max Positions"
            } else {
                return;
            }
        }
        2 => {
            if let Ok(v) = val.parse::<f64>() {
                cfg.max_single_order_usdc = v;
                "Max Order Size"
            } else {
                return;
            }
        }
        3 => {
            if let Ok(v) = val.parse::<u32>() {
                cfg.max_orders_per_second = v;
                "Max Orders/sec"
            } else {
                return;
            }
        }
        4 => {
            cfg.trading_enabled = val.eq_ignore_ascii_case("true") || val == "1";
            "Trading Enabled"
        }
        5 => {
            if let Ok(v) = val.parse::<f64>() {
                cfg.var_threshold_pct = v / 100.0;
                "VaR Threshold"
            } else {
                return;
            }
        }
        _ => return,
    };

    drop(rm);
    log_push(
        activity,
        EntryKind::Engine,
        format!("{label} updated to {val}"),
    );
    state.push_notification(NotificationKind::Success, format!("{label} -> {val}"));
}

// ─── State snapshot ──────────────────────────────────────────────────────────

fn snapshot_portfolio(
    p: &PaperPortfolio,
    books: &OrderBookStore,
    risk_status: String,
) -> PortfolioSnapshot {
    let positions = p
        .positions
        .iter()
        .map(|pos| {
            let current = books
                .get_mark_price(&pos.token_id)
                .map(|v| v as f64 / 1_000.0)
                .unwrap_or(pos.current_price);
            let upnl = match pos.side {
                OrderSide::Buy => (current - pos.entry_price) * pos.shares,
                OrderSide::Sell => (pos.entry_price - current) * pos.shares,
            };
            let upnl_pct = if pos.usdc_spent > 0.0 {
                upnl / pos.usdc_spent * 100.0
            } else {
                0.0
            };
            PositionSnap {
                id: pos.id,
                token_id: pos.token_id.clone(),
                market_title: pos.market_title.clone(),
                market_outcome: pos.market_outcome.clone(),
                side: pos.side,
                entry_price: pos.entry_price,
                current_price: current,
                shares: pos.shares,
                usdc_spent: pos.usdc_spent,
                unrealized_pnl: upnl,
                unrealized_pct: upnl_pct,
                age_secs: pos.opened_at.elapsed().as_secs(),
            }
        })
        .collect();

    let closed_trades = p
        .closed_trades
        .iter()
        .map(|t| ClosedTradeSnap {
            token_id: t.token_id.clone(),
            side: t.side,
            entry_price: t.entry_price,
            exit_price: t.exit_price,
            shares: t.shares,
            realized_pnl: t.realized_pnl,
            reason: t.reason.clone(),
            opened_at: t.opened_at_wall.format("%H:%M:%S").to_string(),
            closed_at: t.closed_at_wall.format("%H:%M:%S").to_string(),
            duration_secs: t.duration_secs,
            scorecard_slippage_bps: t.scorecard.slippage_bps,
            scorecard_queue_delay_ms: t.scorecard.queue_delay_ms,
            scorecard_tags: t.scorecard.outcome_tags.clone(),
        })
        .collect();

    PortfolioSnapshot {
        cash_usdc: p.cash_usdc,
        total_invested: p.total_invested(),
        unrealized_pnl: p.unrealized_pnl(),
        realized_pnl: p.realized_pnl(),
        nav: p.nav(),
        total_signals: p.total_signals,
        filled_orders: p.filled_orders,
        aborted_orders: p.aborted_orders,
        skipped_orders: p.skipped_orders,
        equity_curve: p.equity_curve.clone(),
        positions,
        risk_status,
        closed_trades,
        max_drawdown_pct: p.max_drawdown_pct(),
        high_water_mark: p.high_water_mark(),
    }
}

// ─── Render ──────────────────────────────────────────────────────────────────

fn render(
    f: &mut Frame,
    snap: &PortfolioSnapshot,
    activity: &[ActivityEntry],
    rn1_wallet: &str,
    markets: &[String],
    ws_live: bool,
    paused: bool,
    uptime_s: u64,
    state: &TuiState,
    perf: &PerfSnapshot,
    kernel: &KernelSnapshot,
    risk: &RiskConfigSnap,
    book_store: &OrderBookStore,
    fill_window: &Option<FillWindowSnapshot>,
    fill_latency_samples: &[u64],
    subscribed_markets: &[String],
    twin: &TwinUiSnapshot,
) {
    if state.modern_theme {
        render_modern(
            f,
            snap,
            activity,
            rn1_wallet,
            markets,
            ws_live,
            paused,
            uptime_s,
            state,
            perf,
            kernel,
            risk,
            book_store,
            fill_window,
            fill_latency_samples,
            subscribed_markets,
            twin,
        );
        return;
    }

    let area = f.area();

    // ── Outer layout: header | tab bar | body | hint ─────────────────────
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Length(1), // tab bar
            Constraint::Min(10),   // body (tab content)
            Constraint::Length(1), // hint bar
        ])
        .split(area);

    render_header(
        f,
        outer[0],
        snap,
        rn1_wallet,
        ws_live,
        paused,
        uptime_s,
        perf,
        state.modern_theme,
    );
    render_tab_bar(f, outer[1], state.active_tab, state.modern_theme);

    // Council of Agents Status Box (Floating Overlay or Fixed)
    let agent_area = Rect::new(area.width.saturating_sub(32), 0, 31, 3);
    render_council_of_agents(f, agent_area);

    match state.active_tab {
        TAB_DASHBOARD => render_dashboard(
            f,
            outer[2],
            snap,
            activity,
            markets,
            state.log_scroll,
            perf,
            kernel,
            state,
            fill_latency_samples,
        ),
        TAB_MARKETS => render_markets_tab(
            f,
            outer[2],
            book_store,
            &state.market_search,
            state.selected_market_idx,
            markets,
            subscribed_markets,
        ),
        TAB_HISTORY => render_history_tab(f, outer[2], snap, state.history_scroll, state),
        TAB_CONFIG => render_config_tab(
            f,
            outer[2],
            risk,
            &snap.positions,
            state.config_selected,
            state.config_editing,
        ),
        TAB_PERFORMANCE => {
            render_performance_tab(f, outer[2], snap, perf, kernel, state, fill_latency_samples)
        }
        TAB_TWIN => render_twin_tab(f, outer[2], twin),
        _ => {}
    }

    render_hint(
        f,
        outer[3],
        state.active_tab,
        state.market_search_active,
        state.modern_theme,
    );

    // ── Overlay: notifications ───────────────────────────────────────────
    render_notifications(f, area, &state.notifications);
    render_fill_window_overlay(f, area, fill_window);
}

fn render_modern(
    f: &mut Frame,
    snap: &PortfolioSnapshot,
    activity: &[ActivityEntry],
    rn1_wallet: &str,
    markets: &[String],
    ws_live: bool,
    paused: bool,
    uptime_s: u64,
    state: &TuiState,
    perf: &PerfSnapshot,
    kernel: &KernelSnapshot,
    risk: &RiskConfigSnap,
    book_store: &OrderBookStore,
    fill_window: &Option<FillWindowSnapshot>,
    fill_latency_samples: &[u64],
    subscribed_markets: &[String],
    twin: &TwinUiSnapshot,
) {
    let area = f.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(
        f, outer[0], snap, rn1_wallet, ws_live, paused, uptime_s, perf, true,
    );
    render_tab_bar(f, outer[1], state.active_tab, true);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(74), Constraint::Percentage(26)])
        .split(outer[2]);

    // Main pane with subtle card container.
    let main_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(58, 61, 70)))
        .title(Span::styled(
            " WORKSPACE ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let main_inner = main_block.inner(body[0]);
    f.render_widget(main_block, body[0]);

    match state.active_tab {
        TAB_DASHBOARD => render_dashboard(
            f,
            main_inner,
            snap,
            activity,
            markets,
            state.log_scroll,
            perf,
            kernel,
            state,
            fill_latency_samples,
        ),
        TAB_MARKETS => render_markets_tab(
            f,
            main_inner,
            book_store,
            &state.market_search,
            state.selected_market_idx,
            markets,
            subscribed_markets,
        ),
        TAB_HISTORY => render_history_tab(f, main_inner, snap, state.history_scroll, state),
        TAB_CONFIG => render_config_tab(
            f,
            main_inner,
            risk,
            &snap.positions,
            state.config_selected,
            state.config_editing,
        ),
        TAB_PERFORMANCE => render_performance_tab(
            f,
            main_inner,
            snap,
            perf,
            kernel,
            state,
            fill_latency_samples,
        ),
        TAB_TWIN => render_twin_tab(f, main_inner, twin),
        _ => {}
    }

    render_modern_sidepanel(f, body[1], snap, ws_live, paused, perf, twin);
    render_hint(
        f,
        outer[3],
        state.active_tab,
        state.market_search_active,
        true,
    );
    render_notifications(f, area, &state.notifications);
    render_fill_window_overlay(f, area, fill_window);
}

fn render_modern_sidepanel(
    f: &mut Frame,
    area: Rect,
    snap: &PortfolioSnapshot,
    ws_live: bool,
    paused: bool,
    perf: &PerfSnapshot,
    twin: &TwinUiSnapshot,
) {
    let cols = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(6),
        ])
        .split(area);

    let nav_delta = snap.nav - crate::paper_portfolio::STARTING_BALANCE_USDC;
    let nav_pct = nav_delta / crate::paper_portfolio::STARTING_BALANCE_USDC * 100.0;
    let nav_card = vec![
        Line::from(Span::styled(
            " NET ASSET VALUE",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            format!(" ${:.2}", snap.nav),
            Style::default()
                .fg(pnl_color(nav_delta))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(" {:+.2}% session", nav_pct),
            Style::default().fg(pnl_color(nav_delta)),
        )),
    ];
    f.render_widget(
        Paragraph::new(nav_card).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(65, 68, 78)))
                .title(Span::styled(
                    " PORTFOLIO ",
                    Style::default().fg(MONO_GREEN).add_modifier(Modifier::BOLD),
                )),
        ),
        cols[0],
    );

    let attempts = snap.filled_orders + snap.aborted_orders + snap.skipped_orders;
    let fill_rate = if attempts > 0 {
        (snap.filled_orders as f64 / attempts as f64) * 100.0
    } else {
        0.0
    };
    let exec_card = vec![
        Line::from(format!(
            " fills: {}  aborts: {}",
            snap.filled_orders, snap.aborted_orders
        )),
        Line::from(format!(
            " skips: {}  rate: {:.1}%",
            snap.skipped_orders, fill_rate
        )),
        Line::from(format!(" throughput: {:.0} msg/s", perf.msgs_per_sec)),
    ];
    f.render_widget(
        Paragraph::new(exec_card).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(65, 68, 78)))
                .title(Span::styled(
                    " EXECUTION ",
                    Style::default().fg(MONO_BLUE).add_modifier(Modifier::BOLD),
                )),
        ),
        cols[1],
    );

    let status_card = vec![
        Line::from(format!(
            " engine: {}",
            if paused { "PAUSED" } else { "LIVE" }
        )),
        Line::from(format!(
            " websocket: {}",
            if ws_live { "CONNECTED" } else { "DOWN" }
        )),
        Line::from(format!(
            " twin: {}",
            if twin.enabled { "ACTIVE" } else { "IDLE" }
        )),
    ];
    f.render_widget(
        Paragraph::new(status_card).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(65, 68, 78)))
                .title(Span::styled(
                    " STATUS ",
                    Style::default().fg(MONO_GOLD).add_modifier(Modifier::BOLD),
                )),
        ),
        cols[2],
    );

    let spark_data: Vec<u64> = if snap.equity_curve.len() >= 2 {
        let min_val = snap
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max_val = snap
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let range = (max_val - min_val).max(1e-9);
        snap.equity_curve
            .iter()
            .map(|&v| (((v - min_val) / range) * 100.0).round() as u64)
            .collect()
    } else {
        vec![50, 50]
    };
    f.render_widget(
        Sparkline::default()
            .data(&spark_data)
            .style(Style::default().fg(MONO_GREEN))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Rgb(65, 68, 78)))
                    .title(Span::styled(
                        " EQUITY TREND ",
                        Style::default().fg(MONO_GREEN).add_modifier(Modifier::BOLD),
                    )),
            ),
        cols[3],
    );
}

// ── Tab bar ──────────────────────────────────────────────────────────────────

fn render_tab_bar(f: &mut Frame, area: Rect, active: usize, modern_theme: bool) {
    let tabs = [
        "[1] Dashboard",
        "[2] Markets",
        "[3] History",
        "[4] Config",
        "[5] Performance",
        "[6] Blink Twin",
    ];
    let spans: Vec<Span> = tabs
        .iter()
        .enumerate()
        .map(|(i, &label)| {
            if i == active {
                Span::styled(
                    format!(" {label} "),
                    if modern_theme {
                        Style::default()
                            .fg(Color::Black)
                            .bg(MONO_GREEN)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    },
                )
            } else {
                Span::styled(
                    format!(" {label} "),
                    if modern_theme {
                        Style::default().fg(Color::Gray)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                )
            }
        })
        .collect();

    let mut all_spans = vec![Span::styled(" ", Style::default())];
    for (i, s) in spans.into_iter().enumerate() {
        all_spans.push(s);
        if i < tabs.len() - 1 {
            all_spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
    }

    let line = Line::from(all_spans);
    if modern_theme {
        f.render_widget(
            Paragraph::new(line).block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(Color::Rgb(60, 60, 60))),
            ),
            area,
        );
    } else {
        f.render_widget(Paragraph::new(line), area);
    }
}

// ── Header ───────────────────────────────────────────────────────────────────

fn render_header(
    f: &mut Frame,
    area: Rect,
    snap: &PortfolioSnapshot,
    rn1_wallet: &str,
    ws_live: bool,
    paused: bool,
    uptime_s: u64,
    perf: &PerfSnapshot,
    modern_theme: bool,
) {
    let nav_delta = snap.nav - crate::paper_portfolio::STARTING_BALANCE_USDC;
    let nav_pct = nav_delta / crate::paper_portfolio::STARTING_BALANCE_USDC * 100.0;
    let nav_color = pnl_color(nav_delta);

    let status_icon = if paused { "⏸" } else { "▶" };
    let status_color = if paused { MONO_GOLD } else { MONO_GREEN };

    let ws_icon = if ws_live { "🌐" } else { "💀" };
    let ws_color = if ws_live { MONO_GREEN } else { MONO_PINK };

    let time_str = Local::now().format("%H:%M:%S").to_string();
    let uptime_str = format!(
        "{}:{:02}:{:02}",
        uptime_s / 3600,
        (uptime_s % 3600) / 60,
        uptime_s % 60
    );

    let rn1_short = if rn1_wallet.len() >= 10 {
        format!(
            "{}...{}",
            &rn1_wallet[..6],
            &rn1_wallet[rn1_wallet.len() - 4..]
        )
    } else {
        rn1_wallet.to_string()
    };

    let theme_chip = if modern_theme { " MODERN " } else { " NATIVE " };
    let theme_chip_style = if modern_theme {
        Style::default()
            .fg(Color::Black)
            .bg(MONO_BLUE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    };
    let main_line = Line::from(vec![
        Span::styled(
            format!(" {status_icon} BLINK "),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(format!("{ws_icon} WS "), Style::default().fg(ws_color)),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled("🐋 RN1: ", Style::default().fg(MONO_GRAY)),
        Span::styled(rn1_short, Style::default().fg(MONO_PURPLE)),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled("EQUITY: ", Style::default().fg(MONO_GRAY)),
        Span::styled(
            format!("${:.2} ({:>+.2}%)", snap.nav, nav_pct),
            Style::default().fg(nav_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(theme_chip, theme_chip_style),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(
            format!("{} msg/s ", perf.msgs_per_sec as u64),
            Style::default().fg(MONO_BLUE),
        ),
        Span::styled(
            format!("rc:{} ", perf.ws_reconnects),
            Style::default().fg(if perf.ws_reconnects > 20 {
                MONO_PINK
            } else {
                MONO_GRAY
            }),
        ),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(
            perf.rn1_diag
                .clone()
                .unwrap_or_else(|| "rn1: n/a".to_string()),
            Style::default().fg(if perf.rn1_diag.as_deref().unwrap_or("").contains("ERR") {
                MONO_PINK
            } else {
                MONO_GREEN
            }),
        ),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(time_str, Style::default().fg(MONO_GRAY)),
        Span::styled(
            format!(" (up {})", uptime_str),
            Style::default().fg(MONO_GRAY),
        ),
    ]);

    let block = if modern_theme {
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(70, 70, 70)))
    } else {
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(MONO_GRAY))
    };

    f.render_widget(
        Paragraph::new(main_line)
            .block(block)
            .alignment(Alignment::Left),
        area,
    );
}

// ── Dashboard tab (Tab 1) ────────────────────────────────────────────────────

fn render_dashboard(
    f: &mut Frame,
    area: Rect,
    snap: &PortfolioSnapshot,
    activity: &[ActivityEntry],
    markets: &[String],
    log_scroll: usize,
    _perf: &PerfSnapshot,
    kernel: &KernelSnapshot,
    state: &TuiState,
    fill_latency_samples: &[u64],
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),   // body (portfolio + positions)
            Constraint::Length(4), // kernel telemetry (minimalist)
            Constraint::Length(5), // fill latency histogram
            Constraint::Length(4), // rejection trend
            Constraint::Min(6),    // activity log
        ])
        .split(area);

    render_body(f, outer[0], snap, markets);

    // Kernel telemetry - Modern HUD style
    render_kernel_hud(f, outer[1], kernel);

    // Fill latency histogram - Compact
    render_fill_latency_histogram(f, outer[2], fill_latency_samples);

    // Rejection trend in dedicated row (prevents overlap with other panels)
    render_rejection_trend_overlay(f, outer[3], &state.rejection_trend_24h);

    // Activity log - Minimalist
    render_activity_hud(f, outer[4], activity, log_scroll);
}

// ── Body: portfolio + positions ─────────────────────────────────────────────

fn render_body(f: &mut Frame, area: Rect, snap: &PortfolioSnapshot, _markets: &[String]) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(46), Constraint::Min(34)])
        .split(area);

    render_portfolio_hud(f, cols[0], snap);
    render_positions_hud(f, cols[1], snap);
}

fn render_portfolio_hud(f: &mut Frame, area: Rect, snap: &PortfolioSnapshot) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Stats (compact)
            Constraint::Min(8),    // High-Res Equity Graph (larger)
            Constraint::Length(3), // Risk Gauge
        ])
        .split(area);

    // 1. Stats Panel
    let label_style = Style::default().fg(MONO_GRAY);
    let val_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let stats_lines = vec![
        Line::from(vec![
            Span::styled("  CASH      ", label_style),
            Span::styled(format!("${:.2}", snap.cash_usdc), val_style),
        ]),
        Line::from(vec![
            Span::styled("  INVESTED  ", label_style),
            Span::styled(format!("${:.2}", snap.total_invested), val_style),
        ]),
        Line::from(vec![
            Span::styled("  UNREAL    ", label_style),
            Span::styled(
                format!("{:>+.2} USDC", snap.unrealized_pnl),
                Style::default()
                    .fg(pnl_color(snap.unrealized_pnl))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  REALIZED  ", label_style),
            Span::styled(
                format!("{:>+.2} USDC", snap.realized_pnl),
                Style::default()
                    .fg(pnl_color(snap.realized_pnl))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  SIGNALS   ", label_style),
            Span::styled(
                format!("{}", snap.total_signals),
                Style::default().fg(MONO_BLUE),
            ),
        ]),
        Line::from(vec![
            Span::styled("  FILLED    ", label_style),
            Span::styled(
                format!("{}", snap.filled_orders),
                Style::default().fg(MONO_GREEN),
            ),
        ]),
    ];
    f.render_widget(Paragraph::new(stats_lines), chunks[0]);

    // 2. Equity block with % table (left) + grid graph (right)
    let graph_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(10)])
        .split(chunks[1]);
    let nav_delta = snap.nav - crate::paper_portfolio::STARTING_BALANCE_USDC;
    let nav_pct = (nav_delta / crate::paper_portfolio::STARTING_BALANCE_USDC) * 100.0;
    let unreal_pct = if snap.total_invested > 0.0 {
        (snap.unrealized_pnl / snap.total_invested) * 100.0
    } else {
        0.0
    };
    let realized_pct = (snap.realized_pnl / crate::paper_portfolio::STARTING_BALANCE_USDC) * 100.0;
    let pct_table = vec![
        Line::from(vec![
            Span::styled(" NAV   ", label_style),
            Span::styled(
                format!("{:+.2}%", nav_pct),
                Style::default().fg(pnl_color(nav_pct)),
            ),
        ]),
        Line::from(vec![
            Span::styled(" UNR   ", label_style),
            Span::styled(
                format!("{:+.2}%", unreal_pct),
                Style::default().fg(pnl_color(unreal_pct)),
            ),
        ]),
        Line::from(vec![
            Span::styled(" REAL  ", label_style),
            Span::styled(
                format!("{:+.2}%", realized_pct),
                Style::default().fg(pnl_color(realized_pct)),
            ),
        ]),
        Line::from(vec![
            Span::styled(" WIN   ", label_style),
            Span::styled(
                if snap.total_signals > 0 {
                    format!(
                        "{:.1}%",
                        (snap.filled_orders as f64 / snap.total_signals as f64) * 100.0
                    )
                } else {
                    "0.0%".to_string()
                },
                Style::default().fg(MONO_BLUE),
            ),
        ]),
    ];
    f.render_widget(
        Paragraph::new(pct_table).block(
            Block::default()
                .title(" PNL % ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(MONO_GRAY)),
        ),
        graph_cols[0],
    );

    if snap.equity_curve.len() > 2 {
        let min = snap
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max = snap
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let range = (max - min).max(0.01);
        let y0 = min - range * 0.1;
        let y1 = max + range * 0.1;
        let x1 = snap.equity_curve.len() as f64;

        let canvas = Canvas::default()
            .block(
                Block::default()
                    .title(" PERFORMANCE DNA ")
                    .border_style(Style::default().fg(MONO_GRAY)),
            )
            .x_bounds([0.0, x1])
            .y_bounds([y0, y1])
            .paint(|ctx| {
                let grid_color = Color::DarkGray;
                for r in [0.2_f64, 0.4, 0.6, 0.8] {
                    let gy = y0 + (y1 - y0) * r;
                    ctx.draw(&CanvasLine {
                        x1: 0.0,
                        y1: gy,
                        x2: x1,
                        y2: gy,
                        color: grid_color,
                    });
                }
                for r in [0.25_f64, 0.5, 0.75] {
                    let gx = x1 * r;
                    ctx.draw(&CanvasLine {
                        x1: gx,
                        y1: y0,
                        x2: gx,
                        y2: y1,
                        color: grid_color,
                    });
                }
                for i in 0..snap.equity_curve.len().saturating_sub(1) {
                    ctx.draw(&CanvasLine {
                        x1: i as f64,
                        y1: snap.equity_curve[i],
                        x2: (i + 1) as f64,
                        y2: snap.equity_curve[i + 1],
                        color: MONO_GREEN,
                    });
                }
            });
        f.render_widget(canvas, graph_cols[1]);
    }

    // 3. Risk Gauge (Gradient)
    let loss_limit = 500.0; // Example budget
    let loss_pct = (snap.realized_pnl.min(0.0).abs() / loss_limit).min(1.0);
    let gauge_width = area.width.saturating_sub(15) as usize;
    let filled = (loss_pct * gauge_width as f64) as usize;
    let gauge_color = if loss_pct > 0.8 {
        MONO_PINK
    } else if loss_pct > 0.5 {
        MONO_GOLD
    } else {
        MONO_GREEN
    };

    let gauge_bar = format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(gauge_width.saturating_sub(filled))
    );
    let gauge_line = Line::from(vec![
        Span::styled("  RISK HUD  ", label_style),
        Span::styled(gauge_bar, Style::default().fg(gauge_color)),
        Span::styled(
            format!(" {:.0}%", loss_pct * 100.0),
            Style::default()
                .fg(gauge_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(gauge_line), chunks[2]);
}

fn render_positions_hud(f: &mut Frame, area: Rect, snap: &PortfolioSnapshot) {
    let header_style = Style::default().fg(MONO_GRAY).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from("SIDE").style(header_style),
        Cell::from("MARKNAD").style(header_style),
        Cell::from("UTFALL").style(header_style),
        Cell::from("PRICE").style(header_style),
        Cell::from("SIZE").style(header_style),
        Cell::from("P&L").style(header_style),
        Cell::from("%").style(header_style),
        Cell::from("TO WIN $").style(header_style),
        Cell::from("AGE").style(header_style),
    ]);

    let rows: Vec<Row> = if snap.positions.is_empty() {
        vec![Row::new(vec![
            Cell::from("  NO ACTIVE EXPOSURE").style(Style::default().fg(MONO_GRAY))
        ])]
    } else {
        snap.positions
            .iter()
            .map(|pos| {
                let side_color = match pos.side {
                    OrderSide::Buy => MONO_GREEN,
                    OrderSide::Sell => MONO_PINK,
                };
                let pnl_sty = Style::default()
                    .fg(pnl_color(pos.unrealized_pnl))
                    .add_modifier(Modifier::BOLD);
                let max_profit_usd = match pos.side {
                    OrderSide::Buy => (1.0 - pos.entry_price).max(0.0) * pos.shares,
                    OrderSide::Sell => pos.entry_price.max(0.0) * pos.shares,
                };
                let to_win_usd = (max_profit_usd - pos.unrealized_pnl).max(0.0);
                let to_win_style = if to_win_usd <= 0.01 {
                    Style::default().fg(MONO_GREEN).add_modifier(Modifier::BOLD)
                } else if to_win_usd <= (max_profit_usd * 0.20) {
                    Style::default().fg(MONO_GOLD).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(MONO_GRAY)
                };
                let to_win_text = if to_win_usd <= 0.01 {
                    "AUTO NOW".to_string()
                } else {
                    format!("${:.2}", to_win_usd)
                };

                Row::new(vec![
                    Cell::from(format!("{}", pos.side))
                        .style(Style::default().fg(side_color).add_modifier(Modifier::BOLD)),
                    Cell::from(
                        pos.market_title
                            .clone()
                            .unwrap_or_else(|| shorten_token(&pos.token_id, 16)),
                    )
                    .style(Style::default().fg(MONO_BLUE)),
                    Cell::from(
                        pos.market_outcome
                            .clone()
                            .unwrap_or_else(|| "-".to_string()),
                    )
                    .style(Style::default().fg(MONO_GOLD)),
                    Cell::from(format!("{:.3}", pos.current_price))
                        .style(Style::default().fg(Color::White)),
                    Cell::from(format!("${:.0}", pos.usdc_spent))
                        .style(Style::default().fg(MONO_GOLD)),
                    Cell::from(format!("{:>+.2}", pos.unrealized_pnl)).style(pnl_sty),
                    Cell::from(format!("{:>+.1}%", pos.unrealized_pct)).style(pnl_sty),
                    Cell::from(to_win_text).style(to_win_style),
                    Cell::from(format_age(pos.age_secs)).style(Style::default().fg(MONO_GRAY)),
                ])
            })
            .collect()
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Min(28),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .column_spacing(2)
    .block(
        Block::default()
            .title(" ACTIVE POSITIONS ")
            .border_style(Style::default().fg(MONO_GRAY))
            .borders(Borders::LEFT),
    );

    f.render_widget(table, area);
}

fn render_kernel_hud(f: &mut Frame, area: Rect, kernel: &KernelSnapshot) {
    if !kernel.available {
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(area);

    let hud_style = |val: u64, threshold: u64| {
        if val > threshold {
            MONO_PINK
        } else {
            MONO_GREEN
        }
    };

    let rtt_line = Line::from(vec![
        Span::styled("  RTT p99 ", Style::default().fg(MONO_GRAY)),
        Span::styled(
            format!("{}us", kernel.rtt.p99_us),
            Style::default().fg(hud_style(kernel.rtt.p99_us, 5000)),
        ),
    ]);
    let sched_line = Line::from(vec![
        Span::styled("  SCHED p99 ", Style::default().fg(MONO_GRAY)),
        Span::styled(
            format!("{}us", kernel.sched.p99_us),
            Style::default().fg(hud_style(kernel.sched.p99_us, 200)),
        ),
    ]);
    let sys_line = Line::from(vec![
        Span::styled("  SYSCALL send ", Style::default().fg(MONO_GRAY)),
        Span::styled(
            format!("{}us", kernel.syscall.send_avg_us),
            Style::default().fg(hud_style(kernel.syscall.send_avg_us, 100)),
        ),
    ]);

    f.render_widget(
        Paragraph::new(rtt_line).block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(MONO_GRAY)),
        ),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(sched_line).block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(MONO_GRAY)),
        ),
        cols[1],
    );
    f.render_widget(
        Paragraph::new(sys_line).block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(MONO_GRAY)),
        ),
        cols[2],
    );
}

fn render_activity_hud(f: &mut Frame, area: Rect, entries: &[ActivityEntry], scroll: usize) {
    let inner_h = area.height as usize;
    let lines: Vec<Line> = entries
        .iter()
        .rev()
        .skip(scroll)
        .take(inner_h)
        .map(|e| {
            let color = match e.kind {
                EntryKind::Signal => MONO_BLUE,
                EntryKind::Fill => MONO_GREEN,
                EntryKind::Abort | EntryKind::Warn => MONO_PINK,
                _ => MONO_GRAY,
            };
            Line::from(vec![
                Span::styled(format!(" {} ", e.timestamp), Style::default().fg(MONO_GRAY)),
                Span::styled(
                    format!(
                        " {:<6} ",
                        match e.kind {
                            EntryKind::Engine => "ENGINE",
                            EntryKind::Signal => "SIGNAL",
                            EntryKind::Fill => "FILL",
                            EntryKind::Abort => "ABORT",
                            EntryKind::Skip => "SKIP",
                            EntryKind::Warn => "WARN",
                        }
                    ),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(e.message.clone(), Style::default().fg(Color::White)),
            ])
        })
        .collect();

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(MONO_GRAY)),
        ),
        area,
    );
}

// ── Kernel telemetry panel ───────────────────────────────────────────────────

fn render_kernel_telemetry(f: &mut Frame, area: Rect, kernel: &KernelSnapshot, alert: bool) {
    let label_style = Style::default().fg(Color::DarkGray);
    let val_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let na_style = Style::default().fg(Color::DarkGray);
    let warn_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let border_color = if alert { Color::Red } else { Color::Magenta };
    let border_mod = if alert {
        Modifier::BOLD | Modifier::SLOW_BLINK
    } else {
        Modifier::empty()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color).add_modifier(border_mod))
        .title(Span::styled(
            if alert {
                " !! KERNEL TELEMETRY !! "
            } else {
                " KERNEL TELEMETRY "
            },
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let inner_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(inner);

    if !kernel.available {
        let na_line = Line::from(vec![
            Span::styled("  eBPF: N/A ", na_style),
            Span::styled(
                "(Windows/macOS or Linux build without ebpf-telemetry feature)",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(na_line), inner);
        return;
    }

    // RTT column
    let rtt = &kernel.rtt;
    let rtt_val_style = if rtt.p99_us > RTT_ALERT_US {
        warn_style
    } else {
        val_style
    };
    let rtt_lines = vec![
        Line::from(vec![
            Span::styled(
                " TCP RTT ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("({} samples)", rtt.samples), label_style),
        ]),
        Line::from(vec![
            Span::styled(" avg ", label_style),
            Span::styled(format!("{}us", rtt.avg_us), val_style),
            Span::styled("  p99 ", label_style),
            Span::styled(format!("{}us", rtt.p99_us), rtt_val_style),
        ]),
        Line::from(vec![
            Span::styled(" min ", label_style),
            Span::styled(format!("{}us", rtt.min_us), val_style),
            Span::styled("  max ", label_style),
            Span::styled(format!("{}us", rtt.max_us), val_style),
        ]),
    ];
    f.render_widget(Paragraph::new(rtt_lines), inner_cols[0]);

    // Scheduler column
    let sched = &kernel.sched;
    let violations_style = if sched.threshold_violations > 0 {
        warn_style
    } else {
        val_style
    };
    let sched_lines = vec![
        Line::from(vec![
            Span::styled(
                " SCHED ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("({} samples)", sched.samples), label_style),
        ]),
        Line::from(vec![
            Span::styled(" avg ", label_style),
            Span::styled(format!("{}us", sched.avg_us), val_style),
            Span::styled("  p99 ", label_style),
            Span::styled(format!("{}us", sched.p99_us), val_style),
        ]),
        Line::from(vec![
            Span::styled(" >100us ", label_style),
            Span::styled(format!("{}", sched.threshold_violations), violations_style),
        ]),
    ];
    f.render_widget(Paragraph::new(sched_lines), inner_cols[1]);

    // Syscall column
    let sys = &kernel.syscall;
    let sys_val_style = if sys.send_avg_us > SYSCALL_ALERT_US {
        warn_style
    } else {
        val_style
    };
    let sys_lines = vec![
        Line::from(vec![
            Span::styled(
                " SYSCALL ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("({} calls)", sys.samples), label_style),
        ]),
        Line::from(vec![
            Span::styled(" send ", label_style),
            Span::styled(format!("{}us", sys.send_avg_us), sys_val_style),
            Span::styled("  recv ", label_style),
            Span::styled(format!("{}us", sys.recv_avg_us), val_style),
        ]),
        Line::from(vec![
            Span::styled(" epoll ", label_style),
            Span::styled(format!("{}us", sys.epoll_avg_us), val_style),
        ]),
    ];
    f.render_widget(Paragraph::new(sys_lines), inner_cols[2]);
}

fn render_fill_latency_histogram(f: &mut Frame, area: Rect, samples: &[u64]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            format!(" DETECTION->FILL LATENCY ({}) ", samples.len()),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if samples.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No fills recorded yet",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(empty, inner);
        return;
    }

    let bin_count = inner.height.clamp(1, 5) as usize;
    let min = *samples.iter().min().unwrap();
    let max = *samples.iter().max().unwrap();
    let span = max.saturating_sub(min).max(1);

    let mut bins = vec![0usize; bin_count];
    for &sample in samples {
        let idx = if span == 0 {
            bin_count - 1
        } else {
            (((sample - min) as usize * bin_count) / (span as usize + 1)).min(bin_count - 1)
        };
        bins[idx] += 1;
    }

    let max_count = bins.iter().copied().max().unwrap_or(1);
    let bar_width = inner.width.saturating_sub(26) as usize;

    let lines: Vec<Line> = bins
        .iter()
        .enumerate()
        .map(|(i, &count)| {
            let low = min + (span * i as u64 / bin_count as u64);
            let high = if i == bin_count - 1 {
                max
            } else {
                min + (span * (i as u64 + 1) / bin_count as u64)
            };
            let filled = if max_count == 0 {
                0
            } else {
                (count as f64 / max_count as f64 * bar_width as f64).round() as usize
            }
            .min(bar_width);
            let bar = format!(
                "{}{}",
                "█".repeat(filled),
                "░".repeat(bar_width.saturating_sub(filled))
            );
            Line::from(vec![
                Span::styled(
                    format!("  {:>7}-{:>7}us ", low, high),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(bar, Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!(" {:>3}", count),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

// ── Rejection reasons panel ──────────────────────────────────────────────────

fn render_fill_window_overlay(f: &mut Frame, area: Rect, fill_window: &Option<FillWindowSnapshot>) {
    let Some(fill) = fill_window else {
        return;
    };

    let panel_width = 52u16;
    let panel_height = 8u16;
    if area.width <= panel_width + 1 || area.height <= panel_height + 1 {
        return;
    }

    let x = area.width - panel_width - 1;
    let y = area.height - panel_height - 1;
    let rect = Rect::new(x, y, panel_width, panel_height);
    f.render_widget(Clear, rect);

    let elapsed_ms = fill.elapsed.as_millis().min(fill.countdown.as_millis()) as u64;
    let total_ms = fill.countdown.as_millis().max(1) as u64;
    let remaining_ms = total_ms.saturating_sub(elapsed_ms);
    let progress = elapsed_ms as f64 / total_ms as f64;
    let bar_width = rect.width.saturating_sub(20) as usize;
    let filled = (progress * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    let bar = format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(bar_width.saturating_sub(filled))
    );

    let drift_color = match fill.drift_pct {
        Some(d) if d > 1.5 => Color::Red,
        Some(d) if d > 1.0 => Color::Yellow,
        Some(_) => Color::Green,
        None => Color::Cyan,
    };
    let current_price = fill
        .current_price
        .map(|p| format!("{:.3}", p))
        .unwrap_or_else(|| "--".to_string());
    let drift = fill
        .drift_pct
        .map(|p| format!("{:.2}%", p))
        .unwrap_or_else(|| "--".to_string());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(drift_color))
        .title(Span::styled(
            " FILL WINDOW ",
            Style::default()
                .fg(drift_color)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let lines = vec![
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} {}", fill.side, shorten_token(&fill.token_id, 20)),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  price  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.3} -> {}", fill.entry_price, current_price),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  drift  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                drift,
                Style::default()
                    .fg(drift_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  elapsed ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} ms", elapsed_ms),
                Style::default().fg(Color::White),
            ),
            Span::styled(" / ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} ms", total_ms),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  remain ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} ms", remaining_ms),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(bar, Style::default().fg(drift_color)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ── Markets tab (Tab 2) ──────────────────────────────────────────────────────

fn render_markets_tab(
    f: &mut Frame,
    area: Rect,
    book_store: &OrderBookStore,
    market_search: &str,
    selected_market_idx: usize,
    markets: &[String],
    subscribed_markets: &[String],
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    // ── Left: Market list with search filter ─────────────────────────────
    let all_markets: Vec<String> = {
        let mut m: Vec<String> = markets.to_vec();
        for tid in book_store.token_ids() {
            if !m.contains(&tid) {
                m.push(tid);
            }
        }
        m
    };

    let filtered: Vec<&String> = if market_search.is_empty() {
        all_markets.iter().collect()
    } else {
        let q = market_search.to_lowercase();
        all_markets
            .iter()
            .filter(|m| m.to_lowercase().contains(&q))
            .collect()
    };

    let selected_idx = selected_market_idx.min(filtered.len().saturating_sub(1));

    let search_title = if market_search.is_empty() {
        format!(
            " MARKETS [/] search [Space] toggle | active={} ",
            subscribed_markets.len()
        )
    } else {
        format!(
            " MARKETS [{}] [Space] toggle | active={} ",
            market_search,
            subscribed_markets.len()
        )
    };

    let list_lines: Vec<Line> = filtered
        .iter()
        .enumerate()
        .map(|(i, tid)| {
            let short = shorten_token(tid, 32);
            let mid = book_store
                .get_mid_price(tid)
                .map(|p| format!(" ${:.3}", p as f64 / 1000.0))
                .unwrap_or_else(|| " --".to_string());
            let subscribed = subscribed_markets.iter().any(|m| m == *tid);
            let marker = if subscribed { "●" } else { "○" };

            if i == selected_idx {
                Line::from(vec![Span::styled(
                    format!(" > {marker} {short}{mid}"),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                        .bg(Color::DarkGray),
                )])
            } else {
                Line::from(vec![
                    Span::styled(
                        format!("   {marker} {short}"),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(mid, Style::default().fg(Color::DarkGray)),
                ])
            }
        })
        .collect();

    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            search_title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let list_para = Paragraph::new(list_lines)
        .block(list_block)
        .wrap(Wrap { trim: true });
    f.render_widget(list_para, cols[0]);

    // ── Right: Order book depth for selected market ──────────────────────
    let selected_token = filtered.get(selected_idx).map(|s| s.as_str());
    let book = selected_token.and_then(|t| book_store.get_book_snapshot(t));

    let depth_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(Span::styled(
            format!(
                " ORDER BOOK DEPTH {} ",
                selected_token
                    .map(|t| shorten_token(t, 16))
                    .unwrap_or_default()
            ),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));

    if let Some(book) = book {
        let inner = depth_block.inner(cols[1]);
        f.render_widget(depth_block, cols[1]);

        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // Bids (descending by price, green)
        let bid_header = Row::new(vec![
            Cell::from("Bid Price").style(
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Size").style(
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let bid_rows: Vec<Row> = book
            .bids
            .iter()
            .rev()
            .take(15)
            .map(|(&price, &size)| {
                Row::new(vec![
                    Cell::from(format!("{:.3}", price as f64 / 1000.0))
                        .style(Style::default().fg(Color::Green)),
                    Cell::from(format!("{:.1}", size as f64 / 1000.0))
                        .style(Style::default().fg(Color::White)),
                ])
            })
            .collect();

        let bid_table = Table::new(
            bid_rows,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        )
        .header(bid_header)
        .block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        f.render_widget(bid_table, halves[0]);

        // Asks (ascending by price, red)
        let ask_header = Row::new(vec![
            Cell::from("Ask Price")
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Cell::from("Size").style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ]);
        let ask_rows: Vec<Row> = book
            .asks
            .iter()
            .take(15)
            .map(|(&price, &size)| {
                Row::new(vec![
                    Cell::from(format!("{:.3}", price as f64 / 1000.0))
                        .style(Style::default().fg(Color::Red)),
                    Cell::from(format!("{:.1}", size as f64 / 1000.0))
                        .style(Style::default().fg(Color::White)),
                ])
            })
            .collect();

        let ask_table = Table::new(
            ask_rows,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        )
        .header(ask_header);
        f.render_widget(ask_table, halves[1]);
    } else {
        let inner = depth_block.inner(cols[1]);
        f.render_widget(depth_block, cols[1]);
        let na = Paragraph::new(Line::from(Span::styled(
            "  Select a market to view order book depth",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(na, inner);
    }
}

// ── History tab (Tab 3) ──────────────────────────────────────────────────────

fn render_history_tab(
    f: &mut Frame,
    area: Rect,
    snap: &PortfolioSnapshot,
    history_scroll: usize,
    state: &TuiState,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(52),
            Constraint::Percentage(24),
            Constraint::Percentage(24),
        ])
        .split(area);

    // ── Top: Trade history table ─────────────────────────────────────────
    let header_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let header = Row::new(vec![
        Cell::from("Token").style(header_style),
        Cell::from("Side").style(header_style),
        Cell::from("Entry").style(header_style),
        Cell::from("Exit").style(header_style),
        Cell::from("Shares").style(header_style),
        Cell::from("P&L").style(header_style),
        Cell::from("Close Why").style(header_style),
        Cell::from("Opened").style(header_style),
        Cell::from("Duration").style(header_style),
    ]);

    let inner_h = outer[0].height.saturating_sub(3) as usize;
    let max_scroll = snap.closed_trades.len().saturating_sub(inner_h);
    let scroll = history_scroll.min(max_scroll);

    let rows: Vec<Row> = if snap.closed_trades.is_empty() {
        vec![Row::new(vec![
            Cell::from("  No closed trades yet").style(Style::default().fg(Color::DarkGray))
        ])]
    } else {
        snap.closed_trades
            .iter()
            .rev()
            .skip(scroll)
            .take(inner_h)
            .map(|t| {
                let side_style = match t.side {
                    OrderSide::Buy => Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    OrderSide::Sell => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                };
                let pnl_sty = Style::default()
                    .fg(pnl_color(t.realized_pnl))
                    .add_modifier(Modifier::BOLD);
                Row::new(vec![
                    Cell::from(shorten_token(&t.token_id, 12))
                        .style(Style::default().fg(Color::White)),
                    Cell::from(format!("{}", t.side)).style(side_style),
                    Cell::from(format!("{:.3}", t.entry_price))
                        .style(Style::default().fg(Color::DarkGray)),
                    Cell::from(format!("{:.3}", t.exit_price))
                        .style(Style::default().fg(Color::White)),
                    Cell::from(format!("{:.2}", t.shares)),
                    Cell::from(format!("{:>+.4}", t.realized_pnl)).style(pnl_sty),
                    Cell::from(human_close_reason(&t.reason))
                        .style(Style::default().fg(Color::DarkGray)),
                    Cell::from(t.opened_at.clone()).style(Style::default().fg(Color::DarkGray)),
                    Cell::from(format_age(t.duration_secs))
                        .style(Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect()
    };

    let trade_widths = [
        Constraint::Length(24),
        Constraint::Length(5),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Length(7),
    ];

    let trade_table = Table::new(rows, trade_widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(Span::styled(
                format!(" TRADE HISTORY ({}) ", snap.closed_trades.len()),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(trade_table, outer[0]);

    // ── Middle: execution scorecard ───────────────────────────────
    let score_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[1]);
    let mut tag_counts: HashMap<String, usize> = HashMap::new();
    let mut slip_sum = 0.0;
    let mut delay_sum = 0.0;
    for t in &snap.closed_trades {
        slip_sum += t.scorecard_slippage_bps;
        delay_sum += t.scorecard_queue_delay_ms as f64;
        for tag in &t.scorecard_tags {
            *tag_counts.entry(tag.clone()).or_insert(0) += 1;
        }
    }
    let trade_count = snap.closed_trades.len().max(1) as f64;
    let left_lines = vec![
        Line::from(format!("  Avg slippage: {:.2} bps", slip_sum / trade_count)),
        Line::from(format!(
            "  Avg queue delay: {:.1} ms",
            delay_sum / trade_count
        )),
        Line::from(format!("  Trades scored: {}", snap.closed_trades.len())),
        Line::from(format!(
            "  Shadow realism gap: {:.2} bps",
            state.execution_summary.shadow_realism_gap_bps
        )),
    ];
    f.render_widget(
        Paragraph::new(left_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(Span::styled(
                    " EXECUTION SCORECARD ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        score_layout[0],
    );
    let mut tags: Vec<(String, usize)> = tag_counts.into_iter().collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1));
    let right_lines: Vec<Line> = if tags.is_empty() {
        vec![Line::from("  No score tags yet")]
    } else {
        tags.into_iter()
            .take(8)
            .map(|(k, v)| Line::from(format!("  {:<18} {}", k, v)))
            .collect()
    };
    f.render_widget(
        Paragraph::new(right_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue))
                .title(Span::styled(
                    " OUTCOME TAGS ",
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        score_layout[1],
    );

    let exp_lines = vec![
        Line::from(format!(
            "  Variant A fills={} pnl={:+.2}",
            state.experiment_metrics.variant_a_fills,
            state.experiment_metrics.variant_a_realized_pnl
        )),
        Line::from(format!(
            "  Variant B fills={} pnl={:+.2}",
            state.experiment_metrics.variant_b_fills,
            state.experiment_metrics.variant_b_realized_pnl
        )),
        Line::from(format!(
            "  Toggles: sizingB={} autoclaimB={} driftB={}",
            state.experiment_switches.sizing_variant_b,
            state.experiment_switches.autoclaim_variant_b,
            state.experiment_switches.drift_variant_b
        )),
    ];
    let exp_rect = Rect::new(
        score_layout[0].x,
        score_layout[0].y.saturating_add(1),
        score_layout[0].width.saturating_sub(2),
        4,
    );
    f.render_widget(
        Paragraph::new(exp_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    " A/B METRICS ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        exp_rect,
    );

    // ── Bottom: PnL attribution + Drawdown ───────────────────────────────
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[2]);

    // PnL attribution by token
    let mut pnl_by_token: HashMap<String, f64> = HashMap::new();
    for t in &snap.closed_trades {
        *pnl_by_token
            .entry(shorten_token(&t.token_id, 16))
            .or_insert(0.0) += t.realized_pnl;
    }
    let mut pnl_list: Vec<(String, f64)> = pnl_by_token.into_iter().collect();
    pnl_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let pnl_lines: Vec<Line> = pnl_list
        .iter()
        .map(|(tok, pnl)| {
            Line::from(vec![
                Span::styled(format!("  {:<18}", tok), Style::default().fg(Color::White)),
                Span::styled(
                    format!("{:>+.4} USDC", pnl),
                    Style::default()
                        .fg(pnl_color(*pnl))
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        })
        .collect();

    let pnl_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(Span::styled(
            " PnL ATTRIBUTION ",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ));
    let pnl_para = Paragraph::new(pnl_lines)
        .block(pnl_block)
        .wrap(Wrap { trim: true });
    f.render_widget(pnl_para, bottom[0]);

    // Drawdown stats + sparkline
    let dd_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            " DRAWDOWN TRACKER ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
    let dd_inner = dd_block.inner(bottom[1]);
    f.render_widget(dd_block, bottom[1]);

    let dd_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(2)])
        .split(dd_inner);

    let dd_lines = vec![
        Line::from(vec![
            Span::styled("  Max Drawdown  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.2}%", snap.max_drawdown_pct),
                Style::default()
                    .fg(if snap.max_drawdown_pct > 5.0 {
                        Color::Red
                    } else {
                        Color::Yellow
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  High-Water    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("${:.2}", snap.high_water_mark),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    f.render_widget(Paragraph::new(dd_lines), dd_layout[0]);

    // Sparkline with HWM indicator
    if snap.equity_curve.len() >= 2 {
        let min_val = snap
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max_val = snap
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let range = max_val - min_val;
        let spark_data: Vec<u64> = snap
            .equity_curve
            .iter()
            .map(|&v| {
                if range < 0.001 {
                    50
                } else {
                    ((v - min_val) / range * 99.0) as u64 + 1
                }
            })
            .collect();

        let spark = Sparkline::default()
            .data(&spark_data)
            .style(Style::default().fg(Color::Magenta))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(Span::styled(
                        " equity ",
                        Style::default().fg(Color::DarkGray),
                    )),
            );
        f.render_widget(spark, dd_layout[1]);
    }
}

// ── Config tab (Tab 4) ───────────────────────────────────────────────────────

fn render_config_tab(
    f: &mut Frame,
    area: Rect,
    risk: &RiskConfigSnap,
    positions: &[PositionSnap],
    config_selected: usize,
    config_editing: bool,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // ── Left: Risk parameters form ───────────────────────────────────────
    let params = [
        (
            "Max Daily Loss",
            format!("{:.1}%", risk.max_daily_loss_pct * 100.0),
        ),
        (
            "Max Positions",
            format!("{}", risk.max_concurrent_positions),
        ),
        (
            "Max Order Size",
            format!("${:.0}", risk.max_single_order_usdc),
        ),
        ("Max Orders/sec", format!("{}", risk.max_orders_per_second)),
        (
            "Trading Enabled",
            if risk.trading_enabled {
                "YES".into()
            } else {
                "NO".into()
            },
        ),
        (
            "VaR Threshold",
            format!("{:.1}%", risk.var_threshold_pct * 100.0),
        ),
    ];

    let form_lines: Vec<Line> = params
        .iter()
        .enumerate()
        .map(|(i, (label, value))| {
            let is_selected = i == config_selected;
            let label_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let val_style = if is_selected && config_editing {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK)
            } else if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { " > " } else { "   " };

            Line::from(vec![
                Span::styled(format!("{prefix}{:<18}", label), label_style),
                Span::styled(value.clone(), val_style),
            ])
        })
        .collect();

    let form_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " RISK PARAMETERS [e] edit [r] reset breaker ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let form_para = Paragraph::new(form_lines)
        .block(form_block)
        .wrap(Wrap { trim: false });
    f.render_widget(form_para, cols[0]);

    // ── Right: Circuit breaker + exposure ────────────────────────────────
    let right_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(cols[1]);

    // Circuit breaker status
    let (cb_status, cb_color) = if risk.circuit_breaker_tripped {
        ("  !! CIRCUIT BREAKER TRIPPED !!", Color::Red)
    } else {
        ("  CIRCUIT BREAKER OK", Color::Green)
    };

    let cb_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            cb_status,
            Style::default().fg(cb_color).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Daily P&L   ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>+.2} USDC", risk.daily_pnl),
                Style::default()
                    .fg(pnl_color(risk.daily_pnl))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  VaR Exposure ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("${:.2}", risk.rolling_exposure_usdc),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let cb_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if risk.circuit_breaker_tripped {
            Color::Red
        } else {
            Color::Green
        }))
        .title(Span::styled(
            " CIRCUIT BREAKER ",
            Style::default()
                .fg(if risk.circuit_breaker_tripped {
                    Color::Red
                } else {
                    Color::Green
                })
                .add_modifier(Modifier::BOLD),
        ));
    let cb_para = Paragraph::new(cb_lines).block(cb_block);
    f.render_widget(cb_para, right_layout[0]);

    // Exposure heatmap by market
    let mut exposure_by_market: HashMap<String, f64> = HashMap::new();
    for pos in positions {
        *exposure_by_market
            .entry(shorten_token(&pos.token_id, 14))
            .or_insert(0.0) += pos.usdc_spent;
    }
    let mut exposure_list: Vec<(String, f64)> = exposure_by_market.into_iter().collect();
    exposure_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let max_exposure = exposure_list.iter().map(|(_, v)| *v).fold(0.0f64, f64::max);
    let bar_max = right_layout[1].width.saturating_sub(24) as f64;

    let expo_lines: Vec<Line> = exposure_list
        .iter()
        .map(|(tok, val)| {
            let bar_len = if max_exposure > 0.0 {
                (val / max_exposure * bar_max) as usize
            } else {
                0
            };
            let bar = "█".repeat(bar_len);
            let heat_color = if *val > max_exposure * 0.8 {
                Color::Red
            } else if *val > max_exposure * 0.5 {
                Color::Yellow
            } else {
                Color::Green
            };
            Line::from(vec![
                Span::styled(format!("  {:<16}", tok), Style::default().fg(Color::White)),
                Span::styled(bar, Style::default().fg(heat_color)),
                Span::styled(
                    format!(" ${:.0}", val),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        })
        .collect();

    let expo_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            " EXPOSURE HEATMAP ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let expo_para = Paragraph::new(expo_lines)
        .block(expo_block)
        .wrap(Wrap { trim: true });
    f.render_widget(expo_para, right_layout[1]);

    let exp_rect = Rect::new(
        area.x + 2,
        area.y + area.height.saturating_sub(6),
        area.width.saturating_sub(4),
        5,
    );
    render_experiment_switches(f, exp_rect, config_selected, config_editing);
}

// ── Performance tab (Tab 5) ──────────────────────────────────────────────────

fn render_performance_tab(
    f: &mut Frame,
    area: Rect,
    snap: &PortfolioSnapshot,
    perf: &PerfSnapshot,
    kernel: &KernelSnapshot,
    state: &TuiState,
    fill_latency_samples: &[u64],
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ])
        .split(area);

    let signal_line = match (perf.sig_avg_us, perf.sig_p99_us, perf.sig_count) {
        (Some(avg), Some(p99), count) => format!(
            "  Signal latency: avg={}us  p99={}us  samples={}",
            avg, p99, count
        ),
        _ => "  Signal latency: --".to_string(),
    };
    let fill_line = match (perf.fill_p50_us, perf.fill_p95_us, perf.fill_p99_us) {
        (Some(p50), Some(p95), Some(p99)) => format!(
            "  Fill latency: p50={}us  p95={}us  p99={}us",
            p50, p95, p99
        ),
        _ => "  Fill latency: --".to_string(),
    };
    let quality = match (perf.sig_p99_us, perf.fill_p99_us) {
        (Some(sig), Some(fill)) if sig <= 5_000 && fill <= 120_000 => ("WORLD-CLASS", Color::Green),
        (Some(sig), Some(fill)) if sig <= 15_000 && fill <= 250_000 => ("GOOD", Color::Yellow),
        (Some(_), Some(_)) => ("DEGRADED", Color::Red),
        _ => ("WARMING UP", Color::DarkGray),
    };
    let top_lines = vec![
        Line::from(vec![
            Span::styled(" Quality: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                quality.0,
                Style::default().fg(quality.1).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  | msgs/s {:.0} (total {})",
                    perf.msgs_per_sec, perf.msg_total
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(signal_line),
        Line::from(fill_line),
    ];
    f.render_widget(
        Paragraph::new(top_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    " PERFORMANCE OVERVIEW ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        outer[0],
    );

    render_fill_latency_histogram(f, outer[1], fill_latency_samples);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[2]);
    render_kernel_telemetry(
        f,
        middle[0],
        kernel,
        kernel.available
            && (kernel.rtt.p99_us > RTT_ALERT_US || kernel.syscall.send_avg_us > SYSCALL_ALERT_US),
    );

    let exec_lines = vec![
        Line::from(format!("  Trades: {}", state.execution_summary.trades)),
        Line::from(format!(
            "  Fill rate: {:.1}%",
            state.execution_summary.fill_rate_pct
        )),
        Line::from(format!(
            "  Reject rate: {:.1}%",
            state.execution_summary.reject_rate_pct
        )),
        Line::from(format!(
            "  Avg slippage: {:.2} bps",
            state.execution_summary.avg_slippage_bps
        )),
        Line::from(format!(
            "  Avg queue delay: {:.1} ms",
            state.execution_summary.avg_queue_delay_ms
        )),
        Line::from(format!(
            "  Shadow realism gap: {:.2} bps",
            state.execution_summary.shadow_realism_gap_bps
        )),
    ];
    f.render_widget(
        Paragraph::new(exec_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(Span::styled(
                    " EXECUTION KPI ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        middle[1],
    );

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[3]);

    let exp_lines = vec![
        Line::from(format!(
            "  Variant A fills={} pnl={:+.2}",
            state.experiment_metrics.variant_a_fills,
            state.experiment_metrics.variant_a_realized_pnl
        )),
        Line::from(format!(
            "  Variant B fills={} pnl={:+.2}",
            state.experiment_metrics.variant_b_fills,
            state.experiment_metrics.variant_b_realized_pnl
        )),
        Line::from(format!(
            "  Toggles: sizingB={} autoclaimB={} driftB={}",
            state.experiment_switches.sizing_variant_b,
            state.experiment_switches.autoclaim_variant_b,
            state.experiment_switches.drift_variant_b
        )),
    ];
    f.render_widget(
        Paragraph::new(exp_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .title(Span::styled(
                    " A/B EXPERIMENTS ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        bottom[0],
    );

    let win_rate = if snap.closed_trades.is_empty() {
        0.0
    } else {
        let wins = snap
            .closed_trades
            .iter()
            .filter(|t| t.realized_pnl > 0.0)
            .count() as f64;
        (wins / snap.closed_trades.len() as f64) * 100.0
    };
    let rej_total: usize = state.rejection_counts.values().sum();
    let health_lines = vec![
        Line::from(format!("  Closed trades: {}", snap.closed_trades.len())),
        Line::from(format!("  Win rate: {:.1}%", win_rate)),
        Line::from(format!("  Max drawdown: {:.2}%", snap.max_drawdown_pct)),
        Line::from(format!("  Rejections (session): {}", rej_total)),
    ];
    f.render_widget(
        Paragraph::new(health_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(Span::styled(
                    " TRADING HEALTH ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        bottom[1],
    );
}

fn render_twin_tab(f: &mut Frame, area: Rect, twin: &TwinUiSnapshot) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(8),
        ])
        .split(area);

    let status_text = if twin.enabled { "ACTIVE" } else { "INACTIVE" };
    let status_color = if twin.enabled {
        Color::Green
    } else {
        Color::DarkGray
    };
    let nav_color = if twin.realized_pnl + twin.unrealized_pnl >= 0.0 {
        Color::Green
    } else {
        Color::Red
    };
    let header = vec![
        Line::from(vec![
            Span::styled(" Status: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Generation: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", twin.generation),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::styled(" NAV: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("${:.2}", twin.nav), Style::default().fg(nav_color)),
            Span::styled("   Return: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:+.2}%", twin.nav_return_pct),
                Style::default().fg(nav_color),
            ),
            Span::styled("   Realized: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:+.2}", twin.realized_pnl),
                Style::default().fg(nav_color),
            ),
            Span::styled("   Unrealized: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:+.2}", twin.unrealized_pnl),
                Style::default().fg(nav_color),
            ),
        ]),
    ];
    f.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    " BLINK TWIN STATUS ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        outer[0],
    );

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[1]);

    let cfg_lines = vec![
        Line::from(format!("  Extra latency: {} ms", twin.extra_latency_ms)),
        Line::from(format!(
            "  Slippage penalty: {:.2} bps",
            twin.slippage_penalty_bps
        )),
        Line::from(format!("  Drift multiplier: {:.3}", twin.drift_multiplier)),
    ];
    f.render_widget(
        Paragraph::new(cfg_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(Span::styled(
                    " TWIN CONFIG ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        middle[0],
    );

    let health_lines = vec![
        Line::from(format!("  Signals: {}", twin.total_signals)),
        Line::from(format!("  Win rate: {:.1}%", twin.win_rate_pct)),
        Line::from(format!("  Open positions: {}", twin.open_positions)),
        Line::from(format!("  Closed trades: {}", twin.closed_trades)),
    ];
    f.render_widget(
        Paragraph::new(health_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(Span::styled(
                    " TWIN HEALTH ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        middle[1],
    );

    let perf = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[2]);

    let risk_lines = vec![
        Line::from(format!("  Max drawdown: {:.2}%", twin.max_drawdown_pct)),
        Line::from(format!("  High-water NAV: ${:.2}", twin.high_water_mark)),
        Line::from(format!(
            "  Fill ratio: {:.1}%",
            if twin.filled_orders + twin.aborted_orders + twin.skipped_orders > 0 {
                (twin.filled_orders as f64
                    / (twin.filled_orders + twin.aborted_orders + twin.skipped_orders) as f64)
                    * 100.0
            } else {
                0.0
            }
        )),
    ];
    f.render_widget(
        Paragraph::new(risk_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue))
                .title(Span::styled(
                    " TWIN PERFORMANCE ",
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        perf[0],
    );

    let spark_data: Vec<u64> = if twin.equity_curve.len() >= 2 {
        let min_val = twin
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max_val = twin
            .equity_curve
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let range = (max_val - min_val).max(1e-9);
        twin.equity_curve
            .iter()
            .map(|&v| (((v - min_val) / range) * 100.0).round() as u64)
            .collect()
    } else {
        vec![50, 50]
    };
    let spark = Sparkline::default()
        .data(&spark_data)
        .style(Style::default().fg(Color::Green))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .title(Span::styled(
                    " TWIN EQUITY ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )),
        );
    f.render_widget(spark, perf[1]);

    let bottom_rows = vec![
        Row::new(vec![
            Cell::from("filled_orders"),
            Cell::from(twin.filled_orders.to_string()),
        ]),
        Row::new(vec![
            Cell::from("aborted_orders"),
            Cell::from(twin.aborted_orders.to_string()),
        ]),
        Row::new(vec![
            Cell::from("skipped_orders"),
            Cell::from(twin.skipped_orders.to_string()),
        ]),
    ];
    let bottom_table = Table::new(
        bottom_rows,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .header(Row::new(vec![
        Cell::from("Metric").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Value").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green))
            .title(Span::styled(
                " TWIN EXECUTION BREAKDOWN ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(bottom_table, outer[3]);
}

fn render_rejection_trend_overlay(
    f: &mut Frame,
    area: Rect,
    trends: &HashMap<String, Vec<RejectionTrendPoint>>,
) {
    if area.width < 16 || area.height < 4 {
        return;
    }
    let mut top: Vec<(&String, usize)> = trends
        .iter()
        .map(|(k, v)| (k, v.iter().map(|p| p.count).sum()))
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let lines: Vec<Line> = if top.is_empty() {
        vec![Line::from(" no 24h data")]
    } else {
        top.into_iter()
            .take((area.height.saturating_sub(2)) as usize)
            .map(|(k, c)| Line::from(format!(" {:<16} {}", k, c)))
            .collect()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " REJ 24H ",
            Style::default().fg(Color::DarkGray),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_experiment_switches(f: &mut Frame, area: Rect, _sel: usize, _editing: bool) {
    let lines = vec![
        Line::from(" [x] A/B sizing   [x] A/B autoclaim   [x] A/B drift "),
        Line::from(" Metrics shown in History scorecard"),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " STRATEGY EXPERIMENTS ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

// ── Notifications (overlay) ──────────────────────────────────────────────────

fn render_notifications(f: &mut Frame, area: Rect, notifications: &[Notification]) {
    let active: Vec<&Notification> = notifications
        .iter()
        .filter(|n| !n.is_expired())
        .rev()
        .take(3)
        .collect();

    if active.is_empty() {
        return;
    }

    let notif_width = 42u16;
    let notif_height = 3u16;

    for (i, notif) in active.iter().enumerate() {
        let y = area.y + 1 + (i as u16 * notif_height);
        let x = area.width.saturating_sub(notif_width + 1);
        if y + notif_height > area.height {
            break;
        }

        let rect = Rect::new(x, y, notif_width, notif_height);
        f.render_widget(Clear, rect);

        let color = notif.color();
        let elapsed = notif.created_at.elapsed().as_secs();
        let remaining = notif.ttl.as_secs().saturating_sub(elapsed);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color))
            .title(Span::styled(
                format!(
                    " {} [{remaining}s] ",
                    match notif.kind {
                        NotificationKind::Info => "INFO",
                        NotificationKind::Success => "OK",
                        NotificationKind::Warning => "WARN",
                        NotificationKind::Critical => "CRIT",
                    }
                ),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));

        let msg = if notif.message.len() > (notif_width - 4) as usize {
            format!("{}...", &notif.message[..(notif_width - 7) as usize])
        } else {
            notif.message.clone()
        };

        let para = Paragraph::new(Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(Color::White),
        )))
        .block(block);
        f.render_widget(para, rect);
    }
}

// ── Hint bar ─────────────────────────────────────────────────────────────────

fn render_hint(
    f: &mut Frame,
    area: Rect,
    active_tab: usize,
    search_active: bool,
    modern_theme: bool,
) {
    let global = vec![
        Span::styled("  [q] Quit  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[p] Pause  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[t] Theme  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "[1-6/Tab] Switch tab  ",
            Style::default().fg(Color::DarkGray),
        ),
    ];

    let tab_hints = if search_active {
        vec![
            Span::styled("[Esc] Close search  ", Style::default().fg(Color::Yellow)),
            Span::styled("[Enter] Confirm  ", Style::default().fg(Color::DarkGray)),
        ]
    } else {
        match active_tab {
            TAB_DASHBOARD => vec![Span::styled(
                "[Arrow/PgUp/PgDn] Scroll log",
                Style::default().fg(Color::DarkGray),
            )],
            TAB_MARKETS => vec![
                Span::styled("[/] Search  ", Style::default().fg(Color::DarkGray)),
                Span::styled("[Arrow] Navigate  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "[Space] Toggle subscription",
                    Style::default().fg(Color::DarkGray),
                ),
            ],
            TAB_HISTORY => vec![Span::styled(
                "[Arrow/PgUp/PgDn] Scroll",
                Style::default().fg(Color::DarkGray),
            )],
            TAB_CONFIG => vec![
                Span::styled("[Arrow] Select  ", Style::default().fg(Color::DarkGray)),
                Span::styled("[e] Edit  ", Style::default().fg(Color::DarkGray)),
                Span::styled("[r] Reset breaker", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "  [x/c/v] A/B toggles",
                    Style::default().fg(Color::DarkGray),
                ),
            ],
            TAB_PERFORMANCE => vec![Span::styled(
                "Live performance dashboards",
                Style::default().fg(Color::DarkGray),
            )],
            TAB_TWIN => vec![Span::styled(
                "Blink Twin live status + outcomes",
                Style::default().fg(Color::DarkGray),
            )],
            _ => vec![],
        }
    };

    let mut spans = global;
    spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled(
        if modern_theme {
            "Mode: Modern  "
        } else {
            "Mode: Native  "
        },
        Style::default().fg(if modern_theme { MONO_BLUE } else { Color::Gray }),
    ));
    spans.extend(tab_hints);

    let line = Line::from(spans);
    f.render_widget(Paragraph::new(line), area);
}

// ── Council of Agents Visualization ──────────────────────────────────────────

fn render_council_of_agents(f: &mut Frame, area: Rect) {
    let agents = [
        ("Aura", "Arch", MONO_PINK),
        ("Qsigma", "Quant", MONO_GREEN),
        ("Wraith", "Stealth", MONO_BLUE),
        ("Sentinel", "Risk", MONO_GOLD),
    ];

    let spans: Vec<Span> = agents
        .iter()
        .map(|(name, _role, color)| {
            Span::styled(
                format!(" {name} "),
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    let mut line_spans = vec![Span::styled(" Agents: ", Style::default().fg(MONO_GRAY))];
    for (i, span) in spans.into_iter().enumerate() {
        line_spans.push(span);
        if i < agents.len() - 1 {
            line_spans.push(Span::styled("•", Style::default().fg(MONO_GRAY)));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MONO_PURPLE))
        .title(Span::styled(
            " COUNCIL OF AGENTS ",
            Style::default()
                .fg(MONO_PURPLE)
                .add_modifier(Modifier::BOLD),
        ));

    let para = Paragraph::new(Line::from(line_spans))
        .block(block)
        .alignment(Alignment::Center);
    f.render_widget(para, area);
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[inline]
fn pnl_color(v: f64) -> Color {
    if v > 0.0 {
        Color::Green
    } else if v < 0.0 {
        Color::Red
    } else {
        Color::DarkGray
    }
}

fn shorten_token(token_id: &str, max_len: usize) -> String {
    if token_id.len() <= max_len {
        token_id.to_string()
    } else {
        let tail = 4.min(token_id.len());
        let head = max_len.saturating_sub(tail + 1);
        format!(
            "{}...{}",
            &token_id[..head],
            &token_id[token_id.len() - tail..]
        )
    }
}

fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn human_close_reason(reason: &str) -> String {
    if reason == "autoclaim@market_not_live" || reason == "twin_autoclaim@market_not_live" {
        return "Event ended / market not live".to_string();
    }
    if reason == "backtest-end" {
        return "Session ended".to_string();
    }
    if reason.eq_ignore_ascii_case("loss") {
        return "Risk stop / loss exit".to_string();
    }
    if let Some(rest) = reason.strip_prefix("autoclaim@") {
        if let Some((tp, frac)) = rest.split_once('[') {
            let frac = frac.trim_end_matches(']');
            return format!("Take-profit {tp} (close {frac})");
        }
        return format!("Take-profit {rest}");
    }
    if reason.to_ascii_lowercase().contains("rn1") && reason.to_ascii_lowercase().contains("close")
    {
        return "RN1 close/flip detected".to_string();
    }
    reason.to_string()
}
