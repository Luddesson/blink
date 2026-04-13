//! Blink Engine — entry point.
//!
//! Modes controlled by `.env` variables:
//! - Default (read-only): connect, maintain order books, log RN1 activity.
//! - `PAPER_TRADING=true`: simulate mirror orders with virtual $100 USDC.
//! - Web UI is the active dashboard. The legacy ratatui TUI is archived and no
//!   longer launched, even if `TUI=true` is present in the environment.
//!   Tracing is always persisted to `logs/engine.log` + per-session log files.

use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc, Mutex};
use std::time::Duration;
use std::io::{BufRead, BufReader, Write};
use futures_util::FutureExt as _; // .catch_unwind() on futures

use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;

// All modules are declared in lib.rs; we access them through the crate name.
use engine::activity_log::{EntryKind, new_activity_log, push as log_push};
use engine::agent_rpc::{AgentRpcState, run_agent_rpc_server};
use engine::backtest_engine::{BacktestConfig, BacktestEngine, load_ticks_csv};
use engine::clickhouse_logger::{ClickHouseLogger, WarehouseEvent};
use engine::clob_client::ClobClient;
use engine::config::Config;
use engine::gas_oracle::GasOracle;
use engine::latency_tracker::LatencyStats;
use engine::order_book::OrderBookStore;
use engine::paper_engine::PaperEngine;
use engine::tick_recorder::{TickRecord, TickRecorder};
use engine::tui_app::run_tui;
use engine::blink_twin::TwinSnapshot;
use engine::types::RN1Signal;
use engine::r2_uploader;
use engine::web_server::{AppState, run_web_server};
use engine::ws_client::run_ws;
use engine::rn1_poller::{run_rn1_poller, Rn1PollDiagnostics, Rn1PollDiagnosticsHandle};
use engine::ws_client::WsHealthMetrics;
use std::collections::HashMap;

use bpf_probes::BpfTelemetry;

