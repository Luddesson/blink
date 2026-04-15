//! Alpha signal types for AI-driven autonomous trading.
//!
//! An [`AlphaSignal`] is produced by the Python sidecar (LLM analysis pipeline)
//! and submitted via the agent RPC server. It flows through the same risk
//! management and execution pipeline as [`crate::types::RN1Signal`].

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::types::OrderSide;

// ─── Signal Source ──────────────────────────────────────────────────────────

/// Identifies where a trading signal originated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SignalSource {
    /// Shadow-copy of a tracked wallet's order (existing RN1 pipeline).
    Rn1Copytrade,
    /// AI/LLM-generated autonomous signal from the Python sidecar.
    AiAutonomous {
        model: String,
        prompt_id: String,
    },
    /// Smart-money wallet convergence detection (Bullpen).
    SmartMoneyConvergence,
}

// ─── Alpha Signal ───────────────────────────────────────────────────────────

/// A trading recommendation produced by the AI analysis pipeline.
///
/// Submitted by the Python sidecar via `submit_alpha_signal` RPC.
/// The Rust engine applies its own risk checks before executing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaSignal {
    /// Polymarket token ID to trade.
    pub token_id: String,
    /// Polymarket condition ID (0x…).
    #[serde(default)]
    pub condition_id: String,
    /// Buy or Sell.
    pub side: OrderSide,
    /// LLM confidence in the recommendation (0.0–1.0).
    pub confidence: f64,
    /// Suggested limit price (decimal, e.g. 0.55).
    pub recommended_price: f64,
    /// Suggested order size in USDC.
    pub recommended_size_usdc: f64,
    /// LLM reasoning chain (for logging/auditability).
    #[serde(default)]
    pub reasoning: String,
    /// Source metadata.
    pub source: SignalSource,
    /// Unique analysis ID for ClickHouse traceability.
    pub analysis_id: String,
    /// Wall-clock detection timestamp (set by engine on receipt).
    #[serde(skip)]
    pub received_at: Option<Instant>,
}

// ─── Unified Trading Signal ─────────────────────────────────────────────────

/// Unified signal type that flows through the shared execution pipeline.
///
/// Both RN1 copytrade signals and AI-generated alpha signals converge here
/// before hitting risk management and order execution.
pub enum TradingSignal {
    Rn1(crate::types::RN1Signal),
    Alpha(AlphaSignal),
}

impl TradingSignal {
    pub fn token_id(&self) -> &str {
        match self {
            TradingSignal::Rn1(s) => &s.token_id,
            TradingSignal::Alpha(s) => &s.token_id,
        }
    }

    pub fn side(&self) -> OrderSide {
        match self {
            TradingSignal::Rn1(s) => s.side,
            TradingSignal::Alpha(s) => s.side,
        }
    }

    pub fn source_label(&self) -> &'static str {
        match self {
            TradingSignal::Rn1(_) => "rn1",
            TradingSignal::Alpha(_) => "alpha",
        }
    }
}

// ─── Alpha Risk Config ──────────────────────────────────────────────────────

/// Separate risk limits for AI-generated signals.
///
/// Intentionally conservative defaults — the AI pipeline must earn trust
/// through paper-trading performance before limits are relaxed.
#[derive(Debug, Clone)]
pub struct AlphaRiskConfig {
    /// Master kill switch for alpha trading.
    pub enabled: bool,
    /// Minimum LLM confidence to accept a signal (default 0.65).
    pub confidence_floor: f64,
    /// Maximum USDC per single alpha order (default $5).
    pub max_single_order_usdc: f64,
    /// Maximum concurrent alpha positions (default 3).
    pub max_concurrent_positions: usize,
    /// Maximum daily loss from alpha trades as fraction of NAV (default 5%).
    pub max_daily_loss_pct: f64,
    /// Maximum age of an alpha signal before it's considered stale (seconds).
    pub max_signal_age_secs: u64,
}

impl Default for AlphaRiskConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            confidence_floor: 0.65,
            max_single_order_usdc: 5.0,
            max_concurrent_positions: 3,
            max_daily_loss_pct: 0.05,
            max_signal_age_secs: 60,
        }
    }
}

