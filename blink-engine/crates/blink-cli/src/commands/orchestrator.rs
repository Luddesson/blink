//! `blink orchestrator` — safe OpenClaw-style control-plane eval orchestrator.
//!
//! Runs policy-constrained eval sequences and writes a machine-readable execution ledger.
//! Guardrails explicitly deny mutating or live-trade actions.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;
use serde::Serialize;
use serde_json::Value;

use crate::{client::CliContext, OutputFormat};

#[derive(Args)]
pub struct OrchestratorArgs {
    #[command(subcommand)]
    pub sub: OrchestratorCmd,
}

#[derive(Subcommand)]
pub enum OrchestratorCmd {
    /// Run an eval sequence and emit a ledger JSON artifact.
    Run(RunArgs),
    /// Local smoke run: offline + guardrail probe (no engine required).
    Smoke(SmokeArgs),
}

#[derive(Args, Clone)]
pub struct RunArgs {
    /// Eval sequence profile.
    #[arg(long, value_enum, default_value = "full-cycle")]
    pub sequence: EvalSequence,
    /// Output ledger file path.
    #[arg(long)]
    pub ledger: Option<PathBuf>,
    /// Do not call the engine; produce deterministic mock outputs.
    #[arg(long, default_value_t = false)]
    pub offline: bool,
    /// Add intentionally denied actions to prove guardrails are active.
    #[arg(long, default_value_t = false)]
    pub include_guardrail_probe: bool,
}