// ─── Entry point ─────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // ── Load .env ────────────────────────────────────────────────────────
    match dotenvy::dotenv() {
        Ok(_) => {}
        Err(dotenvy::Error::Io(_)) => eprintln!("Note: no .env — using process environment"),
        Err(_) => eprintln!("Warning: .env has a formatting issue"),
    }

    // ── Panic hook — save portfolio state before dying ───────────────────
    {
        let state_path = std::env::var("PAPER_STATE_PATH")
            .unwrap_or_else(|_| "logs\\paper_portfolio_state.json".to_string());
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            eprintln!("BLINK ENGINE PANIC: {info}");
            // Best-effort: write a sentinel file so restart scripts know it panicked
            let _ = std::fs::write(
                format!("{state_path}.panic"),
                format!("{}", info),
            );
            original(info);
        }));
    }

    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--backtest") {
        let csv_path = args
            .get(pos + 1)
            .expect("--backtest requires a CSV file path");
        let output_path = args
            .iter()
            .position(|a| a == "--output")
            .and_then(|p| args.get(p + 1).cloned());
        return run_backtest(csv_path, output_path.as_deref());
    }
    let preflight_live    = args.iter().any(|a| a == "--preflight-live");
    let emergency_stop    = args.iter().any(|a| a == "--emergency-stop");

    // ── Feature flags ────────────────────────────────────────────────────
    let paper_mode = env_flag("PAPER_TRADING");
    let live_mode  = env_flag("LIVE_TRADING");
    let tui_requested = (paper_mode || live_mode) && env_flag("TUI");
    let web_ui_requested = env_flag("WEB_UI");
    let web_ui_enabled = web_ui_requested || paper_mode || live_mode || tui_requested;
    let tui_mode = false;

    if live_mode && paper_mode {
        eprintln!("Error: Cannot enable both PAPER_TRADING and LIVE_TRADING. Pick one.");
        std::process::exit(1);
    }
    
    // ── Tracing: ALWAYS persist logs to disk + per-session file ───────────
    std::fs::create_dir_all("logs").ok();
    std::fs::create_dir_all("logs\\sessions").ok();

    let session_stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let session_filename = format!("engine-session-{session_stamp}.log");
    let session_log_path = format!("logs\\sessions\\{session_filename}");
    let _ = std::fs::write("logs\\LATEST_SESSION_LOG.txt", format!("{session_log_path}\n"));

    let engine_file_appender = tracing_appender::rolling::daily("logs", "engine.log");
    let session_file_appender = tracing_appender::rolling::never("logs\\sessions", &session_filename);
    let (engine_writer, engine_guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .finish(engine_file_appender);
    let (session_writer, session_guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .finish(session_file_appender);
    let _log_guards = [engine_guard, session_guard];

    let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".into());
    let file_writers = engine_writer.and(session_writer);

    if tui_mode {
        tracing_subscriber::fmt()
            .with_writer(file_writers)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level)),
            )
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(file_writers.and(std::io::stderr))
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level)),
            )
            .with_target(true)
            .init();
        println!("\n╔══════════════════════════════════════════════════════╗");
        println!(  "║         BLINK ENGINE v0.2 — Shadow Maker Bot        ║");
        println!(  "╚══════════════════════════════════════════════════════╝\n");
    }

    // ── Config ────────────────────────────────────────────────────────────
    let config = Config::from_env()?;
    info!(paper_mode, tui_mode, rn1_wallet = %config.rn1_wallet, "Configuration loaded");
    config.validate_live_profile_contract()?;

    if preflight_live {
        run_preflight_live(&config).await?;
        return Ok(());
    }

    // ── Emergency stop (--emergency-stop) ────────────────────────────────
    if emergency_stop {
        run_emergency_stop(&config).await?;
        return Ok(());
    }

    let rn1_wallet = config.rn1_wallet.clone();
    let markets    = config.markets.clone();
    let config     = Arc::new(config);
    let book_store = Arc::new(OrderBookStore::new());
    let clob       = Arc::new(ClobClient::new(&config.clob_host));

    // ── Shared state ──────────────────────────────────────────────────────
    let ws_live        = Arc::new(AtomicBool::new(false));
    let trading_paused = Arc::new(AtomicBool::new(false));
    let activity       = new_activity_log();
    let shutdown       = Arc::new(AtomicBool::new(false));
    let msg_count      = Arc::new(AtomicU64::new(0));
    let latency        = Arc::new(Mutex::new(LatencyStats::new(config.latency_window_size)));
    let risk_status    = Arc::new(Mutex::new("OK".to_string()));
    let market_subscriptions = Arc::new(Mutex::new(markets.clone()));
    let ws_force_reconnect = Arc::new(AtomicBool::new(false));
    let ws_health_metrics = Arc::new(WsHealthMetrics::default());
    let rn1_diagnostics: Rn1PollDiagnosticsHandle = Arc::new(Mutex::new(Rn1PollDiagnostics::default()));

    log_push(&activity, EntryKind::Engine,
        format!("Engine started — PAPER={paper_mode} TUI={tui_mode} RN1={}...", &rn1_wallet[..10]));
    log_push(&activity, EntryKind::Engine, format!("Session log: {session_log_path}"));
    if tui_requested {
        log_push(
            &activity,
            EntryKind::Warn,
            "TUI request redirected: ratatui dashboard is archived; using the web UI instead".to_string(),
        );
    }
    if web_ui_enabled && !web_ui_requested {
        log_push(
            &activity,
            EntryKind::Engine,
            "Web UI auto-enabled for paper/live mode".to_string(),
        );
    }

    // ── eBPF kernel telemetry ───────────────────────────────────────────────
    let kernel_snapshot: Option<Arc<Mutex<bpf_probes::KernelSnapshot>>> =
        if env_flag("EBPF_TELEMETRY")
            || std::env::var("EBPF_TELEMETRY").is_err() {
            match BpfTelemetry::attach(std::process::id()).await {
                Ok(telemetry) => {
                    let handle = telemetry.snapshot_handle();
                    log_push(&activity, EntryKind::Engine,
                        format!("eBPF kernel telemetry attached (available={})", telemetry.is_available()));
                    // Keep telemetry alive for the process lifetime.
                    std::mem::forget(telemetry);
                    Some(handle)
                }
                Err(e) => {
                    log_push(&activity, EntryKind::Warn,
                        format!("eBPF telemetry failed: {e}"));
                    None
                }
            }
        } else {
            info!("EBPF_TELEMETRY explicitly disabled");
            None
        };

    // ── ClickHouse tick recorder (optional — activated by CLICKHOUSE_URL) ─
    let tick_tx: Option<crossbeam_channel::Sender<TickRecord>> =
        match std::env::var("CLICKHOUSE_URL").ok().filter(|s| !s.is_empty()) {
            Some(url) => {
                let (tx, rx) = crossbeam_channel::bounded::<TickRecord>(10_000);
                let recorder = TickRecorder::new(&url);
                let act = activity.clone();
                tokio::spawn(async move {
                    match recorder.ensure_schema().await {
                        Ok(()) => log_push(&act, EntryKind::Engine,
                            format!("ClickHouse connected: {url}")),
                        Err(e) => log_push(&act, EntryKind::Warn,
                            format!("ClickHouse schema error: {e}")),
                    }
                    recorder.run(rx).await;
                });
                info!("ClickHouse tick recording enabled");
                Some(tx)
            }
            None => {
                info!("CLICKHOUSE_URL not set — tick recording disabled");
                None
            }
        };

    // ── ClickHouse data warehouse (optional — activated by CLICKHOUSE_URL) ─
    let warehouse_tx: Option<crossbeam_channel::Sender<WarehouseEvent>> =
        match std::env::var("CLICKHOUSE_URL").ok().filter(|s| !s.is_empty()) {
            Some(ref url) => {
                let (tx, rx) = crossbeam_channel::bounded::<WarehouseEvent>(10_000);
                let logger = ClickHouseLogger::new(url);
                let act = activity.clone();
                let u = url.clone();
                tokio::spawn(async move {
                    match logger.ensure_schema().await {
                        Ok(()) => log_push(&act, EntryKind::Engine,
                            format!("ClickHouse warehouse connected: {u}")),
                        Err(e) => log_push(&act, EntryKind::Warn,
                            format!("ClickHouse warehouse schema error (non-fatal): {e}")),
                    }
                    logger.run(rx).await;
                });
                info!("ClickHouse data warehouse enabled");
                Some(tx)
            }
            None => {
                info!("CLICKHOUSE_URL not set — data warehouse disabled");
                None
            }
        };

    // ── Cloudflare R2 uploader (optional — activated by R2_ACCESS_KEY_ID) ───
    r2_uploader::start_r2_uploader();

    // ── Gas oracle (optional — activated by ETHERSCAN_API_KEY) ────────────
    let _gas_oracle = {
        let api_key = std::env::var("ETHERSCAN_API_KEY").ok();
        Arc::new(GasOracle::new(api_key))
    };

    // ── Channels ──────────────────────────────────────────────────────────
    let (signal_tx, signal_rx) = tokio::sync::mpsc::channel::<RN1Signal>(1024);

    // Alpha signal channel (AI sidecar → engine). Only allocated when enabled.
    let alpha_enabled = config.alpha_enabled;
    let (alpha_signal_tx, alpha_signal_rx) = if alpha_enabled {
        let (tx, rx) = tokio::sync::mpsc::channel::<engine::alpha_signal::AlphaSignal>(256);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let alpha_analytics = if alpha_enabled {
        Some(Arc::new(Mutex::new(engine::alpha_signal::AlphaAnalytics::default())))
    } else {
        None
    };
    let alpha_risk_config = if alpha_enabled {
        Some(engine::alpha_signal::AlphaRiskConfig::from_env())
    } else {
        None
    };
    if alpha_enabled {
        info!(sidecar_url = %config.alpha_sidecar_url, "Alpha pipeline enabled");
    }

    // ── Task: Ctrl-C sets shutdown flag ───────────────────────────────────
    {
        let sd = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            info!("Ctrl-C received");
            sd.store(true, Ordering::Relaxed);
        });
    }

    // ── Task: WebSocket ───────────────────────────────────────────────────
    let ws_task = {
        let cfg   = Arc::clone(&config);
        let books = Arc::clone(&book_store);
        let tx    = signal_tx.clone();
        let live  = Arc::clone(&ws_live);
        let act   = Some(activity.clone());
        let mc    = Arc::clone(&msg_count);
        let ttx   = tick_tx.clone();
        let subs  = Arc::clone(&market_subscriptions);
        let ws_reconnect = Arc::clone(&ws_force_reconnect);
        let ws_health = Arc::clone(&ws_health_metrics);
        tokio::spawn(async move {
            if let Err(e) = run_ws(cfg, books, tx, live, act, mc, ttx, subs, ws_reconnect, Some(ws_health)).await {
                tracing::error!(error = %e, "WebSocket task exited");
            }
        })
    };

    // ── Task: RN1 trade poller (REST-based detection) ────────────────────
    // Primary wallet from RN1_WALLET; additional wallets from TRACK_WALLETS=addr:weight,...
    let rn1_task = {
        // Build the list of wallets to track: primary first, then any extras.
        let primary_wallet = config.rn1_wallet.clone();
        let mut wallet_list: Vec<(String, f64)> = vec![(primary_wallet, 1.0)];
        if let Ok(extra) = std::env::var("TRACK_WALLETS") {
            for entry in extra.split(',') {
                let entry = entry.trim();
                if entry.is_empty() { continue; }
                let (addr, weight) = if let Some(pos) = entry.rfind(':') {
                    let w = entry[pos+1..].parse::<f64>().unwrap_or(0.8).clamp(0.0, 2.0);
                    (entry[..pos].to_string(), w)
                } else {
                    (entry.to_string(), 0.8)
                };
                if !addr.is_empty() { wallet_list.push((addr, weight)); }
            }
        }
        if wallet_list.len() > 1 {
            tracing::info!(
                n = wallet_list.len(),
                wallets = ?wallet_list.iter().map(|(w, _)| &w[..w.len().min(10)]).collect::<Vec<_>>(),
                "1A: Multi-wallet tracking enabled"
            );
        }
        // Spawn one poller task per wallet; share a single diagnostics handle for the primary.
        let _tasks: Vec<_> = wallet_list.into_iter().map(|(wallet, weight)| {
            let cfg  = Arc::clone(&config);
            let tx   = signal_tx.clone();
            let act  = Some(activity.clone());
            let diag = Arc::clone(&rn1_diagnostics);
            tokio::spawn(async move {
                run_rn1_poller(cfg, wallet, weight, tx, act, diag).await;
            })
        }).collect();
        // Return the first task handle for join tracking (primary wallet).
        _tasks.into_iter().next()
    };
    let rn1_task = rn1_task.unwrap_or_else(|| tokio::spawn(async {}));

    // ── Optional Agent JSON-RPC server (for orchestrator/agents) ─────────

    // ── Bullpen CLI bridge (BULLPEN_ENABLED=true to activate) ────────────
    let bullpen_config = engine::bullpen_bridge::BullpenConfig::from_env();
    let bullpen: Option<Arc<engine::bullpen_bridge::BullpenBridge>> = if bullpen_config.enabled {
        let bridge = Arc::new(engine::bullpen_bridge::BullpenBridge::new(bullpen_config));
        let bp = Arc::clone(&bridge);
        tokio::spawn(async move {
            match bp.health_check().await {
                Ok(()) => {},
                Err(e) => tracing::warn!("Bullpen CLI not available: {e} — enrichment disabled"),
            }
        });
        log_push(&activity, EntryKind::Engine, "Bullpen CLI bridge enabled".to_string());
        info!("Bullpen CLI bridge enabled");
        Some(bridge)
    } else {
        None
    };
    // ── Bullpen Discovery Scheduler (cold-path enrichment) ──────────────
    let discovery_store = Arc::new(tokio::sync::RwLock::new(
        engine::bullpen_discovery::DiscoveryStore::new(),
    ));
    if let Some(ref bp) = bullpen {
        let disc_config = engine::bullpen_discovery::DiscoverySchedulerConfig::from_env();
        if disc_config.enabled {
            let scheduler = engine::bullpen_discovery::DiscoveryScheduler::new(
                Arc::clone(bp),
                Arc::clone(&discovery_store),
                disc_config,
            );
            let disc_shutdown = Arc::clone(&shutdown);
            tokio::spawn(async move { scheduler.run(disc_shutdown).await });
            log_push(&activity, EntryKind::Engine, "Bullpen discovery scheduler started".to_string());
            info!("Bullpen discovery scheduler started");
        }
    }
    let _bullpen = bullpen; // Available for future phase wiring

    // ── Bullpen Smart Money Monitor ─────────────────────────────────────
    let convergence_store = {
        let sm_config = engine::bullpen_smart_money::SmartMoneyConfig::from_env();
        if sm_config.enabled {
            if let Some(ref bp) = _bullpen {
                let monitor = engine::bullpen_smart_money::SmartMoneyMonitor::new(
                    Arc::clone(bp),
                    sm_config,
                );
                let store = monitor.convergence_store();
                let sm_shutdown = Arc::clone(&shutdown);
                tokio::spawn(async move { monitor.run(sm_shutdown).await });
                log_push(&activity, EntryKind::Engine, "Bullpen smart money monitor started".to_string());
                info!("Bullpen smart money monitor started");
                Some(store)
            } else {
                None
            }
        } else {
            None
        }
    };

    let rpc_enabled = env_flag("AGENT_RPC_ENABLED");
    let rpc_bind_addr = std::env::var("AGENT_RPC_BIND")
        .unwrap_or_else(|_| "127.0.0.1:7878".to_string());

    // ── Tasks: paper/live engine + optional TUI ─────────────────────────
    let mut tui_thread: Option<std::thread::JoinHandle<()>> = None;
    let mut paper_for_persist: Option<Arc<PaperEngine>> = None;
    let mut live_for_web: Option<Arc<engine::live_engine::LiveEngine>> = None;
    let mut live_for_shutdown: Option<Arc<engine::live_engine::LiveEngine>> = None;
    
    let twin_enabled = env_flag("BLINK_TWIN");
    let twin_engine = if twin_enabled {
        Some(engine::blink_twin::BlinkTwin::new(Arc::clone(&book_store), Some(activity.clone())))
    } else {
        None
    };

    let signal_task: tokio::task::JoinHandle<()> = if paper_mode {
        let mut paper_inner = PaperEngine::new(
            Arc::clone(&book_store),
            Some(activity.clone()),
            Arc::clone(&market_subscriptions),
            Arc::clone(&ws_force_reconnect),
            warehouse_tx.clone(),
        );
        paper_inner.discovery_store = Some(Arc::clone(&discovery_store));
        paper_inner.convergence_store = convergence_store.clone();
        let paper = Arc::new(paper_inner);
        paper_for_persist = Some(Arc::clone(&paper));

        let paper_state_path = std::env::var("PAPER_STATE_PATH")
            .unwrap_or_else(|_| "logs\\paper_portfolio_state.json".to_string());
        let warm_state_path = std::env::var("PAPER_WARM_STATE_PATH")
            .unwrap_or_else(|_| "logs\\paper_warm_state.json".to_string());
        let rejection_state_path = std::env::var("PAPER_REJECTIONS_PATH")
            .unwrap_or_else(|_| "logs\\paper_rejections.json".to_string());
        let reset_paper_state = std::env::var("PAPER_RESET_STATE_ON_START")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        if !reset_paper_state {
            let _ = paper.load_portfolio_if_present(&paper_state_path).await;
        }
        let _ = paper.backfill_position_metadata().await;
        let _ = paper.load_rejections_if_present(&rejection_state_path).await;
        let _ = paper.load_warm_state_if_present(&warm_state_path, &market_subscriptions).await;
        ws_force_reconnect.store(true, Ordering::Relaxed);

        let rejection_trend_state: Arc<Mutex<HashMap<String, Vec<engine::paper_engine::RejectionTrendPoint>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let execution_summary_state = Arc::new(Mutex::new(engine::paper_engine::ExecutionSummary::default()));
        let experiment_state = Arc::new(Mutex::new((
            engine::paper_engine::ExperimentMetrics::default(),
            paper.experiment_switches(),
        )));

        {
            let p = Arc::clone(&paper);
            let rt = Arc::clone(&rejection_trend_state);
            let es = Arc::clone(&execution_summary_state);
            let ex = Arc::clone(&experiment_state);
            let sd = Arc::clone(&shutdown);
            tokio::spawn(async move {
                loop {
                    if sd.load(Ordering::Relaxed) {
                        break;
                    }
                    let trend = p.rejection_trend_24h().await;
                    let summary = p.execution_summary().await;
                    let metrics = p.experiment_metrics().await;
                    let switches = p.experiment_switches();
                    *rt.lock().unwrap() = trend;
                    *es.lock().unwrap() = summary;
                    *ex.lock().unwrap() = (metrics, switches);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
        }

        if tui_mode {
            let portfolio_for_tui = Arc::clone(&paper.portfolio);
            let books  = Arc::clone(&book_store);
            let act    = activity.clone();
            let live   = Arc::clone(&ws_live);
            let rs_h   = Arc::clone(&risk_status);
            let paused = Arc::clone(&trading_paused);
            let sd     = Arc::clone(&shutdown);
            let rn1_w  = rn1_wallet.clone();
            let mkts   = markets.clone();
            let mc     = Arc::clone(&msg_count);
            let lat    = Arc::clone(&latency);
            let ks     = kernel_snapshot.clone();
            let risk_for_tui = Arc::clone(&paper.risk);
            let fill_window_for_tui = Arc::clone(&paper.fill_window);
            let fill_latency_for_tui = Arc::clone(&paper.fill_latency);
            let subs_for_tui = Arc::clone(&market_subscriptions);
            let ws_reconnect_for_tui = Arc::clone(&ws_force_reconnect);
            let rejection_trend_for_tui = Arc::clone(&rejection_trend_state);
            let exec_summary_for_tui = Arc::clone(&execution_summary_state);
            let experiment_for_tui = Arc::clone(&experiment_state);
            let experiment_switches_for_tui = paper.experiment_switches_handle();
            let rn1_diag_for_tui = Arc::clone(&rn1_diagnostics);
            let twin_snapshot = Arc::new(tokio::sync::Mutex::new(TwinSnapshot::default()));
            let twin_health = paper.twin_health_handle();

            {
                let p_status = Arc::clone(&paper);
                let rs_update = Arc::clone(&risk_status);
                tokio::spawn(async move {
                    loop {
                        let status = p_status.risk_status();
                        *rs_update.lock().unwrap() = status;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                });
            }

            // ── Task: REST-based midpoint price updater for held positions ──
            // Fetches CLOB midpoints every ~10s so equity curve stays live
            // even when WS is down.
            {
                let clob_for_marks = Arc::clone(&clob);
                let portfolio_for_marks = Arc::clone(&paper.portfolio);
                let sd_marks = Arc::clone(&shutdown);
                tokio::spawn(async move {
                    loop {
                        if sd_marks.load(Ordering::Relaxed) {
                            break;
                        }
                        let token_ids: Vec<String> = {
                            let p = portfolio_for_marks.lock().await;
                            p.positions.iter().map(|pos| pos.token_id.clone()).collect()
                        };
                        for token_id in &token_ids {
                            match clob_for_marks.get_midpoint(token_id).await {
                                Ok(mid_str) => {
                                    if let Ok(price) = mid_str.parse::<f64>() {
                                        let mut p = portfolio_for_marks.lock().await;
                                        p.update_price(token_id, price);
                                    }
                                }
                                Err(_) => {} // Silently skip — WS/order-book still primary
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                });
            }

            if let Some(twin) = twin_engine.clone() {
                let twin_state = Arc::clone(&twin_snapshot);
                let twin_health_state = Arc::clone(&twin_health);
                tokio::spawn(async move {
                    loop {
                        let snap = twin.snapshot().await;
                        *twin_state.lock().await = snap.clone();
                        let total_attempts = (snap.filled_orders + snap.aborted_orders + snap.skipped_orders) as f64;
                        let abort_rate = if total_attempts > 0.0 {
                            snap.aborted_orders as f64 / total_attempts
                        } else {
                            0.0
                        };
                        let close_rate = if snap.filled_orders > 0 {
                            snap.closed_trades as f64 / snap.filled_orders as f64
                        } else {
                            0.0
                        };
                        *twin_health_state.lock().await = engine::paper_engine::TwinHealth {
                            abort_rate,
                            close_rate,
                            open_positions: snap.open_positions,
                        };
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                });
            }

            tui_thread = Some(std::thread::spawn(move || {
                if let Err(e) = run_tui(
                    portfolio_for_tui,
                    rs_h,
                    books,
                    act,
                    live,
                    paused,
                    rn1_w,
                    mkts,
                    sd,
                    mc,
                    lat,
                    ks,
                    risk_for_tui,
                    fill_window_for_tui,
                    fill_latency_for_tui,
                    subs_for_tui,
                    ws_reconnect_for_tui,
                    rejection_trend_for_tui,
                    exec_summary_for_tui,
                    experiment_for_tui,
                    experiment_switches_for_tui,
                    rn1_diag_for_tui,
                    twin_snapshot,
                    Some(Arc::clone(&ws_health_metrics)),
                ) {
                    eprintln!("TUI error: {e}");
                }
            }));
        } else if !web_ui_enabled {
            let pd = Arc::clone(&paper);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    pd.print_dashboard().await;
                }
            });
        }

        // ── Background autoclaim timer (every 5 s) ──────────────────────
        // Moved out of the hot signal path to avoid portfolio lock starvation.
        {
            let ac = Arc::clone(&paper);
            let sd_ac = Arc::clone(&shutdown);
            tokio::spawn(async move {
                loop {
                    if sd_ac.load(Ordering::Relaxed) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    ac.run_autoclaim().await;
                }
            });
        }

        // ── Mark price tick + equity curve (every 1 s) ───────────────────
        // Updates open position prices from the live order book store and
        // appends a NAV sample to the equity curve. This makes unrealized PnL
        // and the equity chart reflect real-time price moves in web mode where
        // the TUI (which normally drives push_equity_snapshot) is not running.
        {
            let pd = Arc::clone(&paper);
            let sd_mt = Arc::clone(&shutdown);
            tokio::spawn(async move {
                tracing::info!("tick_mark_prices task STARTED");
                let mut consecutive_ok: u64 = 0;
                loop {
                    if sd_mt.load(Ordering::Relaxed) {
                        tracing::info!("tick_mark_prices task exiting (shutdown)");
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let result = std::panic::AssertUnwindSafe(pd.tick_mark_prices())
                        .catch_unwind()
                        .await;
                    match result {
                        Ok(()) => {
                            consecutive_ok += 1;
                            if consecutive_ok == 1 || consecutive_ok % 30 == 0 {
                                tracing::info!(tick = consecutive_ok, "tick_mark_prices heartbeat");
                            }
                        }
                        Err(e) => {
                            let msg = e.downcast_ref::<String>()
                                .map(|s| s.as_str())
                                .or_else(|| e.downcast_ref::<&str>().copied())
                                .unwrap_or("unknown panic");
                            tracing::error!(err = msg, "tick_mark_prices PANICKED — recovering");
                        }
                    }
                }
            });
        }

        // ── Periodic autosave ────────────────────────────────────────────
        // Saves portfolio + warm state + rejections every PAPER_AUTOSAVE_SECS.
        // Critical: state is otherwise only saved on graceful shutdown, so
        // kills/crashes lose all session data.
        {
            let ps = Arc::clone(&paper);
            let sd_save = Arc::clone(&shutdown);
            let subs_save = Arc::clone(&market_subscriptions);
            let save_interval_secs = std::env::var("PAPER_AUTOSAVE_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(10);
            let psp = std::env::var("PAPER_STATE_PATH")
                .unwrap_or_else(|_| "logs\\paper_portfolio_state.json".to_string());
            let wsp = std::env::var("PAPER_WARM_STATE_PATH")
                .unwrap_or_else(|_| "logs\\paper_warm_state.json".to_string());
            let rsp = std::env::var("PAPER_REJECTIONS_PATH")
                .unwrap_or_else(|_| "logs\\paper_rejections.json".to_string());
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(save_interval_secs)).await;
                    if sd_save.load(Ordering::Relaxed) {
                        break;
                    }
                    match ps.save_portfolio(&psp).await {
                        Ok(()) => tracing::info!("autosave: portfolio saved"),
                        Err(e) => tracing::error!(err = %e, "autosave: save_portfolio FAILED"),
                    }
                    let subs = subs_save.lock().unwrap().clone();
                    let _ = ps.save_warm_state(&wsp, &subs, &psp).await;
                    let _ = ps.save_rejections(&rsp).await;
                }
            });
        }

        let tp = Arc::clone(&trading_paused);
        let twin_opt = twin_engine.clone();
        let subs_for_signals = Arc::clone(&market_subscriptions);
        let ws_reconnect_for_signals = Arc::clone(&ws_force_reconnect);
        let mut signal_rx = signal_rx;
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            while let Some(signal) = handle.block_on(signal_rx.recv()) {
                latency.lock().unwrap().record(signal.detected_at.elapsed());
                if tp.load(Ordering::Relaxed) { continue; }
                {
                    let mut subs = subs_for_signals.lock().unwrap();
                    if !subs.contains(&signal.token_id) {
                        subs.push(signal.token_id.clone());
                        ws_reconnect_for_signals.store(true, Ordering::Relaxed);
                    }
                }
                let p = Arc::clone(&paper);
                let t_opt = twin_opt.clone();
                let sig = signal.clone();
                if let Some(twin) = t_opt {
                    handle.block_on(async {
                        tokio::join!(p.handle_signal(sig.clone()), twin.handle_signal(sig));
                    });
                } else {
                    handle.block_on(p.handle_signal(sig));
                }
            }
        })
    } else if live_mode {
        let live = Arc::new(engine::live_engine::LiveEngine::new(
            Arc::clone(&config),
            Arc::clone(&book_store),
            Some(activity.clone()),
        ));
        live_for_web = Some(Arc::clone(&live));
        live_for_shutdown = Some(Arc::clone(&live));
        Arc::clone(&live).spawn_reconciliation_worker();

        // Spawn heartbeat — keeps the Polymarket session alive every 8s.
        {
            let hb_executor = live.executor.clone();
            let hb_risk = Arc::clone(&live.risk);
            let hb_metrics = engine::heartbeat::spawn_heartbeat_worker(hb_executor, None, Some(hb_risk));
            let l = Arc::clone(&live);
            tokio::spawn(async move {
                let mut t = tokio::time::interval(Duration::from_secs(30));
                t.tick().await;
                loop {
                    t.tick().await;
                    let ok  = hb_metrics.ok_count.load(std::sync::atomic::Ordering::Relaxed);
                    let err = hb_metrics.fail_count.load(std::sync::atomic::Ordering::Relaxed);
                    {
                        let mut m = l.failsafe_metrics.lock().unwrap();
                        m.heartbeat_ok_count   = ok;
                        m.heartbeat_fail_count = err;
                    }
                    if err > 0 {
                        tracing::warn!(heartbeat_ok = ok, heartbeat_fail = err, "Heartbeat failures detected");
                    }
                }
            });
        }
        {
            let hm = Arc::clone(&ws_health_metrics);
            let l = Arc::clone(&live);
            tokio::spawn(async move {
                let mut t = tokio::time::interval(Duration::from_secs(15));
                t.tick().await;
                loop {
                    t.tick().await;
                    let fs = l.failsafe_metrics_snapshot();
                    tracing::info!(
                        ws_ping_sent = hm.ping_sent.load(Ordering::Relaxed),
                        ws_pong_recv = hm.pong_recv.load(Ordering::Relaxed),
                        ws_reconnect_attempts = hm.reconnect_attempts.load(Ordering::Relaxed),
                        ws_last_pong_unix_ms = hm.last_pong_unix_ms.load(Ordering::Relaxed),
                        failsafe_checks = fs.check_count,
                        failsafe_triggers = fs.trigger_count,
                        failsafe_max_drift_bps = fs.max_observed_drift_bps,
                        confirmed_fills = fs.confirmed_fills,
                        no_fills = fs.no_fills,
                        stale_orders = fs.stale_orders,
                        confirmation_rate_pct = fs.confirmation_rate_pct,
                        heartbeat_ok = fs.heartbeat_ok_count,
                        heartbeat_fail = fs.heartbeat_fail_count,
                        "Live SLO heartbeat"
                    );
                }
            });
        }
        // ── Daily risk reset (UTC midnight) ───────────────────────────────
        {
            let risk_for_reset = Arc::clone(&live.risk);
            let sd = Arc::clone(&shutdown);
            tokio::spawn(async move {
                loop {
                    // Sleep until next UTC midnight.
                    let now = chrono::Utc::now();
                    let tomorrow = (now + chrono::Duration::days(1)).date_naive().and_hms_opt(0, 0, 0).unwrap();
                    let until_midnight = chrono::NaiveDateTime::signed_duration_since(tomorrow, now.naive_utc());
                    let secs = until_midnight.num_seconds().max(1) as u64;
                    tracing::info!(secs_until_reset = secs, "Daily risk reset scheduled");
                    tokio::time::sleep(Duration::from_secs(secs)).await;

                    if sd.load(Ordering::Relaxed) { break; }
                    risk_for_reset.lock().unwrap().reset_daily();
                    tracing::info!("🔄 Daily risk counters reset (UTC midnight)");
                }
            });
        }
        if tui_mode {
            let portfolio_for_tui = Arc::clone(&live.portfolio);
            let books  = Arc::clone(&book_store);
            let act    = activity.clone();
            let live_ws = Arc::clone(&ws_live);
            let rs_h   = Arc::clone(&risk_status);
            let paused = Arc::clone(&trading_paused);
            let sd     = Arc::clone(&shutdown);
            let rn1_w  = rn1_wallet.clone();
            let mkts   = markets.clone();
            let mc     = Arc::clone(&msg_count);
            let lat    = Arc::clone(&latency);
            let ks     = kernel_snapshot.clone();
            let risk_for_tui = Arc::clone(&live.risk);
            let fill_window_for_tui: Arc<Mutex<Option<engine::paper_engine::FillWindowSnapshot>>> =
                Arc::new(Mutex::new(None));
            let fill_latency_for_tui = Arc::new(Mutex::new(LatencyStats::new(1000)));
            let subs_for_tui = Arc::clone(&market_subscriptions);
            let ws_reconnect_for_tui = Arc::clone(&ws_force_reconnect);
            let rejection_trend_for_tui: Arc<Mutex<HashMap<String, Vec<engine::paper_engine::RejectionTrendPoint>>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let exec_summary_for_tui = Arc::new(Mutex::new(engine::paper_engine::ExecutionSummary::default()));
            let experiment_for_tui = Arc::new(Mutex::new((
                engine::paper_engine::ExperimentMetrics::default(),
                engine::paper_engine::ExperimentSwitches::default(),
            )));
            let experiment_switches_for_tui = Arc::new(Mutex::new(engine::paper_engine::ExperimentSwitches::default()));
            let rn1_diag_for_tui = Arc::clone(&rn1_diagnostics);
            let twin_snapshot = Arc::new(tokio::sync::Mutex::new(TwinSnapshot::default()));

            {
                let l = Arc::clone(&live);
                let rs_update = Arc::clone(&risk_status);
                let sd = Arc::clone(&shutdown);
                tokio::spawn(async move {
                    loop {
                        if sd.load(Ordering::Relaxed) {
                            break;
                        }
                        let status = l.risk_status();
                        *rs_update.lock().unwrap() = status;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                });
            }

            if let Some(twin) = twin_engine.clone() {
                let twin_state = Arc::clone(&twin_snapshot);
                tokio::spawn(async move {
                    loop {
                        let snap = twin.snapshot().await;
                        *twin_state.lock().await = snap;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                });
            }

            tui_thread = Some(std::thread::spawn(move || {
                if let Err(e) = run_tui(
                    portfolio_for_tui,
                    rs_h,
                    books,
                    act,
                    live_ws,
                    paused,
                    rn1_w,
                    mkts,
                    sd,
                    mc,
                    lat,
                    ks,
                    risk_for_tui,
                    fill_window_for_tui,
                    fill_latency_for_tui,
                    subs_for_tui,
                    ws_reconnect_for_tui,
                    rejection_trend_for_tui,
                    exec_summary_for_tui,
                    experiment_for_tui,
                    experiment_switches_for_tui,
                    rn1_diag_for_tui,
                    twin_snapshot,
                    Some(Arc::clone(&ws_health_metrics)),
                ) {
                    eprintln!("TUI error: {e}");
                }
            }));
        }

        let tp = Arc::clone(&trading_paused);
        let twin_opt = twin_engine.clone();
        let mut signal_rx = signal_rx;
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            while let Some(signal) = handle.block_on(signal_rx.recv()) {
                latency.lock().unwrap().record(signal.detected_at.elapsed());
                if tp.load(Ordering::Relaxed) { continue; }
                let l = Arc::clone(&live);
                let t_opt = twin_opt.clone();
                let sig = signal.clone();
                if let Some(twin) = t_opt {
                    handle.block_on(async {
                        tokio::join!(l.handle_signal(sig.clone()), twin.handle_signal(sig));
                    });
                } else {
                    handle.block_on(l.handle_signal(sig));
                }
            }
        })
    } else {
        // Read-only mode
        let mut signal_rx = signal_rx;
        tokio::spawn(async move {
            while let Some(signal) = signal_rx.recv().await {
                latency.lock().unwrap().record(signal.detected_at.elapsed());
                tracing::warn!(token_id = %signal.token_id, "RN1 signal — read-only");
            }
        })
    };

    // ── Alpha signal consumer (AI sidecar → PaperEngine) ────────────────
    if let Some(mut alpha_rx) = alpha_signal_rx {
        let alpha_paper = paper_for_persist.as_ref().map(Arc::clone);
        let alpha_act = activity.clone();
        let alpha_analytics_ref = alpha_analytics.clone();
        let alpha_handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            while let Some(signal) = alpha_handle.block_on(alpha_rx.recv()) {
                let source_label = format!("AI/{}", signal.analysis_id);
                if let Some(ref paper) = alpha_paper {
                    let rn1_compat = RN1Signal {
                        token_id: signal.token_id.clone(),
                        market_title: Some(format!("[ALPHA] {}", signal.analysis_id)),
                        market_outcome: None,
                        side: signal.side,
                        price: (signal.recommended_price * 1000.0) as u64,
                        size: (signal.recommended_size_usdc * 1000.0) as u64,
                        order_id: format!("alpha-{}", signal.analysis_id),
                        detected_at: signal.received_at.unwrap_or_else(std::time::Instant::now),
                        event_start_time: None,
                        event_end_time: None,
                        source_wallet: "alpha-sidecar".to_string(),
                        wallet_weight: 1.0,
                    };

                    alpha_handle.block_on(paper.handle_signal(rn1_compat));

                    if let Some(ref analytics) = alpha_analytics_ref {
                        if let Ok(mut a) = analytics.lock() {
                            a.positions_opened += 1;
                        }
                    }
                }
                log_push(&alpha_act, engine::activity_log::EntryKind::Signal,
                    format!("Alpha signal processed: {source_label}"));
            }
        });
        log_push(&activity, EntryKind::Engine, "Alpha signal consumer started".to_string());
        info!("Alpha signal consumer task spawned");
    }

    if rpc_enabled {
        let state = AgentRpcState {
            ws_live: Arc::clone(&ws_live),
            trading_paused: Arc::clone(&trading_paused),
            msg_count: Arc::clone(&msg_count),
            risk_status: Arc::clone(&risk_status),
            market_subscriptions: Arc::clone(&market_subscriptions),
            shutdown: Arc::clone(&shutdown),
            paper: paper_for_persist.as_ref().map(Arc::clone),
            bullpen: _bullpen.clone(),
            discovery_store: Some(Arc::clone(&discovery_store)),
            convergence_store: convergence_store.clone(),
            alpha_signal_tx: alpha_signal_tx.clone(),
            alpha_analytics: alpha_analytics.clone(),
            alpha_risk_config: alpha_risk_config.clone(),
        };
        let bind = rpc_bind_addr.clone();
        let act = activity.clone();
        tokio::spawn(async move {
            if let Err(e) = run_agent_rpc_server(&bind, state).await {
                log_push(&act, EntryKind::Warn, format!("Agent RPC server failed: {e}"));
                tracing::warn!(error = %e, bind_addr = %bind, "Agent RPC server failed");
            }
        });
        log_push(&activity, EntryKind::Engine, format!("Agent RPC enabled on {rpc_bind_addr}"));
        info!(bind_addr = %rpc_bind_addr, "Agent RPC server enabled");
    } else {
        info!("AGENT_RPC_ENABLED not set — agent RPC server disabled");
    }

    // ── Optional Web UI server (auto-enabled for paper/live mode) ────────
    if web_ui_enabled {
        let web_ui_port = std::env::var("WEB_UI_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(3030);
        let web_bind = format!("0.0.0.0:{web_ui_port}");
        let (broadcast_tx, _) = tokio::sync::broadcast::channel::<String>(64);

        let risk_handle = paper_for_persist.as_ref().map(|p| Arc::clone(&p.risk));

        let web_state = AppState {
            ws_live: Arc::clone(&ws_live),
            trading_paused: Arc::clone(&trading_paused),
            msg_count: Arc::clone(&msg_count),
            book_store: Arc::clone(&book_store),
            activity_log: activity.clone(),
            paper: paper_for_persist.as_ref().map(Arc::clone),
            risk: risk_handle,
            twin_snapshot: None,
            ws_health: None,
            latency: None,
            market_subscriptions: Arc::clone(&market_subscriptions),
            broadcast_tx,
            started_at: Arc::new(std::time::Instant::now()),
            provider: engine::execution_provider::create_provider_from_env(),
            live_engine: live_for_web.as_ref().map(Arc::clone),
            bullpen: _bullpen.clone(),
            discovery_store: Some(Arc::clone(&discovery_store)),
            convergence_store: convergence_store.clone(),
            slug_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
            portfolio_cache: Arc::new(std::sync::RwLock::new(None)),
            clickhouse_url: std::env::var("CLICKHOUSE_URL").ok().filter(|s| !s.is_empty()),
            snapshot_seq: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            portfolio_cached_at_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };

        let static_dir = std::env::var("WEB_UI_STATIC_DIR").ok()
            .or_else(|| {
                let candidate = "static/ui".to_string();
                if std::path::Path::new(&candidate).exists() { Some(candidate) } else { None }
            })
            .or_else(|| {
                let candidate = "web-ui/dist".to_string();
                if std::path::Path::new(&candidate).exists() { Some(candidate) } else { None }
            });

        let bind = web_bind.clone();
        let broadcast_secs = config.ws_broadcast_interval_secs;
        tokio::spawn(async move {
            run_web_server(&bind, web_state, static_dir, broadcast_secs).await;
        });
        log_push(&activity, EntryKind::Engine, format!("Web UI enabled on {web_bind}"));
        info!(bind_addr = %web_bind, "Web UI server enabled");
    }

    // ── Wait for shutdown (Ctrl-C or TUI q) ──────────────────────────────
    loop {
        if shutdown.load(Ordering::Relaxed) { break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    info!("Initiating graceful shutdown");
    ws_task.abort();
    rn1_task.abort();
    drop(signal_tx);
    signal_task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), signal_task).await;

    // ── Live mode: graceful shutdown with reconciliation + state persist ──
    if let Some(live) = live_for_shutdown.take() {
        info!("Running live engine graceful shutdown (reconcile + cancel + persist)…");
        let shutdown_timeout = tokio::time::timeout(
            Duration::from_secs(30),
            live.graceful_shutdown(),
        ).await;
        if shutdown_timeout.is_err() {
            tracing::warn!("Live engine graceful shutdown timed out after 30s");
        }
    }

    if paper_mode {
        let paper_state_path = std::env::var("PAPER_STATE_PATH")
            .unwrap_or_else(|_| "logs\\paper_portfolio_state.json".to_string());
        if let Some(paper) = paper_for_persist.as_ref() {
            if let Err(e) = paper.save_portfolio(&paper_state_path).await {
                log_push(&activity, EntryKind::Warn, format!("Failed to save paper state: {e}"));
                tracing::warn!(error = %e, path = %paper_state_path, "Failed to save paper portfolio state");
            } else {
                log_push(&activity, EntryKind::Engine, format!("Saved paper state to {paper_state_path}"));
                info!(path = %paper_state_path, "Saved paper portfolio state");
            }
            let warm_state_path = std::env::var("PAPER_WARM_STATE_PATH")
                .unwrap_or_else(|_| "logs\\paper_warm_state.json".to_string());
            let rejection_state_path = std::env::var("PAPER_REJECTIONS_PATH")
                .unwrap_or_else(|_| "logs\\paper_rejections.json".to_string());
            let subs = market_subscriptions.lock().unwrap().clone();
            let _ = paper.save_warm_state(&warm_state_path, &subs, &paper_state_path).await;
            let _ = paper.save_rejections(&rejection_state_path).await;
        }
    }
    if let Some(h) = tui_thread { let _ = h.join(); }

    if env_flag_default_true("AUTO_POSTRUN_REVIEW") {
        match write_postrun_review(&session_log_path) {
            Ok(path) => {
                log_push(&activity, EntryKind::Engine, format!("Post-run review saved: {path}"));
                info!(path = %path, "Post-run review saved");
            }
            Err(e) => {
                log_push(&activity, EntryKind::Warn, format!("Post-run review failed: {e}"));
                tracing::warn!(error = %e, "Post-run review failed");
            }
        }
    }

    info!("BLINK ENGINE shutdown complete");
    Ok(())
}

