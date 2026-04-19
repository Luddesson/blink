//! Runtime strategy mode domain and switching controller.
//!
//! **Latency class: COLD PATH. Never call from the signal → order hot path.**

use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

const DEFAULT_HISTORY_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StrategyMode {
    #[default]
    Mirror,
    Conservative,
    Aggressive,
}

impl StrategyMode {
    pub fn profile(self) -> StrategyProfile {
        match self {
            StrategyMode::Mirror => StrategyProfile {
                min_notional_multiplier: 1.0,
                sizing_multiplier: 1.0,
                price_band_lo_adjust: 0.0,
                price_band_hi_adjust: 0.0,
            },
            StrategyMode::Conservative => StrategyProfile {
                min_notional_multiplier: 2.0,
                sizing_multiplier: 0.5,
                price_band_lo_adjust: 0.06,
                price_band_hi_adjust: -0.06,
            },
            StrategyMode::Aggressive => StrategyProfile {
                min_notional_multiplier: 0.1,
                sizing_multiplier: 4.0,
                price_band_lo_adjust: -0.15,
                price_band_hi_adjust: 0.15,
            },
        }
    }
}

impl fmt::Display for StrategyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StrategyMode::Mirror => write!(f, "mirror"),
            StrategyMode::Conservative => write!(f, "conservative"),
            StrategyMode::Aggressive => write!(f, "aggressive"),
        }
    }
}

