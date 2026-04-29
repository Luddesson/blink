//! Alpha signal types for AI-driven autonomous trading.
//!
//! An [`AlphaSignal`] is produced by the Python sidecar (LLM analysis pipeline)
//! and submitted via the agent RPC server. It flows through the same risk
//! management and order execution pipeline as RN1 signals.

use std::collections::VecDeque;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::types::OrderSide;

// ─── Alpha Signal ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaSignal {
    pub token_id: String,
    pub side: OrderSide,
    pub confidence: f64,
    pub expected_move_bps: u64,
    pub reasoning: String,
    /// Source metadata.
    pub source: SignalSource,
    /// Unique analysis ID for ClickHouse traceability.
    pub analysis_id: String,
    /// Human-readable market question (set by sidecar).
    #[serde(default)]
    pub market_question: String,
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

// ─── Analytics & History ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalSource {
    pub strategy: String,
    pub model: String,
    pub temperature: f64,
    pub window_hours: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaSignalRecord {
    pub timestamp_ms: u64,
    pub token_id: String,
    pub analysis_id: String,
    pub side: OrderSide,
    pub confidence: f64,
    pub expected_move_bps: u64,
    pub status: String, // "accepted", "rejected", "opened", "filled", "failed"
    pub reject_reason: Option<String>,
    pub position_id: Option<usize>,
    pub entry_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaCycleReport {
    pub markets_scanned: u32,
    pub markets_analyzed: u32,
    pub signals_generated: u32,
    pub signals_submitted: u32,
    pub duration_secs: f64,
    pub top_markets: Vec<AlphaCycleMarket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaCycleMarket {
    pub token_id: String,
    pub title: String,
    pub confidence: f64,
    pub reasoning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlphaCycleSnapshot {
    pub timestamp: String,
    pub markets_scanned: u32,
    pub markets_analyzed: u32,
    pub signals_generated: u32,
    pub signals_submitted: u32,
    pub duration_secs: f64,
}

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
        if let Some(rec) = self
            .signal_history
            .iter_mut()
            .rev()
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
        if let Some(rec) = self
            .signal_history
            .iter_mut()
            .rev()
            .find(|r| r.analysis_id == analysis_id)
        {
            rec.status = "engine_rejected".to_string();
        }
    }

    pub fn add_cycle_report(&mut self, report: AlphaCycleReport) {
        self.cycles_completed += 1;
        self.last_cycle_at = Some(chrono::Utc::now().to_rfc3339());
        self.last_cycle_markets_scanned = report.markets_scanned;
        self.last_cycle_markets_analyzed = report.markets_analyzed;
        self.last_cycle_signals_generated = report.signals_generated;
        self.last_cycle_signals_submitted = report.signals_submitted;
        self.last_cycle_duration_secs = report.duration_secs;
        self.last_cycle_top_markets = report.top_markets.clone();

        let snapshot = AlphaCycleSnapshot {
            timestamp: chrono::Utc::now().to_rfc3339(),
            markets_scanned: report.markets_scanned,
            markets_analyzed: report.markets_analyzed,
            signals_generated: report.signals_generated,
            signals_submitted: report.signals_submitted,
            duration_secs: report.duration_secs,
        };

        if self.cycle_history.len() >= MAX_CYCLE_HISTORY {
            self.cycle_history.pop_front();
        }
        self.cycle_history.push_back(snapshot);
    }

    pub fn win_rate_pct(&self) -> f64 {
        if self.win_count + self.loss_count == 0 {
            return 0.0;
        }
        (self.win_count as f64 / (self.win_count + self.loss_count) as f64) * 100.0
    }
    pub fn avg_pnl_per_trade(&self) -> f64 {
        let total = self.win_count + self.loss_count;
        if total == 0 {
            return 0.0;
        }
        self.realized_pnl_usdc / total as f64
    }
    pub fn add_signal_calibration(&mut self, record: AlphaSignalRecord) {
        // Calibration records are essentially detailed signal history entries
        // but might include retrospective ground-truth data from the sidecar.
        // For now, we just record them into the history ring.
        self.record_signal(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_risk_config_safety() {
        // Ensure defaults are always conservative.
        let cfg = AlphaRiskConfig::default();
        assert!(
            cfg.max_single_order_usdc <= 10.0,
            "Max order must be \u{2264} $10 for safety"
        );
        assert!(
            cfg.max_concurrent_positions <= 5,
            "Max positions must be \u{2264} 5"
        );
    }
}
