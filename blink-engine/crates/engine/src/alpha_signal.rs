//! Alpha signal types for AI-driven autonomous trading.
//!
//! An [`AlphaSignal`] is produced by the Python sidecar (LLM analysis pipeline)
//! and submitted via the agent RPC server. It flows through the same risk
//! management and execution pipeline as [`crate::types::RN1Signal`].

use std::collections::VecDeque;
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
    /// "BUY", "SELL", "PASS", "LOW_EDGE", "SUBMITTED", "REJECTED"
    pub action: String,
    /// LLM reasoning text (1-3 sentences explaining the decision).
    #[serde(default)]
    pub reasoning: Option<String>,
    /// CLOB spread in percent (e.g. 0.02 = 2%).
    #[serde(default)]
    pub spread_pct: Option<f64>,
    /// Bid-side depth in USDC (top 5 levels).
    #[serde(default)]
    pub bid_depth_usdc: Option<f64>,
    /// Ask-side depth in USDC (top 5 levels).
    #[serde(default)]
    pub ask_depth_usdc: Option<f64>,
    /// 1-hour price change (e.g. +0.03 = +3%).
    #[serde(default)]
    pub price_change_1h: Option<f64>,
    /// "BUY" or "SELL" direction for submitted signals.
    #[serde(default)]
    pub side: Option<String>,
    /// Token ID for position correlation.
    #[serde(default)]
    pub token_id: Option<String>,
    /// Recommended size in USDC (Kelly output).
    #[serde(default)]
    pub recommended_size_usdc: Option<f64>,
    /// Reasoning chain data (Phase 2) — Call 1 + Devil's Advocate + final combination.
    #[serde(default)]
    pub reasoning_chain: Option<serde_json::Value>,
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

// ─── Signal History ─────────────────────────────────────────────────────────

/// Record of a single alpha signal through its full lifecycle.
///
/// Created when the sidecar submits a signal. Updated when the engine
/// accepts/rejects, opens a position, or closes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaSignalRecord {
    /// ISO 8601 timestamp when the signal was received.
    pub timestamp: String,
    /// Unique analysis ID from the LLM pipeline.
    pub analysis_id: String,
    /// Polymarket token ID.
    pub token_id: String,
    /// Market question text.
    pub market_question: String,
    /// "BUY" or "SELL".
    pub side: String,
    /// LLM confidence (0.0-1.0).
    pub confidence: f64,
    /// LLM reasoning text.
    pub reasoning: String,
    /// Suggested limit price.
    pub recommended_price: f64,
    /// Suggested size in USDC.
    pub recommended_size_usdc: f64,
    /// Lifecycle status: "accepted", "rejected:reason", "opened", "closed"
    pub status: String,
    /// Position ID if a position was opened.
    pub position_id: Option<usize>,
    /// Accumulated realized P&L (handles partial closes).
    pub realized_pnl: Option<f64>,
    /// Current unrealized P&L (updated on API calls).
    pub unrealized_pnl: Option<f64>,
    /// Entry price if position was opened.
    pub entry_price: Option<f64>,
    /// Current price if position is open.
    pub current_price: Option<f64>,
}

/// Snapshot of a single cycle for trend tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaCycleSnapshot {
    /// ISO 8601 timestamp.
    pub timestamp: String,
    pub markets_scanned: u32,
    pub markets_analyzed: u32,
    pub signals_submitted: u32,
    pub signals_accepted: u32,
    pub cycle_duration_secs: f64,
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
    // Signal history (ring buffer — last N signals with full detail)
    pub signal_history: VecDeque<AlphaSignalRecord>,
    // Cycle history (ring buffer — last N cycles for trend charts)
    pub cycle_history: VecDeque<AlphaCycleSnapshot>,
    // Performance metrics
    pub win_count: u64,
    pub loss_count: u64,
    pub best_trade_pnl: f64,
    pub worst_trade_pnl: f64,
    pub total_fees_paid: f64,
    // Calibration data (updated by sidecar via report_alpha_calibration RPC)
    pub calibration: Option<serde_json::Value>,
}