impl FromStr for StrategyMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mirror" => Ok(Self::Mirror),
            "conservative" => Ok(Self::Conservative),
            "aggressive" => Ok(Self::Aggressive),
            other => Err(format!(
                "invalid strategy mode '{other}' (expected mirror|conservative|aggressive)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrategyProfile {
    pub min_notional_multiplier: f64,
    pub sizing_multiplier: f64,
    pub price_band_lo_adjust: f64,
    pub price_band_hi_adjust: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct StrategyDecisionContext {
    pub live_trading_active: bool,
    pub rn1_notional_usd: f64,
    pub entry_price: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrategyFilterDecision {
    pub min_notional_multiplier: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrategySizingDecision {
    pub sizing_multiplier: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrategyExecutionDecision {
    pub price_band_lo_adjust: f64,
    pub price_band_hi_adjust: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrategyExitDecision {
    pub max_hold_multiplier: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrategyDecisionSet {
    pub filter: StrategyFilterDecision,
    pub sizing: StrategySizingDecision,
    pub execution: StrategyExecutionDecision,
    pub exit: StrategyExitDecision,
}

pub trait StrategyPolicy {
    fn mode(&self) -> StrategyMode;

    fn filter_decision(&self, _ctx: &StrategyDecisionContext) -> StrategyFilterDecision;

    fn sizing_decision(&self, _ctx: &StrategyDecisionContext) -> StrategySizingDecision;

    fn execution_decision(&self, _ctx: &StrategyDecisionContext) -> StrategyExecutionDecision;

    fn exit_decision(&self, _ctx: &StrategyDecisionContext) -> StrategyExitDecision;

    fn decide(&self, ctx: &StrategyDecisionContext) -> StrategyDecisionSet {
        StrategyDecisionSet {
            filter: self.filter_decision(ctx),
            sizing: self.sizing_decision(ctx),
            execution: self.execution_decision(ctx),
            exit: self.exit_decision(ctx),
        }
    }
}

impl StrategyPolicy for StrategyMode {
    fn mode(&self) -> StrategyMode {
        *self
    }

    fn filter_decision(&self, _ctx: &StrategyDecisionContext) -> StrategyFilterDecision {
        let profile = self.profile();
        StrategyFilterDecision {
            min_notional_multiplier: profile.min_notional_multiplier,
        }
    }

    fn sizing_decision(&self, _ctx: &StrategyDecisionContext) -> StrategySizingDecision {
        let profile = self.profile();
        StrategySizingDecision {
            sizing_multiplier: profile.sizing_multiplier,
        }
    }

    fn execution_decision(&self, _ctx: &StrategyDecisionContext) -> StrategyExecutionDecision {
        let profile = self.profile();
        StrategyExecutionDecision {
            price_band_lo_adjust: profile.price_band_lo_adjust,
            price_band_hi_adjust: profile.price_band_hi_adjust,
        }
    }

    fn exit_decision(&self, _ctx: &StrategyDecisionContext) -> StrategyExitDecision {
        let max_hold_multiplier = match self {
            StrategyMode::Mirror => 1.0,
            StrategyMode::Conservative => 0.8,
            StrategyMode::Aggressive => 1.35,
        };
        StrategyExitDecision {
            max_hold_multiplier,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySwitchRecord {
    pub seq: u64,
    pub switched_at_ms: i64,
    pub from: StrategyMode,
    pub to: StrategyMode,
    pub reason: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySnapshot {
    pub current_mode: StrategyMode,
    pub switch_seq: u64,
    pub last_switched_at_ms: i64,
    pub cooldown_secs: u64,
    pub runtime_switch_enabled: bool,
    pub live_switch_allowed: bool,
    pub require_reason: bool,
    #[serde(default)]
    pub history: Vec<StrategySwitchRecord>,
    pub profile: StrategyProfile,
}

#[derive(Debug, Clone)]
pub struct StrategyControllerConfig {
    pub initial_mode: StrategyMode,
    pub runtime_switch_enabled: bool,
    pub live_switch_allowed: bool,
    pub cooldown_secs: u64,
    pub require_reason: bool,
    pub history_limit: usize,
}

impl StrategyControllerConfig {
    pub fn with_defaults(
        initial_mode: StrategyMode,
        runtime_switch_enabled: bool,
        live_switch_allowed: bool,
        cooldown_secs: u64,
        require_reason: bool,
    ) -> Self {
        Self {
            initial_mode,
            runtime_switch_enabled,
            live_switch_allowed,
            cooldown_secs,
            require_reason,
            history_limit: DEFAULT_HISTORY_LIMIT,
        }
    }
}

#[derive(Debug)]
pub enum StrategySwitchError {
    RuntimeSwitchDisabled,
    LiveSwitchNotAllowed,
    CooldownActive { remaining_secs: u64 },
    ReasonRequired,
}

impl StrategySwitchError {
    pub fn rpc_code(&self) -> i64 {
        match self {
            StrategySwitchError::RuntimeSwitchDisabled => -32010,
            StrategySwitchError::LiveSwitchNotAllowed => -32011,
            StrategySwitchError::CooldownActive { .. } => -32012,
            StrategySwitchError::ReasonRequired => -32013,
        }
    }

    pub fn message(&self) -> String {
        match self {
            StrategySwitchError::RuntimeSwitchDisabled => {
                "strategy runtime switching is disabled".to_string()
            }
            StrategySwitchError::LiveSwitchNotAllowed => {
                "strategy switching is not allowed while live trading is active".to_string()
            }
            StrategySwitchError::CooldownActive { remaining_secs } => {
                format!("strategy switch cooldown active ({remaining_secs}s remaining)")
            }
            StrategySwitchError::ReasonRequired => "strategy switch reason is required".to_string(),
        }
    }
}

#[derive(Debug)]
struct StrategyControllerState {
    current_mode: StrategyMode,
    switch_seq: u64,
    last_switched_at_ms: i64,
    history: VecDeque<StrategySwitchRecord>,
}

pub struct StrategyController {
    runtime_switch_enabled: bool,
    live_switch_allowed: bool,
    cooldown_secs: u64,
    require_reason: bool,
    history_limit: usize,
    state: Mutex<StrategyControllerState>,
}

impl StrategyController {
    pub fn new(config: StrategyControllerConfig) -> Self {
        Self {
            runtime_switch_enabled: config.runtime_switch_enabled,
            live_switch_allowed: config.live_switch_allowed,
            cooldown_secs: config.cooldown_secs,
            require_reason: config.require_reason,
            history_limit: config.history_limit.max(1),
            state: Mutex::new(StrategyControllerState {
                current_mode: config.initial_mode,
                switch_seq: 0,
                last_switched_at_ms: now_ms(),
                history: VecDeque::with_capacity(config.history_limit.max(1)),
            }),
        }
    }

    pub fn snapshot(&self) -> StrategySnapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        StrategySnapshot {
            current_mode: state.current_mode,
            switch_seq: state.switch_seq,
            last_switched_at_ms: state.last_switched_at_ms,
            cooldown_secs: self.cooldown_secs,
            runtime_switch_enabled: self.runtime_switch_enabled,
            live_switch_allowed: self.live_switch_allowed,
            require_reason: self.require_reason,
            history: state.history.iter().cloned().collect(),
            profile: state.current_mode.profile(),
        }
    }

    pub fn history(&self) -> Vec<StrategySwitchRecord> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.history.iter().cloned().collect()
    }

    pub fn switch_mode(
        &self,
        next_mode: StrategyMode,
        reason: Option<String>,
        source: &str,
        live_active: bool,
    ) -> Result<StrategySnapshot, StrategySwitchError> {
        if !self.runtime_switch_enabled {
            return Err(StrategySwitchError::RuntimeSwitchDisabled);
        }
        if live_active && !self.live_switch_allowed {
            return Err(StrategySwitchError::LiveSwitchNotAllowed);
        }
        if self.require_reason && reason.as_deref().unwrap_or("").trim().is_empty() {
            return Err(StrategySwitchError::ReasonRequired);
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.current_mode == next_mode {
            return Ok(self.snapshot_locked(&state));
        }

        let now = now_ms();
        if self.cooldown_secs > 0 {
            let cooldown_ms = self.cooldown_secs.saturating_mul(1_000) as i64;
            let elapsed_ms = now.saturating_sub(state.last_switched_at_ms);
            if state.switch_seq > 0 && elapsed_ms < cooldown_ms {
                let remaining_ms = cooldown_ms - elapsed_ms;
                let remaining_secs = ((remaining_ms + 999) / 1_000) as u64;
                return Err(StrategySwitchError::CooldownActive { remaining_secs });
            }
        }

        state.switch_seq = state.switch_seq.saturating_add(1);
        let seq = state.switch_seq;
        let from = state.current_mode;
        state.current_mode = next_mode;
        state.last_switched_at_ms = now;
        state.history.push_back(StrategySwitchRecord {
            seq,
            switched_at_ms: now,
            from,
            to: next_mode,
            reason: reason.filter(|r| !r.trim().is_empty()),
            source: source.to_string(),
        });
        while state.history.len() > self.history_limit {
            state.history.pop_front();
        }

        Ok(self.snapshot_locked(&state))
    }

    pub fn rollback_to_mirror(&self, reason: Option<String>, source: &str) -> StrategySnapshot {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.current_mode == StrategyMode::Mirror {
            return self.snapshot_locked(&state);
        }
        let now = now_ms();
        state.switch_seq = state.switch_seq.saturating_add(1);
        let seq = state.switch_seq;
        let from = state.current_mode;
        state.current_mode = StrategyMode::Mirror;
        state.last_switched_at_ms = now;
        state.history.push_back(StrategySwitchRecord {
            seq,
            switched_at_ms: now,
            from,
            to: StrategyMode::Mirror,
            reason: Some(
                reason
                    .filter(|r| !r.trim().is_empty())
                    .unwrap_or_else(|| "rollback_to_mirror".to_string()),
            ),
            source: source.to_string(),
        });
        while state.history.len() > self.history_limit {
            state.history.pop_front();
        }

        self.snapshot_locked(&state)
    }

    pub fn restore_snapshot(&self, snapshot: &StrategySnapshot) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.current_mode = snapshot.current_mode;
        state.switch_seq = snapshot.switch_seq;
        state.last_switched_at_ms = snapshot.last_switched_at_ms;
        state.history = snapshot.history.iter().cloned().collect();
        while state.history.len() > self.history_limit {
            state.history.pop_front();
        }
    }

    fn snapshot_locked(&self, state: &StrategyControllerState) -> StrategySnapshot {
        StrategySnapshot {
            current_mode: state.current_mode,
            switch_seq: state.switch_seq,
            last_switched_at_ms: state.last_switched_at_ms,
            cooldown_secs: self.cooldown_secs,
            runtime_switch_enabled: self.runtime_switch_enabled,
            live_switch_allowed: self.live_switch_allowed,
            require_reason: self.require_reason,
            history: state.history.iter().cloned().collect(),
            profile: state.current_mode.profile(),
        }
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::{
        StrategyController, StrategyControllerConfig, StrategyDecisionContext, StrategyMode,
        StrategyPolicy, StrategySwitchError,
    };

    #[test]
    fn strategy_mode_parser_accepts_exact_modes() {
        assert_eq!(
            "mirror".parse::<StrategyMode>().ok(),
            Some(StrategyMode::Mirror)
        );
        assert_eq!(
            "conservative".parse::<StrategyMode>().ok(),
            Some(StrategyMode::Conservative)
        );
        assert_eq!(
            "aggressive".parse::<StrategyMode>().ok(),
            Some(StrategyMode::Aggressive)
        );
        assert!("turbo".parse::<StrategyMode>().is_err());
        assert_eq!(
            "  CONSERVATIVE  ".parse::<StrategyMode>().ok(),
            Some(StrategyMode::Conservative)
        );
    }

    #[test]
    fn controller_enforces_reason_and_cooldown() {
        let cfg = StrategyControllerConfig {
            initial_mode: StrategyMode::Mirror,
            runtime_switch_enabled: true,
            live_switch_allowed: true,
            cooldown_secs: 300,
            require_reason: true,
            history_limit: 16,
        };
        let controller = StrategyController::new(cfg);

        let err = controller
            .switch_mode(StrategyMode::Conservative, None, "test", false)
            .expect_err("expected reason-required failure");
        assert!(matches!(err, StrategySwitchError::ReasonRequired));

        controller
            .switch_mode(
                StrategyMode::Conservative,
                Some("volatility spike".to_string()),
                "test",
                false,
            )
            .expect("first switch should pass");

        let err = controller
            .switch_mode(
                StrategyMode::Aggressive,
                Some("retry".to_string()),
                "test",
                false,
            )
            .expect_err("expected cooldown failure");
        assert!(matches!(err, StrategySwitchError::CooldownActive { .. }));
    }

    #[test]
    fn strategy_policy_decisions_match_mode_profile() {
        let mode = StrategyMode::Aggressive;
        let decisions = mode.decide(&StrategyDecisionContext::default());
        assert_eq!(decisions.filter.min_notional_multiplier, 0.1);
        assert_eq!(decisions.sizing.sizing_multiplier, 4.0);
        assert_eq!(decisions.execution.price_band_lo_adjust, -0.15);
        assert_eq!(decisions.execution.price_band_hi_adjust, 0.15);
        assert_eq!(decisions.exit.max_hold_multiplier, 1.35);
        assert_eq!(mode.mode(), StrategyMode::Aggressive);
    }

    #[test]
    fn controller_blocks_live_switches_when_not_allowed() {
        let cfg = StrategyControllerConfig {
            initial_mode: StrategyMode::Mirror,
            runtime_switch_enabled: true,
            live_switch_allowed: false,
            cooldown_secs: 0,
            require_reason: false,
            history_limit: 16,
        };
        let controller = StrategyController::new(cfg);

        let err = controller
            .switch_mode(StrategyMode::Aggressive, None, "test", true)
            .expect_err("expected live-switch guard to reject");
        assert!(matches!(err, StrategySwitchError::LiveSwitchNotAllowed));
    }

    #[test]
    fn rollback_to_mirror_bypasses_runtime_reason_and_cooldown_guards() {
        let cfg = StrategyControllerConfig {
            initial_mode: StrategyMode::Aggressive,
            runtime_switch_enabled: false,
            live_switch_allowed: false,
            cooldown_secs: 300,
            require_reason: true,
            history_limit: 16,
        };
        let controller = StrategyController::new(cfg);

        let snapshot = controller.rollback_to_mirror(None, "test_rollback");
        assert_eq!(snapshot.current_mode, StrategyMode::Mirror);
        assert_eq!(snapshot.switch_seq, 1);
        let record = snapshot
            .history
            .last()
            .expect("rollback should be recorded");
        assert_eq!(record.from, StrategyMode::Aggressive);
        assert_eq!(record.to, StrategyMode::Mirror);
        assert_eq!(record.reason.as_deref(), Some("rollback_to_mirror"));
    }
}
