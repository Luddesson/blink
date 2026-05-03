# Blink Hyper Mode

Snapshot date: 2026-05-02 UTC

Purpose: run Blink live-canary work with maximum parallel analysis while preserving hard risk controls. This mode optimizes for evidence quality, auditability, and fast operator decisions. It does not override compliance, circuit breakers, signoff, or capital gates.

## Prime Directive

No live-risk action is allowed unless the live go-live TODO gates are satisfied or explicitly waived in a signed artifact.

Live-risk actions include:

- resetting the circuit breaker,
- increasing capital or order caps,
- enabling a broader market policy,
- changing deployed env mode,
- restarting into a new binary,
- submitting, forcing, or encouraging live orders.

## Active Runtime Doctrine

- Current live source of truth: `blink-engine/to-do.md`.
- Older planning source: `docs/TODO.md`.
- Default decision while any hard gate is open: `TUNE`.
- Default trading posture while status is uncertain: observer or blocked by risk.
- Exchange/wallet truth outranks local optimistic state.
- Evidence folder outranks terminal scrollback.
- Human signoff outranks model confidence.

## Subagent Swarm

Each subagent must run read-only unless explicitly assigned a disjoint write scope.

1. Risk Sentinel
   - Owns: circuit breaker, heartbeat, stale orders, wallet truth, geoblock, risk status.
   - Output: `GO`, `TUNE`, or `ROLLBACK` with blocker evidence.

2. Release Auditor
   - Owns: git status, dirty worktree, generated files, secrets hygiene, release identity.
   - Output: auditable-release blockers and exact files to commit, ignore, or exclude.

3. Canary Gate Architect
   - Owns: sequencing from current state to Stage 0 or Stage 1.
   - Output: read-only commands, signoff-required commands, forbidden commands.

4. Quant Inventory Analyst
   - Owns: NAV, cash, order caps, legacy inventory, PnL pollution risk.
   - Output: capital policy and inventory handling requirements.

5. Verification Runner
   - Owns: non-live tests, preflight evidence, API snapshots, rollback preview.
   - Output: command transcript locations and pass/fail summary.

6. Operator Scribe
   - Owns: signoff draft, evidence index, final decision record.
   - Output: signed or unsigned launch packet with explicit promotion boolean.

## Execution Classes

Class A: read-only safe

- `git status --short`
- `git diff --stat`
- `curl -sS http://127.0.0.1:3030/api/status`
- `curl -sS http://127.0.0.1:3030/api/risk`
- `curl -sS http://127.0.0.1:3030/api/failsafe`
- `curl -sS http://127.0.0.1:3030/api/live/portfolio`
- `curl -sS http://127.0.0.1:3030/api/geoblock`
- `journalctl -u blink-engine ... --no-pager`

Class B: build/test evidence

- `CARGO_INCREMENTAL=0 cargo test -p engine --lib`
- `cargo build --release -p engine`
- deployed `--preflight-live`

Class C: signoff-required mutation

- edit `/etc/blink-engine.env` or `/opt/blink/.env`,
- install or move deployed binaries,
- restart `blink-engine`,
- reset circuit breaker,
- emergency stop when it changes trading state,
- any live canary start action.

Class D: forbidden until all hard gates close

- placing or forcing live orders,
- raising order caps,
- enabling negative-risk trading,
- declaring production live,
- ignoring legacy positions in canary PnL.

## GO/TUNE/ROLLBACK State Machine

`GO` requires all of:

- latest preflight passes after latest restart/env/binary change,
- `risk_status` is not `CIRCUIT_BREAKER`,
- `circuit_breaker_tripped=false`,
- heartbeat is fresh and stable,
- `pending_orders=0`,
- `stale_orders=0`,
- wallet truth is matched,
- negative-risk policy is explicit,
- order cap is less than or equal to approved canary capital,
- rollback is previewed,
- primary operator signed,
- risk reviewer signed,
- exact deployed revision is known.

`TUNE` applies when:

- the system is controlled but a GO gate is open,
- circuit breaker is tripped from a reviewed/recovered cause but not reset,
- worktree is dirty or release identity is ambiguous,
- legacy inventory is unclassified,
- alerting/reporting is incomplete,
- negative-risk blocking is safe but not yet measured.

`ROLLBACK` applies when:

- heartbeat is actively failing,
- wallet truth is unmatched,
- stale orders appear,
- geoblock/compliance is blocked or unverified while trading is enabled,
- local state shows a fill or position without exchange truth,
- a canary order exceeds approved caps,
- a breaker trips during canary.

## Quant Discipline

The system is not allowed to trade because a signal looks strong. It can trade only when infrastructure, risk, wallet truth, release identity, compliance, and signoff all permit it.

Every candidate action must be reduced to:

- expected upside,
- maximum loss,
- reversibility,
- audit evidence,
- exact owner,
- stop condition.

If any field is unknown, the action remains `TUNE`.

## First Action From Current State

The current safe objective is not live execution. It is to convert the system from dirty, circuit-breaker-tripped `TUNE` into either:

- Stage 0 observer with trading disabled, or
- signed Stage 1 one-dollar canary with circuit breaker reset and max order reduced to `$1.00`.

Do not start Stage 1 until the live go-live TODO definition of done is satisfied.
