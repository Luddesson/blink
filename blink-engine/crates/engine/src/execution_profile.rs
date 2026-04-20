//! Execution profile — describes **HOW** orders are executed, separate from
//! [`crate::strategy::StrategyMode`] which describes the trading intent.
//!
//! Rubber-duck rationale: `StrategyMode::Aggressive` conflates "take more
//! risk / size up" (intent) with "send orders faster / looser gates"
//! (execution mechanics). The execution profile is an orthogonal dial that
//! tunes the gate + router knobs without touching signal intent.
//!
//! # Environment variable
//! - `BLINK_EXECUTION_PROFILE` — one of `passive | balanced | hft_taker | hft_maker`
//!   (default: `balanced`).
//!
//! # Knobs
//! Each profile resolves to an [`ExecutionProfileKnobs`] bundle consumed by:
//! - [`crate::pretrade_gate::GateConfig`] (freshness + drift + post-only)
//! - [`crate::signal_pipeline::per_token_queue_depth`] (worker concurrency)
//!
//! Individual env overrides (`BLINK_GATE_STALE_MS`, `BLINK_GATE_MAX_DRIFT_BPS`,
//! `BLINK_GATE_POST_ONLY`, `BLINK_SIGNAL_PER_TOKEN_QUEUE`) still win when set —
//! the profile only changes the *default* value.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionProfile {
    /// Classic taker-only, conservative gates. Widest slippage tolerance, slow.
    Passive,
    /// Current aggressive defaults — the historical behaviour. Default.
    Balanced,
    /// HFT taker: minimal pretrade wait, smaller orders, faster concurrency.
    HftTaker,
    /// HFT maker: layered maker quotes with post-only. Stub defaults here;
    /// full layering is implemented in the maker-layering task.
    HftMaker,
}

impl Default for ExecutionProfile {
    fn default() -> Self {
        ExecutionProfile::Balanced
    }
}

/// Resolved knob bundle for a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionProfileKnobs {
    /// Max book-snapshot age (ms) before the pretrade gate skips a signal.
    pub pretrade_gate_stale_ms: u32,
    /// Max |price − ref| drift (bps) tolerated by the pretrade gate.
    pub pretrade_gate_drift_bps: u16,
    /// Max concurrent in-flight signals per token (signal pipeline queue depth).
    pub max_concurrent_per_token: usize,
    /// Whether the gate enforces post-only cross-check by default.
    pub post_only: bool,
}

impl ExecutionProfile {
    pub fn from_env() -> Self {
        match std::env::var("BLINK_EXECUTION_PROFILE") {
            Ok(raw) => raw.parse().unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn knobs(self) -> ExecutionProfileKnobs {
        match self {
            ExecutionProfile::Passive => ExecutionProfileKnobs {
                pretrade_gate_stale_ms: 1_500,
                pretrade_gate_drift_bps: 40,
                max_concurrent_per_token: 16,
                post_only: true,
            },
            ExecutionProfile::Balanced => ExecutionProfileKnobs {
                pretrade_gate_stale_ms: 800,
                pretrade_gate_drift_bps: 80,
                max_concurrent_per_token: 64,
                post_only: true,
            },
            ExecutionProfile::HftTaker => ExecutionProfileKnobs {
                pretrade_gate_stale_ms: 250,
                pretrade_gate_drift_bps: 120,
                max_concurrent_per_token: 128,
                post_only: false,
            },
            ExecutionProfile::HftMaker => ExecutionProfileKnobs {
                pretrade_gate_stale_ms: 400,
                pretrade_gate_drift_bps: 60,
                max_concurrent_per_token: 128,
                post_only: true,
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionProfile::Passive => "passive",
            ExecutionProfile::Balanced => "balanced",
            ExecutionProfile::HftTaker => "hft_taker",
            ExecutionProfile::HftMaker => "hft_maker",
        }
    }
}

impl fmt::Display for ExecutionProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExecutionProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "passive" => Ok(Self::Passive),
            "balanced" | "" => Ok(Self::Balanced),
            "hft_taker" | "hft-taker" | "hfttaker" => Ok(Self::HftTaker),
            "hft_maker" | "hft-maker" | "hftmaker" => Ok(Self::HftMaker),
            other => Err(format!(
                "invalid execution profile '{other}' (expected passive|balanced|hft_taker|hft_maker)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_known_variants_and_aliases() {
        assert_eq!("passive".parse::<ExecutionProfile>().unwrap(), ExecutionProfile::Passive);
        assert_eq!("Balanced".parse::<ExecutionProfile>().unwrap(), ExecutionProfile::Balanced);
        assert_eq!("hft_taker".parse::<ExecutionProfile>().unwrap(), ExecutionProfile::HftTaker);
        assert_eq!("hft-maker".parse::<ExecutionProfile>().unwrap(), ExecutionProfile::HftMaker);
        assert!("turbo".parse::<ExecutionProfile>().is_err());
    }

    #[test]
    fn knobs_differ_between_profiles() {
        let passive = ExecutionProfile::Passive.knobs();
        let hft_taker = ExecutionProfile::HftTaker.knobs();
        assert!(passive.pretrade_gate_stale_ms > hft_taker.pretrade_gate_stale_ms);
        assert!(hft_taker.max_concurrent_per_token >= passive.max_concurrent_per_token);
        assert!(!hft_taker.post_only);
    }

    #[test]
    fn default_is_balanced() {
        assert_eq!(ExecutionProfile::default(), ExecutionProfile::Balanced);
    }
}