impl AlphaRiskConfig {
    /// Loads alpha risk config from environment variables.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        cfg.enabled = std::env::var("ALPHA_TRADING_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        if let Ok(v) = std::env::var("ALPHA_CONFIDENCE_FLOOR") {
            cfg.confidence_floor = v.parse().unwrap_or(cfg.confidence_floor);
        }
        if let Ok(v) = std::env::var("ALPHA_MAX_SINGLE_ORDER_USDC") {
            cfg.max_single_order_usdc = v.parse().unwrap_or(cfg.max_single_order_usdc);
        }
        if let Ok(v) = std::env::var("ALPHA_MAX_CONCURRENT_POSITIONS") {
            cfg.max_concurrent_positions = v.parse().unwrap_or(cfg.max_concurrent_positions);
        }
        if let Ok(v) = std::env::var("ALPHA_MAX_DAILY_LOSS_PCT") {
            cfg.max_daily_loss_pct = v.parse().unwrap_or(cfg.max_daily_loss_pct);
        }
        if let Ok(v) = std::env::var("ALPHA_MAX_SIGNAL_AGE_SECS") {
            cfg.max_signal_age_secs = v.parse().unwrap_or(cfg.max_signal_age_secs);
        }

        cfg
    }
}

// ─── Alpha Cycle Reporting ──────────────────────────────────────────────────

/// A single market's analysis result within a cycle (reported by the sidecar).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlphaCycleMarket {
    pub question: String,
    pub yes_price: f64,
    #[serde(default)]
    pub llm_probability: Option<f64>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub edge_bps: Option<f64>,
    /// "BUY", "SELL", "PASS", "LOW_EDGE", "SUBMITTED"
    pub action: String,
}

/// Cycle-level report sent by the Python sidecar after each analysis run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlphaCycleReport {
    pub markets_scanned: u32,
    pub markets_analyzed: u32,
    pub signals_generated: u32,
    pub signals_submitted: u32,
    pub cycle_duration_secs: f64,
    #[serde(default)]
    pub top_markets: Vec<AlphaCycleMarket>,
}

// ─── Alpha Analytics ────────────────────────────────────────────────────────

/// Tracks AI trading performance separately from RN1.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AlphaAnalytics {
    pub signals_received: u64,
    pub signals_accepted: u64,
    pub signals_rejected: u64,
    pub reject_reasons: std::collections::HashMap<String, u64>,
    pub realized_pnl_usdc: f64,
    pub unrealized_pnl_usdc: f64,
    pub positions_opened: u64,
    pub positions_closed: u64,
    // Cycle reporting (updated by sidecar via RPC)
    pub cycles_completed: u64,
    pub last_cycle_at: Option<String>,
    pub last_cycle_markets_scanned: u32,
    pub last_cycle_markets_analyzed: u32,
    pub last_cycle_signals_generated: u32,
    pub last_cycle_signals_submitted: u32,
    pub last_cycle_duration_secs: f64,
    pub last_cycle_top_markets: Vec<AlphaCycleMarket>,
}

impl AlphaAnalytics {
    pub fn record_accept(&mut self) {
        self.signals_received += 1;
        self.signals_accepted += 1;
    }

    pub fn record_reject(&mut self, reason: &str) {
        self.signals_received += 1;
        self.signals_rejected += 1;
        *self.reject_reasons.entry(reason.to_string()).or_default() += 1;
    }

    pub fn record_cycle(&mut self, report: AlphaCycleReport) {
        self.cycles_completed += 1;
        self.last_cycle_at = Some(chrono::Utc::now().to_rfc3339());
        self.last_cycle_markets_scanned = report.markets_scanned;
        self.last_cycle_markets_analyzed = report.markets_analyzed;
        self.last_cycle_signals_generated = report.signals_generated;
        self.last_cycle_signals_submitted = report.signals_submitted;
        self.last_cycle_duration_secs = report.cycle_duration_secs;
        self.last_cycle_top_markets = report.top_markets;
    }
}