const MAX_SIGNAL_HISTORY: usize = 50;
const MAX_CYCLE_HISTORY: usize = 30;

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

    /// Record a signal into the history ring buffer (called from agent_rpc on submit).
    pub fn record_signal(&mut self, record: AlphaSignalRecord) {
        if self.signal_history.len() >= MAX_SIGNAL_HISTORY {
            self.signal_history.pop_front();
        }
        self.signal_history.push_back(record);
    }

    /// Mark a signal as having opened a position (called from alpha consumer).
    pub fn mark_signal_opened(&mut self, analysis_id: &str, position_id: usize, entry_price: f64) {
        if let Some(rec) = self.signal_history.iter_mut().rev()
            .find(|r| r.analysis_id == analysis_id)
        {
            rec.status = "opened".to_string();
            rec.position_id = Some(position_id);
            rec.entry_price = Some(entry_price);
        }
        self.positions_opened += 1;
    }

    /// Mark a signal as rejected by the engine pipeline (called from alpha consumer).
    pub fn mark_signal_engine_rejected(&mut self, analysis_id: &str) {
        if let Some(rec) = self.signal_history.iter_mut().rev()
            .find(|r| r.analysis_id == analysis_id)
        {
            if rec.status == "accepted" {
                rec.status = "engine_rejected".to_string();
            }
        }
    }

    /// Record realized P&L when an AI position is closed (supports partial closes).
    pub fn record_close(&mut self, analysis_id: &str, pnl: f64) {
        self.realized_pnl_usdc += pnl;
        self.positions_closed += 1;

        if pnl > 0.0 {
            self.win_count += 1;
        } else if pnl < 0.0 {
            self.loss_count += 1;
        }
        if pnl > self.best_trade_pnl { self.best_trade_pnl = pnl; }
        if pnl < self.worst_trade_pnl { self.worst_trade_pnl = pnl; }

        // Update signal record
        if let Some(rec) = self.signal_history.iter_mut().rev()
            .find(|r| r.analysis_id == analysis_id)
        {
            rec.realized_pnl = Some(rec.realized_pnl.unwrap_or(0.0) + pnl);
            rec.status = "closed".to_string();
        }
    }

    /// Update unrealized P&L for open AI positions (called periodically).
    pub fn update_unrealized(&mut self, analysis_id: &str, unrealized: f64, current_price: f64) {
        if let Some(rec) = self.signal_history.iter_mut().rev()
            .find(|r| r.analysis_id == analysis_id)
        {
            rec.unrealized_pnl = Some(unrealized);
            rec.current_price = Some(current_price);
        }
    }

    pub fn record_cycle(&mut self, report: AlphaCycleReport) {
        self.cycles_completed += 1;
        let now = chrono::Utc::now().to_rfc3339();
        self.last_cycle_at = Some(now.clone());
        self.last_cycle_markets_scanned = report.markets_scanned;
        self.last_cycle_markets_analyzed = report.markets_analyzed;
        self.last_cycle_signals_generated = report.signals_generated;
        self.last_cycle_signals_submitted = report.signals_submitted;
        self.last_cycle_duration_secs = report.cycle_duration_secs;
        self.last_cycle_top_markets = report.top_markets;

        // Record cycle snapshot for trend tracking
        if self.cycle_history.len() >= MAX_CYCLE_HISTORY {
            self.cycle_history.pop_front();
        }
        self.cycle_history.push_back(AlphaCycleSnapshot {
            timestamp: now,
            markets_scanned: self.last_cycle_markets_scanned,
            markets_analyzed: self.last_cycle_markets_analyzed,
            signals_submitted: self.last_cycle_signals_submitted,
            signals_accepted: 0, // updated separately
            cycle_duration_secs: self.last_cycle_duration_secs,
        });
    }

    /// Win rate as a percentage (0-100).
    pub fn win_rate_pct(&self) -> f64 {
        let total = self.win_count + self.loss_count;
        if total == 0 { return 0.0; }
        (self.win_count as f64 / total as f64) * 100.0
    }

    /// Average P&L per closed trade.
    pub fn avg_pnl_per_trade(&self) -> f64 {
        let total = self.win_count + self.loss_count;
        if total == 0 { return 0.0; }
        self.realized_pnl_usdc / total as f64
    }
}