/// Run a historical backtest from a CSV file and exit.
fn run_backtest(csv_path: &str, output_path: Option<&str>) -> Result<()> {
    println!("\n╔══════════════════════════════════════════════════════╗");
    println!(  "║         BLINK ENGINE — Backtest Mode                 ║");
    println!(  "╚══════════════════════════════════════════════════════╝\n");

    let rn1_wallet = std::env::var("RN1_WALLET")
        .unwrap_or_else(|_| "".to_string())
        .to_lowercase();

    let config = BacktestConfig {
        rn1_wallet,
        starting_usdc: std::env::var("BACKTEST_STARTING_USDC")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(100.0),
        size_multiplier: std::env::var("BACKTEST_SIZE_MULTIPLIER")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(0.02),
        drift_threshold: std::env::var("BACKTEST_DRIFT_THRESHOLD")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(0.015),
        fill_window_ms: std::env::var("BACKTEST_FILL_WINDOW_MS")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(3000),
        slippage_bps: std::env::var("BACKTEST_SLIPPAGE_BPS")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(10),
    };

    println!("Loading ticks from: {csv_path}");
    let ticks = load_ticks_csv(csv_path)?;
    println!("Loaded {} ticks", ticks.len());

    let mut engine = BacktestEngine::new(config, ticks);
    let results = engine.run();

    println!("\n─── Backtest Results ───────────────────────────────────");
    println!("  Total Return:     {:.2}%", results.total_return_pct);
    println!("  Sharpe Ratio:     {:.4}", results.sharpe_ratio);
    println!("  Sortino Ratio:    {:.4}", results.sortino_ratio);
    println!("  Max Drawdown:     {:.2}%", results.max_drawdown_pct);
    println!("  Calmar Ratio:     {:.4}", results.calmar_ratio);
    println!("  Win Rate:         {:.1}%", results.win_rate * 100.0);
    println!("  Profit Factor:    {:.4}", results.profit_factor);
    println!("  Avg Duration:     {} ms", results.avg_trade_duration_ms);
    println!("  Total Trades:     {}", results.total_trades);
    println!("  Equity Points:    {}", results.equity_curve.len());
    println!("───────────────────────────────────────────────────────\n");

    if let Some(path) = output_path {
        let json = serde_json::to_string_pretty(&results)?;
        std::fs::write(path, &json)?;
        println!("Results written to: {path}");
    }

    Ok(())
}

