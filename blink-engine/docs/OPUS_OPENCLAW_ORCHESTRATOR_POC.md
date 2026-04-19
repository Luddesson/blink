# Opus OpenClaw Orchestrator POC (Control-Plane Only)

## Overview

OpenClaw orchestrator in `blink-cli` is a read-only workflow runner with strict policy guardrails.

- Command group: `blink orchestrator`
- Module: `crates/blink-cli/src/commands/orchestrator.rs`
- Purpose: run deterministic control-plane evaluation sequences and emit a machine-readable ledger

Supported sequence profiles:

- `full-cycle`
- `integrity`
- `decision`
- `confidence`

## Guardrail Policy Spec

The policy model is explicit and deny-first:

### 1) Method allow/deny controls

- `allow_methods`: explicit allowlist (`GET`, `CMD`)
- `deny_methods`: explicit denylist (`POST`, `PUT`, `PATCH`, `DELETE`)
- Any denied or non-allowlisted method is blocked before execution.

### 2) Endpoint allow/deny controls

- `allow_paths`:
  - `/api/status`
  - `/api/mode`
  - `/api/risk`
  - `/api/latency`
  - `/api/portfolio`
- `deny_path_fragments`: mutation/control fragments like `/api/pause`, `/api/orders`, `/api/positions`, `emergency-stop`
- `deny_keywords`: high-risk terms like `live`, `trade`, `buy`, `sell`, `cancel`, `resume`, `pause`

### 3) Environment constraints

- `allow_engine_hosts`: localhost engine endpoints only
- `deny_true_env_vars`: deny online execution when dangerous env flags are truthy (`LIVE_TRADING`, `TRADING_ENABLED`)
- `--offline` mode is explicitly allowed and skips online environment checks.

### 4) Command constraints

- `allow_commands`: allowlisted local commands only (`echo`, `printf`)
- `deny_fragments`: dangerous command fragments (`cargo run`, `live`, `trade`, `order`, `cancel`)
- `max_command_len` and `max_args` cap command complexity
- Non-allowlisted or denied command patterns are blocked.

### 5) Artifact boundary checks

- `allow_root_dirs`: output artifacts must stay under `logs\` or `reports\`
- `require_extension`: `.json` only
- `deny_absolute_path`: absolute output paths blocked
- `deny_parent_traversal`: `..` traversal blocked

If a requested ledger path fails boundary checks, it is denied and the run falls back to the safe default `logs\orchestrator-ledger-<sequence>-<timestamp>.json`.

## Execution Ledger Contract

Every run writes a JSON ledger with:

- policy snapshot (`policy`)
- guardrail decisions (`guardrail_decisions`) with:
  - `scope`
  - `outcome` (`allowed` / `denied`)
  - `rule`
  - `reason`
  - `timestamp_ms`
- denied action records (`denied_actions`) with step/action/rule/reason
- per-step execution records (`executed_steps`) including `guardrail_rule` and `guardrail_reason`
- run-level status (`success`, `completed_with_denials`, `failed`)

This ensures denied actions are auditable with explicit reasons.

## Examples

### Allowed safe flow

```bash
cargo run -p blink-cli -- orchestrator run --sequence integrity --offline
```

Expected: read-only GET workflow succeeds; ledger includes allowed guardrail decisions.

### Denied risky actions (smoke)

```bash
cargo run -p blink-cli -- orchestrator smoke
```

Expected: probe steps for `POST /api/pause`, `POST /api/orders/cancel-all`, and risky local command are denied and recorded in `denied_actions`.

### Denied unsafe ledger path

```bash
cargo run -p blink-cli -- orchestrator run --sequence integrity --offline --ledger ..\..\secret.txt
```

Expected: artifact boundary denial recorded; output redirected to safe default under `logs\`.