#[derive(Args, Clone)]
pub struct SmokeArgs {
    /// Output ledger file path.
    #[arg(long)]
    pub ledger: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvalSequence {
    FullCycle,
    Integrity,
    Decision,
    Confidence,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum StepAction {
    EngineGet { path: String },
    EnginePost { path: String, body: Value },
    LocalCommand { command: String, args: Vec<String> },
}

#[derive(Clone, Debug, Serialize)]
struct PlannedStep {
    id: String,
    name: String,
    action: StepAction,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum StepStatus {
    Success,
    Denied,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum GuardrailOutcome {
    Allowed,
    Denied,
}

#[derive(Debug, Clone, Serialize)]
struct GuardrailDecision {
    scope: String,
    outcome: GuardrailOutcome,
    rule: String,
    reason: String,
    timestamp_ms: u128,
}

#[derive(Debug, Serialize)]
struct ExecutedStep {
    id: String,
    name: String,
    status: StepStatus,
    started_at_ms: u128,
    finished_at_ms: u128,
    output: Option<Value>,
    error: Option<String>,
    guardrail_rule: String,
    guardrail_reason: String,
}

#[derive(Debug, Serialize)]
struct DeniedAction {
    id: String,
    name: String,
    action: StepAction,
    rule: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
    Success,
    CompletedWithDenials,
    Failed,
}

#[derive(Debug, Serialize)]
struct EnvironmentGuardrail {
    allow_engine_hosts: Vec<String>,
    deny_true_env_vars: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CommandGuardrail {
    allow_commands: Vec<String>,
    deny_fragments: Vec<String>,
    max_command_len: usize,
    max_args: usize,
}

#[derive(Debug, Serialize)]
struct ArtifactGuardrail {
    allow_root_dirs: Vec<String>,
    require_extension: String,
    deny_absolute_path: bool,
    deny_parent_traversal: bool,
}

#[derive(Debug, Serialize)]
struct GuardrailPolicy {
    allow_methods: Vec<String>,
    deny_methods: Vec<String>,
    allow_paths: Vec<String>,
    deny_path_fragments: Vec<String>,
    deny_keywords: Vec<String>,
    environment: EnvironmentGuardrail,
    command: CommandGuardrail,
    artifact: ArtifactGuardrail,
}

#[derive(Debug, Serialize)]
struct ExecutionLedger {
    run_id: String,
    sequence: EvalSequence,
    started_at_ms: u128,
    finished_at_ms: u128,
    status: RunStatus,
    offline: bool,
    policy: GuardrailPolicy,
    planned_steps: Vec<PlannedStep>,
    executed_steps: Vec<ExecutedStep>,
    denied_actions: Vec<DeniedAction>,
    guardrail_decisions: Vec<GuardrailDecision>,
    errors: Vec<String>,
}

#[derive(Debug)]
struct GuardrailFailure {
    rule: String,
    reason: String,
}

impl GuardrailFailure {
    fn denied(rule: &str, reason: impl Into<String>) -> Self {
        Self {
            rule: rule.to_string(),
            reason: reason.into(),
        }
    }
}

pub async fn run(ctx: CliContext, args: OrchestratorArgs) -> Result<()> {
    match args.sub {
        OrchestratorCmd::Run(run_args) => run_sequence(&ctx, run_args).await,
        OrchestratorCmd::Smoke(smoke_args) => {
            run_sequence(
                &ctx,
                RunArgs {
                    sequence: EvalSequence::Integrity,
                    ledger: smoke_args.ledger,
                    offline: true,
                    include_guardrail_probe: true,
                },
            )
            .await
        }
    }
}

async fn run_sequence(ctx: &CliContext, args: RunArgs) -> Result<()> {
    let policy = guardrail_policy();
    let run_id = format!(
        "openclaw-orchestrator-{}-{}",
        format!("{:?}", args.sequence).to_lowercase(),
        now_ms()
    );
    let started_at = now_ms();
    let planned_steps = build_plan(args.sequence, args.include_guardrail_probe);
    let mut executed_steps: Vec<ExecutedStep> = Vec::with_capacity(planned_steps.len());
    let mut denied_actions: Vec<DeniedAction> = Vec::new();
    let mut guardrail_decisions: Vec<GuardrailDecision> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    if let Some(decision) = evaluate_environment_guardrails(ctx, args.offline, &policy) {
        if matches!(decision.outcome, GuardrailOutcome::Denied) {
            errors.push(format!("run denied: {}", decision.reason));
        }
        guardrail_decisions.push(decision);
    }

    for step in &planned_steps {
        let step_started = now_ms();
        let decision = evaluate_step_guardrail(step, &policy);
        let denied = matches!(decision.outcome, GuardrailOutcome::Denied);
        let guardrail_rule = decision.rule.clone();
        let guardrail_reason = decision.reason.clone();
        guardrail_decisions.push(decision);

        if denied {
            errors.push(format!("{} denied: {}", step.id, guardrail_reason));
            denied_actions.push(DeniedAction {
                id: step.id.clone(),
                name: step.name.clone(),
                action: step.action.clone(),
                rule: guardrail_rule.clone(),
                reason: guardrail_reason.clone(),
            });
            executed_steps.push(ExecutedStep {
                id: step.id.clone(),
                name: step.name.clone(),
                status: StepStatus::Denied,
                started_at_ms: step_started,
                finished_at_ms: now_ms(),
                output: None,
                error: Some(guardrail_reason.clone()),
                guardrail_rule,
                guardrail_reason,
            });
            continue;
        }

        let result = execute_step(ctx, step, args.offline).await;
        match result {
            Ok(output) => executed_steps.push(ExecutedStep {
                id: step.id.clone(),
                name: step.name.clone(),
                status: StepStatus::Success,
                started_at_ms: step_started,
                finished_at_ms: now_ms(),
                output: Some(output),
                error: None,
                guardrail_rule,
                guardrail_reason,
            }),
            Err(err) => {
                let msg = err.to_string();
                errors.push(format!("{} failed: {msg}", step.id));
                executed_steps.push(ExecutedStep {
                    id: step.id.clone(),
                    name: step.name.clone(),
                    status: StepStatus::Failed,
                    started_at_ms: step_started,
                    finished_at_ms: now_ms(),
                    output: None,
                    error: Some(msg),
                    guardrail_rule,
                    guardrail_reason,
                });
            }
        }
    }

    let has_failed = executed_steps
        .iter()
        .any(|s| matches!(s.status, StepStatus::Failed));
    let has_denied = executed_steps
        .iter()
        .any(|s| matches!(s.status, StepStatus::Denied));

    let status = if has_failed {
        RunStatus::Failed
    } else if has_denied {
        RunStatus::CompletedWithDenials
    } else {
        RunStatus::Success
    };

    let default_ledger = PathBuf::from(format!(
        "logs\\orchestrator-ledger-{}-{}.json",
        format!("{:?}", args.sequence).to_lowercase(),
        now_ms()
    ));
    let requested_ledger = args.ledger.clone().unwrap_or(default_ledger.clone());
    let artifact_decision = evaluate_artifact_guardrail(&requested_ledger, &policy);
    let ledger_path = if matches!(artifact_decision.outcome, GuardrailOutcome::Denied) {
        errors.push(format!(
            "ledger path denied: {} ({})",
            requested_ledger.display(),
            artifact_decision.reason
        ));
        default_ledger
    } else {
        requested_ledger
    };
    guardrail_decisions.push(artifact_decision);

    let ledger = ExecutionLedger {
        run_id,
        sequence: args.sequence,
        started_at_ms: started_at,
        finished_at_ms: now_ms(),
        status,
        offline: args.offline,
        policy,
        planned_steps,
        executed_steps,
        denied_actions,
        guardrail_decisions,
        errors,
    };
    write_ledger(&ledger_path, &ledger)?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&ledger)?);
        return Ok(());
    }

    println!(
        "\n{} {}",
        "OpenClaw orchestrator run:".bold(),
        ledger.run_id.bold()
    );
    println!("  sequence: {:?}", ledger.sequence);
    println!("  offline: {}", ledger.offline);
    println!("  status: {:?}", ledger.status);
    println!("  planned: {}", ledger.planned_steps.len());
    println!("  executed: {}", ledger.executed_steps.len());
    println!("  denied: {}", ledger.denied_actions.len());
    println!(
        "  guardrail decisions: {}",
        ledger.guardrail_decisions.len()
    );
    println!("  ledger: {}", ledger_path.display().to_string().cyan());

    if !ledger.errors.is_empty() {
        println!("{}", "\nErrors:".yellow().bold());
        for e in &ledger.errors {
            println!("  - {e}");
        }
    }

    Ok(())
}

fn build_plan(sequence: EvalSequence, include_guardrail_probe: bool) -> Vec<PlannedStep> {
    let mut steps = match sequence {
        EvalSequence::FullCycle => vec![
            get_step("fc-01", "engine status snapshot", "/api/status"),
            get_step("fc-02", "engine mode snapshot", "/api/mode"),
            get_step("fc-03", "risk snapshot", "/api/risk"),
            get_step("fc-04", "latency snapshot", "/api/latency"),
            get_step("fc-05", "portfolio snapshot", "/api/portfolio"),
        ],
        EvalSequence::Integrity => vec![
            get_step("in-01", "engine status snapshot", "/api/status"),
            get_step("in-02", "risk snapshot", "/api/risk"),
            get_step("in-03", "latency snapshot", "/api/latency"),
        ],
        EvalSequence::Decision => vec![
            get_step("de-01", "engine status snapshot", "/api/status"),
            get_step("de-02", "mode snapshot", "/api/mode"),
            get_step("de-03", "portfolio snapshot", "/api/portfolio"),
        ],
        EvalSequence::Confidence => vec![
            get_step("co-01", "engine status snapshot", "/api/status"),
            get_step("co-02", "latency snapshot", "/api/latency"),
            get_step("co-03", "risk snapshot", "/api/risk"),
        ],
    };

    if include_guardrail_probe {
        steps.push(PlannedStep {
            id: "probe-01".to_string(),
            name: "guardrail probe: pause mutation must be denied".to_string(),
            action: StepAction::EnginePost {
                path: "/api/pause".to_string(),
                body: serde_json::json!({ "paused": true }),
            },
        });
        steps.push(PlannedStep {
            id: "probe-02".to_string(),
            name: "guardrail probe: order mutation must be denied".to_string(),
            action: StepAction::EnginePost {
                path: "/api/orders/cancel-all".to_string(),
                body: serde_json::json!({}),
            },
        });
        steps.push(PlannedStep {
            id: "probe-03".to_string(),
            name: "guardrail probe: risky local command must be denied".to_string(),
            action: StepAction::LocalCommand {
                command: "cargo".to_string(),
                args: vec!["run".to_string(), "-p".to_string(), "engine".to_string()],
            },
        });
    }

    steps
}

fn get_step(id: &str, name: &str, path: &str) -> PlannedStep {
    PlannedStep {
        id: id.to_string(),
        name: name.to_string(),
        action: StepAction::EngineGet {
            path: path.to_string(),
        },
    }
}

fn guardrail_policy() -> GuardrailPolicy {
    GuardrailPolicy {
        allow_methods: vec!["GET".to_string(), "CMD".to_string()],
        deny_methods: vec![
            "POST".to_string(),
            "PUT".to_string(),
            "PATCH".to_string(),
            "DELETE".to_string(),
        ],
        allow_paths: vec![
            "/api/status".to_string(),
            "/api/mode".to_string(),
            "/api/risk".to_string(),
            "/api/latency".to_string(),
            "/api/portfolio".to_string(),
        ],
        deny_path_fragments: vec![
            "/api/pause".to_string(),
            "/api/orders".to_string(),
            "/api/positions".to_string(),
            "emergency-stop".to_string(),
        ],
        deny_keywords: vec![
            "live".to_string(),
            "trade".to_string(),
            "buy".to_string(),
            "sell".to_string(),
            "cancel".to_string(),
            "resume".to_string(),
            "pause".to_string(),
        ],
        environment: EnvironmentGuardrail {
            allow_engine_hosts: vec![
                "http://localhost:3030".to_string(),
                "http://127.0.0.1:3030".to_string(),
            ],
            deny_true_env_vars: vec!["LIVE_TRADING".to_string(), "TRADING_ENABLED".to_string()],
        },
        command: CommandGuardrail {
            allow_commands: vec!["echo".to_string(), "printf".to_string()],
            deny_fragments: vec![
                "cargo run".to_string(),
                "live".to_string(),
                "trade".to_string(),
                "order".to_string(),
                "cancel".to_string(),
            ],
            max_command_len: 64,
            max_args: 6,
        },
        artifact: ArtifactGuardrail {
            allow_root_dirs: vec!["logs".to_string(), "reports".to_string()],
            require_extension: ".json".to_string(),
            deny_absolute_path: true,
            deny_parent_traversal: true,
        },
    }
}

fn evaluate_environment_guardrails(
    ctx: &CliContext,
    offline: bool,
    policy: &GuardrailPolicy,
) -> Option<GuardrailDecision> {
    if offline {
        return Some(allow_decision(
            "run.environment",
            "offline_mode",
            "offline mode skips live environment checks",
        ));
    }

    let host = ctx.engine_url.to_ascii_lowercase();
    let allowed_host = policy
        .environment
        .allow_engine_hosts
        .iter()
        .map(|v| v.to_ascii_lowercase())
        .any(|prefix| host.starts_with(&prefix));
    if !allowed_host {
        return Some(deny_decision(
            "run.environment",
            "environment.host_allowlist",
            format!(
                "engine host `{}` is outside localhost allowlist",
                ctx.engine_url
            ),
        ));
    }

    let denied_env = policy
        .environment
        .deny_true_env_vars
        .iter()
        .find(|env_key| {
            std::env::var(env_key)
                .ok()
                .map(|v| is_true_like(&v))
                .unwrap_or(false)
        });
    if let Some(env_key) = denied_env {
        return Some(deny_decision(
            "run.environment",
            "environment.live_env_denied",
            format!("env `{env_key}` is true; run in offline mode for safety"),
        ));
    }

    Some(allow_decision(
        "run.environment",
        "environment.allow",
        "engine host and env flags satisfy safety constraints",
    ))
}

fn evaluate_step_guardrail(step: &PlannedStep, policy: &GuardrailPolicy) -> GuardrailDecision {
    let scope = format!("step.{}", step.id);
    match guardrail_check(step, policy) {
        Ok((rule, reason)) => allow_decision(&scope, &rule, reason),
        Err(failure) => deny_decision(&scope, &failure.rule, failure.reason),
    }
}

fn guardrail_check(
    step: &PlannedStep,
    policy: &GuardrailPolicy,
) -> std::result::Result<(String, String), GuardrailFailure> {
    match &step.action {
        StepAction::EngineGet { path } => {
            let method = "GET";
            evaluate_method(method, policy)?;
            evaluate_path(path, policy)?;
            Ok((
                "path.allowlist".to_string(),
                format!("GET `{path}` allowed by read-only endpoint policy"),
            ))
        }
        StepAction::EnginePost { path, .. } => {
            let method = "POST";
            evaluate_method(method, policy)?;
            evaluate_path(path, policy)?;
            Ok((
                "method.allow".to_string(),
                format!("method `{method}` allowed"),
            ))
        }
        StepAction::LocalCommand { command, args } => {
            let method = "CMD";
            evaluate_method(method, policy)?;
            evaluate_command(command, args, policy)?;
            Ok((
                "command.allowlist".to_string(),
                format!("command `{command}` allowed by command guardrail"),
            ))
        }
    }
}

fn evaluate_method(
    method: &str,
    policy: &GuardrailPolicy,
) -> std::result::Result<(), GuardrailFailure> {
    if policy.deny_methods.iter().any(|v| v == method) {
        return Err(GuardrailFailure::denied(
            "method.denylist",
            format!("method `{method}` denied by guardrail policy"),
        ));
    }
    if !policy.allow_methods.iter().any(|v| v == method) {
        return Err(GuardrailFailure::denied(
            "method.allowlist",
            format!("method `{method}` is not explicitly allowlisted"),
        ));
    }
    Ok(())
}

fn evaluate_path(
    path: &str,
    policy: &GuardrailPolicy,
) -> std::result::Result<(), GuardrailFailure> {
    let path_lower = path.to_ascii_lowercase();
    if policy
        .deny_path_fragments
        .iter()
        .any(|frag| path_lower.contains(&frag.to_ascii_lowercase()))
    {
        return Err(GuardrailFailure::denied(
            "path.deny_fragments",
            format!("path `{path}` denied by mutation guardrail"),
        ));
    }

    if policy
        .deny_keywords
        .iter()
        .any(|kw| path_lower.contains(&kw.to_ascii_lowercase()))
    {
        return Err(GuardrailFailure::denied(
            "path.deny_keywords",
            format!("path `{path}` denied by dangerous keyword guardrail"),
        ));
    }

    if !policy.allow_paths.iter().any(|allowed| allowed == path) {
        return Err(GuardrailFailure::denied(
            "path.allowlist",
            format!("path `{path}` is outside allowlisted read-only control-plane endpoints"),
        ));
    }

    Ok(())
}

fn evaluate_command(
    command: &str,
    args: &[String],
    policy: &GuardrailPolicy,
) -> std::result::Result<(), GuardrailFailure> {
    if command.trim().is_empty() {
        return Err(GuardrailFailure::denied(
            "command.empty",
            "empty command is denied",
        ));
    }
    if command.len() > policy.command.max_command_len {
        return Err(GuardrailFailure::denied(
            "command.length",
            format!(
                "command length {} exceeds max {}",
                command.len(),
                policy.command.max_command_len
            ),
        ));
    }
    if args.len() > policy.command.max_args {
        return Err(GuardrailFailure::denied(
            "command.arg_limit",
            format!(
                "command has {} args which exceeds max {}",
                args.len(),
                policy.command.max_args
            ),
        ));
    }

    let joined = if args.is_empty() {
        command.to_ascii_lowercase()
    } else {
        format!(
            "{} {}",
            command.to_ascii_lowercase(),
            args.join(" ").to_ascii_lowercase()
        )
    };
    if policy
        .command
        .deny_fragments
        .iter()
        .any(|frag| joined.contains(&frag.to_ascii_lowercase()))
    {
        return Err(GuardrailFailure::denied(
            "command.deny_fragments",
            format!("command `{joined}` denied by command policy"),
        ));
    }

    if !policy
        .command
        .allow_commands
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(command))
    {
        return Err(GuardrailFailure::denied(
            "command.allowlist",
            format!("command `{command}` is not allowlisted"),
        ));
    }
    Ok(())
}

fn evaluate_artifact_guardrail(path: &Path, policy: &GuardrailPolicy) -> GuardrailDecision {
    let scope = "artifact.ledger";
    match artifact_guardrail_check(path, policy) {
        Ok(_) => allow_decision(
            scope,
            "artifact.boundary_allow",
            format!(
                "artifact path `{}` is within allowed boundary",
                path.display()
            ),
        ),
        Err(failure) => deny_decision(scope, &failure.rule, failure.reason),
    }
}

fn artifact_guardrail_check(
    path: &Path,
    policy: &GuardrailPolicy,
) -> std::result::Result<(), GuardrailFailure> {
    let normalized_raw = path.to_string_lossy().replace('\\', "/");
    let normalized_path = Path::new(&normalized_raw);

    if policy.artifact.deny_absolute_path && (path.is_absolute() || normalized_path.is_absolute()) {
        return Err(GuardrailFailure::denied(
            "artifact.absolute_path",
            format!("absolute artifact path `{}` is denied", path.display()),
        ));
    }
    if policy.artifact.deny_parent_traversal
        && normalized_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(GuardrailFailure::denied(
            "artifact.path_traversal",
            format!(
                "artifact path `{}` contains parent traversal",
                path.display()
            ),
        ));
    }
    let ext = normalized_path
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| format!(".{}", v.to_ascii_lowercase()));
    let required_ext = policy.artifact.require_extension.to_ascii_lowercase();
    if ext.as_deref() != Some(required_ext.as_str()) {
        return Err(GuardrailFailure::denied(
            "artifact.extension",
            format!(
                "artifact path `{}` must use `{}` extension",
                path.display(),
                policy.artifact.require_extension
            ),
        ));
    }

    let first = normalized_path.components().next().and_then(|c| match c {
        std::path::Component::Normal(v) => v.to_str().map(ToString::to_string),
        _ => None,
    });
    let Some(first_segment) = first else {
        return Err(GuardrailFailure::denied(
            "artifact.root_dir",
            "artifact path must include a root directory".to_string(),
        ));
    };
    if !policy
        .artifact
        .allow_root_dirs
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&first_segment))
    {
        return Err(GuardrailFailure::denied(
            "artifact.root_dir",
            format!(
                "artifact root `{first_segment}` is outside allowed roots {:?}",
                policy.artifact.allow_root_dirs
            ),
        ));
    }

    Ok(())
}