#[inline]
fn env_flag(key: &str) -> bool {
    std::env::var(key)
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

#[inline]
fn env_flag_default_true(key: &str) -> bool {
    std::env::var(key)
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(true)
}

async fn run_preflight_live(config: &Config) -> Result<()> {
    anyhow::ensure!(config.live_trading, "--preflight-live requires LIVE_TRADING=true");
    config.validate_live_profile_contract()?;

    // ── Check 1: market data reachable ───────────────────────────────────
    let clob = ClobClient::new(&config.clob_host);
    let token = &config.markets[0];
    let buy_price = clob
        .get_price(token, engine::types::OrderSide::Buy)
        .await
        .map_err(|e| anyhow::anyhow!("preflight failed: get_price BUY for token {token}: {e}"))?;
    let sell_price = clob
        .get_price(token, engine::types::OrderSide::Sell)
        .await
        .map_err(|e| anyhow::anyhow!("preflight failed: get_price SELL for token {token}: {e}"))?;
    let mid = clob
        .get_midpoint(token)
        .await
        .map_err(|e| anyhow::anyhow!("preflight failed: get_midpoint for token {token}: {e}"))?;
    let _ = buy_price
        .parse::<f64>()
        .map_err(|e| anyhow::anyhow!("preflight failed: BUY price parse error for token {token}: {e}"))?;
    let _ = sell_price
        .parse::<f64>()
        .map_err(|e| anyhow::anyhow!("preflight failed: SELL price parse error for token {token}: {e}"))?;
    let _ = mid
        .parse::<f64>()
        .map_err(|e| anyhow::anyhow!("preflight failed: midpoint parse error for token {token}: {e}"))?;

    println!(
        "✅ preflight-live [1/4] market data: token={} buy={} sell={} mid={}",
        token, buy_price, sell_price, mid,
    );

    // ── Check 2: auth credentials valid ──────────────────────────────────
    let executor = engine::order_executor::OrderExecutor::from_config(config);
    executor
        .validate_credentials()
        .await
        .map_err(|e| anyhow::anyhow!("preflight failed: auth check: {e}"))?;
    println!("✅ preflight-live [2/4] auth credentials valid (GET /auth/ok)");

    // ── Check 3: signature fields present ────────────────────────────────
    println!(
        "✅ preflight-live [3/4] order config: signature_type={} nonce={} expiration={}",
        config.polymarket_signature_type,
        config.polymarket_order_nonce,
        config.polymarket_order_expiration,
    );

    // ── Check 4: risk config non-zero ─────────────────────────────────────
    let risk_cfg = engine::risk_manager::RiskConfig::from_env();
    anyhow::ensure!(
        risk_cfg.max_single_order_usdc > 0.0,
        "preflight failed: MAX_SINGLE_ORDER_USDC must be > 0"
    );
    println!(
        "✅ preflight-live [4/4] risk limits: max_single_order_usdc={} max_daily_loss_pct={}",
        risk_cfg.max_single_order_usdc, risk_cfg.max_daily_loss_pct,
    );

    println!("\n🟢  ALL PREFLIGHT CHECKS PASSED — safe to go live");
    Ok(())
}

/// Operator-initiated emergency stop: cancels all open exchange orders and
/// writes logs/EMERGENCY_STOP.flag.
async fn run_emergency_stop(config: &Config) -> Result<()> {
    eprintln!("🚨 --emergency-stop requested by operator");
    let executor = engine::order_executor::OrderExecutor::from_config(config);
    match executor.cancel_all_orders().await {
        Ok(())  => eprintln!("✅ cancel_all_orders succeeded"),
        Err(e)  => eprintln!("⚠️  cancel_all_orders error (may be no open orders): {e}"),
    }
    std::fs::create_dir_all("logs")?;
    std::fs::write(
        "logs\\EMERGENCY_STOP.flag",
        format!("reason=operator_cli\ntimestamp={}\n", chrono::Utc::now()),
    )?;
    eprintln!("📄 Wrote logs/EMERGENCY_STOP.flag");
    Ok(())
}

#[derive(Debug, Default)]
struct RunReview {
    total_lines: usize,
    info_lines: usize,
    warn_lines: usize,
    error_lines: usize,
    signals: usize,
    fills: usize,
    aborts: usize,
    skips: usize,
    risk_blocked: usize,
    liquidity_downsized: usize,
    ws_handshake_ok: usize,
    ws_subscribed: usize,
    ws_closed_cleanly: usize,
    ws_reconnect_requested: usize,
    ws_reconnect_suppressed: usize,
    ws_parse_errors: usize,
    ws_parser_summary_lines: usize,
    ws_parser_parsed_total: usize,
    ws_parser_unknown_total: usize,
    ws_parser_failed_total: usize,
    reconnect_hints: usize,
    rn1_poll_cycles: usize,
    rn1_poller_metrics_lines: usize,
    signal_channel_full: usize,
    twin_mentions: usize,
    twin_fill_hints: usize,
    twin_close_hints: usize,
    nav_points: usize,
    nav_step_abs_sum: f64,
    nav_first: Option<f64>,
    nav_last: Option<f64>,
    nav_min: Option<f64>,
    nav_max: Option<f64>,
    max_gap_secs: i64,
    first_ts: Option<DateTime<Utc>>,
    last_ts: Option<DateTime<Utc>>,
}

fn parse_nav(line: &str) -> Option<f64> {
    for marker in ["NAV=$", "nav=$"] {
        if let Some(idx) = line.find(marker) {
            let tail = &line[idx + marker.len()..];
            let mut num = String::new();
            for ch in tail.chars() {
                if ch.is_ascii_digit() || ch == '.' {
                    num.push(ch);
                } else {
                    break;
                }
            }
            if let Ok(v) = num.parse::<f64>() {
                return Some(v);
            }
        }
    }
    None
}

fn parse_ts_utc(line: &str) -> Option<DateTime<Utc>> {
    let token = line.split_whitespace().next()?;
    DateTime::parse_from_rfc3339(token).ok().map(|d| d.with_timezone(&Utc))
}

fn analyze_session_log(path: &str) -> std::io::Result<RunReview> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut r = RunReview::default();
    let mut prev_ts: Option<DateTime<Utc>> = None;
    let mut prev_nav: Option<f64> = None;

    for line in reader.lines() {
        let line = line?;
        r.total_lines += 1;
        let l = line.to_lowercase();

        let ts = parse_ts_utc(&line);
        if r.first_ts.is_none() {
            r.first_ts = ts;
        }
        if let (Some(a), Some(b)) = (prev_ts, ts) {
            let gap = (b - a).num_seconds().max(0);
            if gap > r.max_gap_secs {
                r.max_gap_secs = gap;
            }
        }
        prev_ts = ts;
        r.last_ts = ts.or(r.last_ts);

        if line.contains(" INFO ") { r.info_lines += 1; }
        if line.contains(" WARN ") { r.warn_lines += 1; }
        if line.contains(" ERROR ") { r.error_lines += 1; }
        if l.contains("rn1 signal received") { r.signals += 1; }
        if l.contains("paper order filled") { r.fills += 1; }
        if l.contains("aborted") || l.contains("abort") { r.aborts += 1; }
        if l.contains("skipped") { r.skips += 1; }
        if l.contains("risk blocked") { r.risk_blocked += 1; }
        if l.contains("liquidity guard downsized") { r.liquidity_downsized += 1; }
        if l.contains("websocket handshake complete") { r.ws_handshake_ok += 1; }
        if l.contains("subscribed to markets") { r.ws_subscribed += 1; }
        if l.contains("websocket closed cleanly") { r.ws_closed_cleanly += 1; }
        if l.contains("reconnect requested for updated market subscriptions") { r.ws_reconnect_requested += 1; }
        if l.contains("reconnect request suppressed by debounce") { r.ws_reconnect_suppressed += 1; }
        if l.contains("parse") && l.contains("ws") { r.ws_parse_errors += 1; }
        if l.contains("ws parser session summary") {
            r.ws_parser_summary_lines += 1;
            if let Some(i) = l.find("parsed=") {
                let num: String = l[i + 7..].chars().take_while(|c| c.is_ascii_digit()).collect();
                r.ws_parser_parsed_total += num.parse::<usize>().unwrap_or(0);
            }
            if let Some(i) = l.find("unknown=") {
                let num: String = l[i + 8..].chars().take_while(|c| c.is_ascii_digit()).collect();
                r.ws_parser_unknown_total += num.parse::<usize>().unwrap_or(0);
            }
            if let Some(i) = l.find("parse_failed=") {
                let num: String = l[i + 13..].chars().take_while(|c| c.is_ascii_digit()).collect();
                r.ws_parser_failed_total += num.parse::<usize>().unwrap_or(0);
            }
        }
        if l.contains("reconnect") { r.reconnect_hints += 1; }
        if l.contains("engine::rn1_poller: poll cycle") { r.rn1_poll_cycles += 1; }
        if l.contains("rn1 poller metrics") { r.rn1_poller_metrics_lines += 1; }
        if l.contains("signal channel full") { r.signal_channel_full += 1; }
        if l.contains("twin") {
            r.twin_mentions += 1;
            if l.contains("fill") { r.twin_fill_hints += 1; }
            if l.contains("close") || l.contains("autoclaim") || l.contains("tp") || l.contains("sl") {
                r.twin_close_hints += 1;
            }
        }

        if let Some(nav) = parse_nav(&line) {
            r.nav_points += 1;
            if r.nav_first.is_none() { r.nav_first = Some(nav); }
            if let Some(prev) = prev_nav {
                r.nav_step_abs_sum += (nav - prev).abs();
            }
            prev_nav = Some(nav);
            r.nav_last = Some(nav);
            r.nav_min = Some(r.nav_min.map(|v| v.min(nav)).unwrap_or(nav));
            r.nav_max = Some(r.nav_max.map(|v| v.max(nav)).unwrap_or(nav));
        }
    }

    Ok(r)
}

