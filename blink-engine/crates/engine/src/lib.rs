//! # Blink Engine
//!
//! A high-frequency, low-latency trading engine for [Polymarket](https://polymarket.com)
//! that detects and mirrors orders from a tracked "whale" wallet (RN1) on the CLOB.
//!
//! ## Crate layout
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`types`] | Core types: `OrderSide`, `TimeInForce`, `MarketEvent`, `RN1Signal`, price helpers |
//! | [`config`] | Runtime config loaded from environment variables |
//! | [`ws_client`] | Persistent WebSocket client with exponential-backoff reconnection |
//! | [`order_book`] | In-memory multi-market CLOB order book (`BTreeMap` + `DashMap`) |
//! | [`sniffer`] | RN1 wallet filter — emits [`types::RN1Signal`] on matching events |
//! | [`paper_engine`] | Paper trading engine — full pipeline simulation, no network orders |
//! | [`live_engine`] | Live trading engine — EIP-712 signing + CLOB REST submission |
//! | [`paper_portfolio`] | Virtual $100 USDC portfolio state, sizing, and P&L tracking |
//! | [`order_signer`] | EIP-712 order signing (k256 / secp256k1, manual Keccak256) |
//! | [`order_executor`] | CLOB REST API client with transient-error retry and backoff |
//! | [`clob_client`] | Read-only CLOB REST client (prices, order books, markets) |
//! | [`risk_manager`] | Pre-order risk checks: kill switch, circuit breaker, rate limit |
//! | [`activity_log`] | Thread-safe ring buffer of engine events for TUI display |
//! | [`latency_tracker`] | Rolling-window latency stats (min/max/avg/p99 in µs) |
//! | [`tick_recorder`] | ClickHouse batch writer for tick-level order events (activated via `CLICKHOUSE_URL`) |
//! | [`clickhouse_logger`] | Extended ClickHouse data warehouse — order book snapshots, RN1 signals, trade executions, system metrics |
//! | [`quant_data`] | Read-only adapters from warehouse tables into offline replay events |
//! | [`quant_report`] | Offline replay reporting for $100 signal-taker simulations |
//! | [`quant_replay`] | Offline deterministic replay, microstructure feature extraction, and fill simulation |
//! | [`gas_oracle`] | Moving-average gas price oracle for Polygon transactions (activated via `ETHERSCAN_API_KEY`) |
//! | [`tui_app`] | ratatui terminal dashboard (activated via `TUI=true`) |
//! | [`game_start_watcher`] | Polls CLOB prices to detect in-play market transitions |
//!
//! ## Price scaling convention
//!
//! All prices and sizes are stored as [`u64`] scaled by **1,000** to eliminate
//! floating-point arithmetic in the hot path:
//!
//! ```
//! use engine::types::parse_price;
//! assert_eq!(parse_price("0.65"),  650);
//! assert_eq!(parse_price("50000"), 50_000_000);
//! ```
//!
//! Use [`types::format_price`] to convert back for display.

pub mod activity_log;
pub mod agent_rpc;
pub mod alpha_signal;
pub mod backtest_engine;
pub mod blink_twin;
pub mod buffer_pool;
pub mod clob_client;
pub mod command_listener;
pub mod config;
pub mod drift_overrides;
pub mod execution_profile;
pub mod execution_provider;
pub mod exit_strategy;
pub mod gas_oracle;
pub mod geo_guard;
pub mod heartbeat;
pub mod hot_metrics;
pub mod ingress_dedup;
pub mod latency_tracker;
pub mod live_engine;
pub mod maker_layering;
pub mod market_class;
pub mod market_metadata;
pub mod mev_router;
pub mod operator_alerts;
pub mod order_book;
pub mod order_executor;
pub mod order_router;
pub mod order_signer;
pub mod paper_engine;
pub mod paper_portfolio;
pub mod postgres_logger;
pub mod pretrade_gate;
pub mod quant_data;
pub mod quant_replay;
pub mod quant_report;
pub mod quant_strategy;
pub mod r2_uploader;
pub mod risk_manager;
pub mod rn1_poller;
pub mod signal_pipeline;
pub mod sniffer;
pub mod strategy;
pub mod tick_recorder;
pub mod timed_mutex;
pub mod truth_reconciler;
pub mod tui_app;
pub mod types;
pub mod web_server;
pub mod ws_client;