fn allow_decision(scope: &str, rule: &str, reason: impl Into<String>) -> GuardrailDecision {
    GuardrailDecision {
        scope: scope.to_string(),
        outcome: GuardrailOutcome::Allowed,
        rule: rule.to_string(),
        reason: reason.into(),
        timestamp_ms: now_ms(),
    }
}

fn deny_decision(scope: &str, rule: &str, reason: impl Into<String>) -> GuardrailDecision {
    GuardrailDecision {
        scope: scope.to_string(),
        outcome: GuardrailOutcome::Denied,
        rule: rule.to_string(),
        reason: reason.into(),
        timestamp_ms: now_ms(),
    }
}

fn is_true_like(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

async fn execute_step(ctx: &CliContext, step: &PlannedStep, offline: bool) -> Result<Value> {
    if offline {
        return Ok(mock_output(step));
    }

    match &step.action {
        StepAction::EngineGet { path } => ctx.engine_get(path).await,
        StepAction::EnginePost { path, .. } => {
            anyhow::bail!("guardrail failure: attempted to execute denied mutation `{path}`")
        }
        StepAction::LocalCommand { command, .. } => {
            anyhow::bail!("guardrail failure: attempted to execute local command `{command}`")
        }
    }
}

fn mock_output(step: &PlannedStep) -> Value {
    match &step.action {
        StepAction::EngineGet { path } => match path.as_str() {
            "/api/status" => serde_json::json!({
                "mode": "paper",
                "ws_connected": true,
                "trading_paused": false,
                "source": "offline-mock"
            }),
            "/api/mode" => serde_json::json!({
                "mode": "paper",
                "source": "offline-mock"
            }),
            "/api/risk" => serde_json::json!({
                "risk_status": "OK",
                "trading_enabled": false,
                "source": "offline-mock"
            }),
            "/api/latency" => serde_json::json!({
                "p50_us": 120,
                "p99_us": 490,
                "source": "offline-mock"
            }),
            "/api/portfolio" => serde_json::json!({
                "nav": 100.0,
                "open_positions": [],
                "source": "offline-mock"
            }),
            _ => serde_json::json!({
                "path": path,
                "source": "offline-mock"
            }),
        },
        StepAction::EnginePost { path, body } => serde_json::json!({
            "path": path,
            "body": body,
            "source": "offline-mock"
        }),
        StepAction::LocalCommand { command, args } => serde_json::json!({
            "command": command,
            "args": args,
            "source": "offline-mock"
        }),
    }
}

fn write_ledger(path: &Path, ledger: &ExecutionLedger) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(ledger)?;
    fs::write(path, raw).with_context(|| format!("failed to write ledger {}", path.display()))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_safe_get_step() {
        let policy = guardrail_policy();
        let step = PlannedStep {
            id: "t1".to_string(),
            name: "safe".to_string(),
            action: StepAction::EngineGet {
                path: "/api/status".to_string(),
            },
        };

        let decision = evaluate_step_guardrail(&step, &policy);
        assert!(matches!(decision.outcome, GuardrailOutcome::Allowed));
        assert_eq!(decision.rule, "path.allowlist");
    }

    #[test]
    fn denies_mutating_post_step() {
        let policy = guardrail_policy();
        let step = PlannedStep {
            id: "t2".to_string(),
            name: "deny".to_string(),
            action: StepAction::EnginePost {
                path: "/api/pause".to_string(),
                body: serde_json::json!({ "paused": true }),
            },
        };

        let decision = evaluate_step_guardrail(&step, &policy);
        assert!(matches!(decision.outcome, GuardrailOutcome::Denied));
        assert_eq!(decision.rule, "method.denylist");
    }

    #[test]
    fn denies_risky_local_command() {
        let policy = guardrail_policy();
        let step = PlannedStep {
            id: "t3".to_string(),
            name: "deny command".to_string(),
            action: StepAction::LocalCommand {
                command: "cargo".to_string(),
                args: vec!["run".to_string(), "-p".to_string(), "engine".to_string()],
            },
        };

        let decision = evaluate_step_guardrail(&step, &policy);
        assert!(matches!(decision.outcome, GuardrailOutcome::Denied));
        assert_eq!(decision.rule, "command.deny_fragments");
    }

    #[test]
    fn allows_safe_artifact_boundary() {
        let policy = guardrail_policy();
        let decision = evaluate_artifact_guardrail(
            Path::new("logs\\orchestrator-ledger-integrity-123.json"),
            &policy,
        );
        assert!(matches!(decision.outcome, GuardrailOutcome::Allowed));
    }

    #[test]
    fn denies_artifact_path_traversal() {
        let policy = guardrail_policy();
        let decision =
            evaluate_artifact_guardrail(Path::new("logs\\..\\secrets\\ledger.json"), &policy);
        assert!(matches!(decision.outcome, GuardrailOutcome::Denied));
        assert_eq!(decision.rule, "artifact.path_traversal");
    }
}