fn write_postrun_review(session_log_path: &str) -> Result<String> {
    let review = analyze_session_log(session_log_path)?;
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let dir = std::env::var("POSTRUN_REVIEW_DIR").unwrap_or_else(|_| "logs\\reports".to_string());
    std::fs::create_dir_all(&dir)?;
    let out_path = format!("{dir}\\postrun-review-{ts}.txt");

    let duration_min = match (review.first_ts, review.last_ts) {
        (Some(a), Some(b)) => (b - a).num_seconds().max(0) as f64 / 60.0,
        _ => 0.0,
    };
    let attempts = review.fills + review.aborts + review.skips;
    let fill_rate = if attempts > 0 { (review.fills as f64 / attempts as f64) * 100.0 } else { 0.0 };
    let abort_rate = if attempts > 0 { (review.aborts as f64 / attempts as f64) * 100.0 } else { 0.0 };
    let skip_rate = if attempts > 0 { (review.skips as f64 / attempts as f64) * 100.0 } else { 0.0 };

    let (ret_pct, nav_swing_pct) = match (review.nav_first, review.nav_last, review.nav_min, review.nav_max) {
        (Some(s), Some(e), Some(nmin), Some(nmax)) if s > 0.0 => (
            ((e - s) / s) * 100.0,
            ((nmax - nmin) / s) * 100.0,
        ),
        _ => (0.0, 0.0),
    };

    let realism_alert = if duration_min > 0.0 && ret_pct.abs() / duration_min > 0.40 {
        "HIGH"
    } else if nav_swing_pct > 15.0 {
        "MEDIUM"
    } else {
        "LOW"
    };
    let parser_unknown_rate = if review.ws_parser_parsed_total > 0 {
        (review.ws_parser_unknown_total as f64 / review.ws_parser_parsed_total as f64) * 100.0
    } else {
        0.0
    };
    let parser_fail_rate = if review.ws_parser_parsed_total > 0 {
        (review.ws_parser_failed_total as f64 / review.ws_parser_parsed_total as f64) * 100.0
    } else {
        0.0
    };
    let nav_jitter_pct = match review.nav_first {
        Some(s) if s > 0.0 => (review.nav_step_abs_sum / s) * 100.0,
        _ => 0.0,
    };
    let session_size_bytes = std::fs::metadata(session_log_path).map(|m| m.len()).unwrap_or(0);
    let paper_state = "logs\\paper_portfolio_state.json";
    let warm_state = "logs\\paper_warm_state.json";
    let rej_state = "logs\\paper_rejections.json";

    let mut assumptions: Vec<String> = Vec::new();
    if review.max_gap_secs >= 3 {
        assumptions.push(format!(
            "Detected max log gap of {}s; perceived UI freeze likely due to event drought or reconnect wait, not necessarily process hang.",
            review.max_gap_secs
        ));
    }
    if review.ws_reconnect_requested > 0 && review.ws_reconnect_suppressed > 0 {
        assumptions.push("Frequent subscription updates likely triggered reconnect pressure, but debounce suppressed churn.".to_string());
    }
    if review.signals > 0 && attempts == 0 {
        assumptions.push("Signals were observed without execution attempts; likely paused state, risk gate, or sizing floor rejection path.".to_string());
    }
    if abort_rate > 50.0 {
        assumptions.push("High abort rate indicates fill-window strictness too aggressive for current market microstructure.".to_string());
    }
    if parser_fail_rate > 0.5 || parser_unknown_rate > 2.0 {
        assumptions.push("Parser quality degraded; message schema drift or unsupported event variants likely affected coverage.".to_string());
    }
    if review.twin_mentions > 0 && review.twin_fill_hints == 0 {
        assumptions.push("Twin active but low fill evidence; Twin penalties may be too strict for current liquidity.".to_string());
    }
    if assumptions.is_empty() {
        assumptions.push("No strong anomaly signatures detected; behavior appears within expected guardrails for the sampled run.".to_string());
    }

    let mut file = std::fs::File::create(&out_path)?;
    writeln!(file, "BLINK POST-RUN EVALUATION TEMPLATE v2-DEEP")?;
    writeln!(file, "session_log={session_log_path}")?;
    writeln!(file, "generated_utc={}", Utc::now().to_rfc3339())?;
    writeln!(file)?;
    writeln!(file, "[1] EXECUTIVE SUMMARY")?;
    writeln!(file, "duration_min={duration_min:.2}")?;
    writeln!(file, "signals={} attempts={} fills={} aborts={} skips={}", review.signals, attempts, review.fills, review.aborts, review.skips)?;
    writeln!(file, "fill_rate_pct={fill_rate:.2} abort_rate_pct={abort_rate:.2} skip_rate_pct={skip_rate:.2}")?;
    writeln!(file, "nav_return_pct={ret_pct:.2} nav_swing_pct={nav_swing_pct:.2}")?;
    writeln!(file, "nav_points={} nav_jitter_pct={nav_jitter_pct:.2}", review.nav_points)?;
    writeln!(file, "log_lines={} info={} warn={} error={}", review.total_lines, review.info_lines, review.warn_lines, review.error_lines)?;
    writeln!(file)?;
    writeln!(file, "[2] DATA SOURCES COVERAGE")?;
    writeln!(file, "session_log_bytes={session_size_bytes}")?;
    writeln!(file, "paper_state_exists={} warm_state_exists={} rejection_state_exists={}",
        std::path::Path::new(paper_state).exists(),
        std::path::Path::new(warm_state).exists(),
        std::path::Path::new(rej_state).exists()
    )?;
    writeln!(file)?;
    writeln!(file, "[3] CONNECTIVITY & INGEST QUALITY")?;
    writeln!(file, "ws_handshake_ok={} ws_subscribed={} ws_closed_cleanly={}", review.ws_handshake_ok, review.ws_subscribed, review.ws_closed_cleanly)?;
    writeln!(file, "ws_reconnect_requested={} ws_reconnect_suppressed={} reconnect_hints={}", review.ws_reconnect_requested, review.ws_reconnect_suppressed, review.reconnect_hints)?;
    writeln!(file, "ws_parser_summary_lines={} parser_parsed_total={} parser_unknown_total={} parser_failed_total={}",
        review.ws_parser_summary_lines, review.ws_parser_parsed_total, review.ws_parser_unknown_total, review.ws_parser_failed_total)?;
    writeln!(file, "parser_unknown_rate_pct={parser_unknown_rate:.3} parser_failed_rate_pct={parser_fail_rate:.3}")?;
    writeln!(file, "ws_parse_error_hints={} signal_channel_full={}", review.ws_parse_errors, review.signal_channel_full)?;
    writeln!(file, "assessment={}", if review.ws_parse_errors > 0 || parser_fail_rate > 0.1 { "degraded" } else { "stable" })?;
    writeln!(file)?;
    writeln!(file, "[4] SIGNAL PIPELINE DIAGNOSTIC")?;
    writeln!(file, "rn1_poll_cycles={} rn1_poller_metrics_lines={} signals_detected={}", review.rn1_poll_cycles, review.rn1_poller_metrics_lines, review.signals)?;
    writeln!(file, "assessment={}", if review.rn1_poll_cycles == 0 { "poller_inactive_or_unlogged" } else { "poller_active" })?;
    writeln!(file)?;
    writeln!(file, "[5] EXECUTION QUALITY")?;
    writeln!(file, "risk_blocked={} liquidity_downsized={}", review.risk_blocked, review.liquidity_downsized)?;
    writeln!(file, "assessment={}", if fill_rate < 20.0 { "low_fill_efficiency" } else { "acceptable" })?;
    writeln!(file)?;
    writeln!(file, "[6] BLINK TWIN DIAGNOSTIC")?;
    writeln!(file, "twin_mentions={} twin_fill_hints={} twin_close_hints={}", review.twin_mentions, review.twin_fill_hints, review.twin_close_hints)?;
    writeln!(file, "assessment={}", if review.twin_fill_hints > 0 && review.twin_close_hints == 0 { "twin_not_rotating" } else { "ok_or_no_data" })?;
    writeln!(file)?;
    writeln!(file, "[7] REALISM GAP DIAGNOSTIC")?;
    writeln!(file, "realism_alert={realism_alert}")?;
    writeln!(file, "rule_return_per_min_pct={:.3}", if duration_min > 0.0 { ret_pct.abs() / duration_min } else { 0.0 })?;
    writeln!(file, "rule_nav_swing_pct={nav_swing_pct:.2}")?;
    writeln!(file, "max_log_gap_secs={}", review.max_gap_secs)?;
    writeln!(file)?;
    writeln!(file, "[8] INFERENCES & CONCLUSIONS")?;
    for (i, a) in assumptions.iter().enumerate() {
        writeln!(file, "{}. {}", i + 1, a)?;
    }
    writeln!(file)?;
    writeln!(file, "[9] ACTIONABLE NEXT TUNING")?;
    writeln!(file, "- Keep PAPER_REALISM_MODE=true")?;
    writeln!(file, "- If realism_alert=HIGH: increase PAPER_FILL_WINDOW_MS and PAPER_ADVERSE_FILL_BPS")?;
    writeln!(file, "- If abort_rate_pct>60: reduce PAPER_SIZE_MULTIPLIER and raise PAPER_DEPTH_CAPTURE_RATIO cautiously")?;
    writeln!(file, "- If twin_not_rotating: tighten TWIN_AUTOCLAIM_TIERS and verify live market price coverage")?;
    writeln!(file, "- If max_log_gap_secs>=3 and ws_reconnect_requested high: increase WS_RECONNECT_DEBOUNCE_MS")?;
    writeln!(file, "- If parser_unknown_rate_pct rises run-over-run: inspect new WS event variants and update parser")?;
    writeln!(file)?;
    writeln!(file, "[10] MACHINE SUMMARY")?;
    writeln!(file, "summary.fill_rate_pct={fill_rate:.2}")?;
    writeln!(file, "summary.abort_rate_pct={abort_rate:.2}")?;
    writeln!(file, "summary.nav_return_pct={ret_pct:.2}")?;
    writeln!(file, "summary.reconnect_hints={}", review.reconnect_hints)?;
    writeln!(file, "summary.max_log_gap_secs={}", review.max_gap_secs)?;
    writeln!(file, "summary.realism_alert={realism_alert}")?;

    std::fs::write("logs\\LATEST_POSTRUN_REVIEW.txt", format!("{out_path}\n"))?;
    Ok(out_path)
}
