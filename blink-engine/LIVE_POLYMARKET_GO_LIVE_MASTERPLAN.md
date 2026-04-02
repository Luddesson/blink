# Blink x Polymarket Live Go-Live Masterplan

Status: **Pre-Production / Not Live-Ready Yet**

Owner: Blink core team

Scope: End-to-end requirements to run Blink live on Polymarket CLOB with real funds, including code findings, external constraints, operations, and release gates.

---

## 1) Executive Summary

Blink has a strong paper/runtime foundation (TUI, risk manager, logging, graceful shutdown, post-run analysis), but based on current code review + Polymarket CLOB requirements, it is **not yet safe to run with meaningful real capital**.

Main reason: there are **P0 correctness gaps in live order flow and settlement reconciliation** that can produce false state, wrong accounting, and silent divergence between engine and exchange.

Recommended launch strategy:

1. **Phase A (P0 fixes)**: protocol correctness + live state integrity
2. **Phase B (P1 hardening)**: reliability, observability, runbooks
3. **Phase C (capital ramp)**: progressive notional rollout with strict kill-switch governance

---

## 2) External Requirements (Polymarket CLOB)

From Polymarket docs:

- CLOB auth is two-level:
  - L1: EIP-712 for credential creation/derivation
  - L2: HMAC-SHA256 for trading endpoints
- Trading requests require strict auth headers and valid signature construction.
- Signature type + funder model must match wallet type (EOA / POLY_PROXY / GNOSIS_SAFE).
- Orders must respect tick size, allowance, balance, and order validity checks.
- Heartbeat endpoint is required to keep sessions alive; stale heartbeat can cancel open orders.
- Official clients exist (TS/Python/Rust) and are recommended for production-grade compatibility.

Sources used during this review:

- https://docs.polymarket.com/developers/CLOB/introduction
- https://docs.polymarket.com/developers/CLOB/authentication
- https://docs.polymarket.com/developers/CLOB/orders/orders
- https://docs.polymarket.com/developers/CLOB/clients

Archive references additionally reviewed:

- `_ARCHIVE\POLYMARKET_API_REFERENCE`
- `_ARCHIVE\SECURITY_AND_RISK_GUIDE`
- `_ARCHIVE\MASTER_ROADMAP`
- `_ARCHIVE\engine_docs\architecture.md`
- `_ARCHIVE\engine_docs\configuration.md`
- `_ARCHIVE\engine_docs\trading-modes.md`
- `_ARCHIVE\root_docs\analysis\FINAL_RECOMMENDATIONS.md`
- `_ARCHIVE\root_docs\analysis\EXECUTIVE_SUMMARY_RN1.md`

---

## 3) Codebase Verification Findings (Critical)

### P0-1: Live engine records fills even when exchange order fails

Evidence:

- `crates\engine\src\live_engine.rs` (handle_signal)
- After submit attempt, code always runs:
  - `p.open_position(...)`
  - `risk.record_fill(size_usdc)`

Impact:

- Internal portfolio can show a filled position even if exchange rejected order.
- Risk and exposure metrics become wrong.
- TUI and logs can look profitable/active while real account did not fill.

Required fix:

- Only create position + `record_fill` after confirmed accepted order state.
- Add explicit branching for `resp.success == false` and network failures.

---

### P0-2: Signature mode/funder handling is hardcoded and incompatible with common proxy flows

Evidence:

- `crates\engine\src\order_signer.rs`
  - `signature_type: 0`
  - `nonce: 0`
  - `expiration: 0`
- Polymarket docs require correct signature type (EOA/Proxy/Gnosis Safe) + funder semantics.

Impact:

- High rejection risk for accounts using proxy/safe model (most common in production).
- Inability to use proper order type behaviors requiring expiration/nonces.

Required fix:

- Add explicit env/config fields:
  - `POLY_SIGNATURE_TYPE`
  - `POLY_FUNDER_ADDRESS`
  - nonce strategy
  - expiration policy (GTC/GTD)
- Validate combinations at startup.

---

### P0-3: Order amount conversion has SELL precision/correctness risk

Evidence:

- `crates\engine\src\order_signer.rs` in `compute_amounts()`
  - SELL path: `maker_amount = params.size as u64`

Impact:

- Potential truncation and unit mismatch for non-integer share sizes.
- Can create under-sized or invalid sell orders.

Required fix:

- Define canonical unit model for SELL (shares base units) and convert deterministically (no lossy cast).
- Add unit tests for fractional share scenarios and round-trip consistency.

---

### P0-4: No robust live order lifecycle reconciliation

Evidence:

- `OrderExecutor` supports `get_order_status`, but `LiveEngine` does not run a reconciliation loop.
- No active management for partial fills, stale GTCs, cancel/replace logic.

Impact:

- Engine state drifts from exchange state.
- Inventory/risk can become stale and dangerous.

Required fix:

- Add order lifecycle daemon:
  - subscribe or poll open orders/trades
  - reconcile to local state
  - update realized/unrealized correctly
  - handle retry/cancel/replace.

---

### P0-5: Risk close accounting incomplete in live path

Evidence:

- `RiskManager` has `record_close()`.
- No clear live close path wiring to call it in `LiveEngine`.

Impact:

- Daily PnL and breaker logic can be inaccurate over session.

Required fix:

- Wire `record_close(realized_pnl)` on every confirmed close/settlement event.
- Add invariant tests: `daily_pnl = sum(closes) - sum(fills)` (by day window).

---

### P0-6: Header/auth implementation should be validated against official SDK behavior

Evidence:

- `order_executor.rs` builds custom headers manually (`POLY-API-KEY`, `POLY-PASSPHRASE`, etc).
- Docs define specific names and semantics; subtle mismatch can break auth.

Impact:

- Intermittent auth failures and brittle compatibility with backend updates.

Required fix:

- Replace custom auth path with official Rust SDK (`rs-clob-client`) or byte-for-byte parity tests against reference implementation.

---

### P1-7: Live mode can silently degrade to effectively non-trading if vault init fails

Evidence:

- `LiveEngine::new` can set `vault=None` on init error.
- Live handling branch treats `vault.is_none()` similarly to dry-run behavior path.

Impact:

- Operator may think bot is live while execution is effectively disabled.

Required fix:

- If `LIVE_TRADING=true` and vault init fails -> **hard fail startup**.
- Emit explicit fatal error and exit.

---

### P1-8: README/env defaults are not fully consistent

Evidence:

- Documentation mentions max order defaults differing from code defaults in some sections.

Impact:

- Misconfigured production rollout due to wrong assumptions.

Required fix:

- Align all defaults between:
  - `RiskConfig::default`
  - `.env.example`
  - README tables

---

## 3.1) Archive-derived strategic findings now integrated

The archive materially reinforced and extended this go-live plan in five areas:

1) **RN1 behavior model**
- Archive analysis indicates RN1 behaves more like high-volume market-maker/arbitrage flow than a pure directional bettor.
- Consequence for live: blind mirroring is structurally unsafe; selective filters + hedge-awareness are mandatory.

2) **Hedge/close ambiguity**
- Archive highlights synthetic hedging patterns (opposite-side entries in same market).
- Consequence for live: Blink needs explicit trade-intent classification (open/add/hedge/close) before execution.

3) **Sports in-play hazard**
- Archive’s 3-second in-play/failsafe emphasis aligns with current drift logic but requires production-grade guarantee paths and abort observability.
- Consequence for live: treat failsafe path as SLO-critical, not optional.

4) **Ops/security posture**
- Archive’s key-isolation and circuit-breaker stance matches this plan’s safety-first approach.
- Consequence for live: vault failure and auth incompatibility must be fatal startup blockers, not degradations.

5) **Execution reality vs roadmap ambition**
- Archive roadmap includes aggressive HFT goals (ultra-low-latency infra, MEV routing), while present code has protocol correctness blockers.
- Consequence for live: sequence must be corrected: protocol correctness first, latency optimization second.

---

## 3.2) New blockers added from archive synthesis

### P0-9: Missing RN1 intent classification (open vs hedge vs close)

Evidence:
- Archive analyses emphasize synthetic hedge behavior and warn against naive copy.
- Current live flow processes signal-side directly without intent classifier.

Impact:
- Bot can mirror RN1 hedge/flatten actions as fresh exposure.
- This can invert expected edge and inflate drawdown.

Required fix:
- Add RN1 intent classifier with market-level state:
  - `new_exposure`, `add_exposure`, `hedge_or_flatten`, `noise/trash`.
- Default-safe behavior: skip ambiguous intent.

---

### P1-10: Failsafe/abort path lacks production-grade SLO and runbook wiring

Evidence:
- Current fill-window checks exist, but no explicit “failsafe SLO” telemetry and escalation ladder.
- Archive risk guide treats this as the primary protection layer.

Impact:
- Silent degradation during volatile sports windows.
- Operator cannot quickly distinguish “normal rejects” from failsafe stress.

Required fix:
- Add dedicated failsafe metrics:
  - trigger count, cancel latency, cancel confirmation success, retries exhausted.
- Add incident playbook command path:
  - pause -> cancel-all -> reconcile -> resume gate.

---

### P1-11: Documentation/operating semantics drift around live safety

Evidence:
- Archive and current docs differ in assumptions around defaults, risk limits, and mode semantics.

Impact:
- Dangerous operator misunderstandings during launch.

Required fix:
- Publish one canonical “live profile” document and machine-checked env validator.
- Enforce `--preflight-live` before accepting live mode.

---

## 4) Production Go-Live Architecture Requirements

## 4.1 Trading correctness

- Deterministic sizing and rounding spec (BUY/SELL, maker/taker amounts).
- Tick-size validation before submit.
- Pre-submit balance+allowance checks for relevant asset.
- Explicit order state machine:
  - Created -> Submitted -> Live/Matched/Delayed -> Confirmed/Failed -> Settled.

## 4.2 Risk + control plane

- Hard kill switch (already present) with dual control:
  - runtime toggle + env baseline
- Daily loss breaker + VaR breaker + rate limiter (already present, needs close wiring).
- Max per-market exposure + correlated exposure limit.
- Separate “operator pause” vs “fatal pause”.

## 4.3 Key security

- Prefer hardware-backed signing / isolated vault.
- Zero plaintext private keys in process logs.
- Startup fails if live keys invalid or missing.
- Rotation plan for API creds and signer keys.

## 4.4 Observability

- Structured logs for:
  - submit request id
  - exchange response id
  - local position id
  - reconciliation delta
- Metrics:
  - submit success rate
  - reject code breakdown
  - auth failure rate
  - stale-order count
  - local-vs-exchange position drift
- Alerts:
  - breaker trip
  - repeated auth errors
  - divergence > threshold

## 4.5 Operational safety

- Heartbeat watchdog and recovery.
- Startup preflight (credentials, funder, allowances, market sanity).
- Incident runbook:
  - pause
  - cancel open orders
  - reconcile
  - rotate creds if needed
  - controlled resume.

---

## 5) Go-Live Phased Plan (Decisive Execution)

## Phase A — Protocol & State Integrity (Blocker Phase)

Exit criteria: all P0 findings fixed and tested.

Work:

- Fix live fill bookkeeping (no local fill without exchange acceptance).
- Add robust order reconciliation worker.
- Implement configurable signature type/funder/nonce/expiration.
- Correct SELL amount precision model.
- Validate/replace custom auth with official SDK parity.
- Wire `record_close` on confirmed closes.
- Implement RN1 intent classifier (open/add/hedge/close) with default-safe skip.

Tests required:

- Unit: amount conversion, header/signature generation, state transitions.
- Integration (mock CLOB): submit accepted/rejected/partial scenarios.
- Chaos: transient 429/5xx/auth errors.

## Phase B — Operational Hardening

Exit criteria: stable soak in paper+shadow-live environments.

Work:

- Preflight command (`--preflight-live`) checks:
  - creds parse
  - signature type/funder consistency
  - USDC.e balance/allowance
  - market tick-size + live status
- Heartbeat loop + expiry handling.
- Failsafe SLO metrics + alert rules (cancel latency, retry exhaustion, confirmation rate).
- Better incident hooks (single command to pause/cancel/reconcile).
- Alerting channels (Slack/Discord/Webhook).
- Canonical “live profile” and env validation contract.

## Phase C — Capital Ramp

Exit criteria: risk committee/operator sign-off.

Rollout:

- Stage 1: very low notional, limited markets, daytime only.
- Stage 2: medium notional with automatic breaker drill tests.
- Stage 3: full target notional after N clean sessions with zero reconciliation drift.

Hard policy:

- Any unresolved drift or unexplained reject spike => automatic rollback to paper/read-only.

---

## 6) Pre-Live Checklist (Must Pass 100%)

- [ ] `LIVE_TRADING=true` and `TRADING_ENABLED=true` only in controlled environment.
- [ ] Signature type configured correctly for account model.
- [ ] Funder address verified against Polymarket settings.
- [ ] USDC.e (correct token) funded on Polygon.
- [ ] Allowance to exchange contract confirmed.
- [ ] Auth headers/signatures validated against official SDK/reference.
- [ ] Heartbeat confirmed and monitored.
- [ ] Reconciliation loop active and zero drift in dry rehearsal.
- [ ] RN1 intent classifier enabled and hedge/close ambiguity handling tested.
- [ ] Failsafe SLO dashboard green under volatility replay.
- [ ] Circuit breaker + kill-switch tested live with canary notional.
- [ ] Post-run report includes live-specific execution drift and reject taxonomy.

---

## 7) Recommended Design Upgrades (Outside-the-Box)

- Dual-engine mode:
  - Engine A = execution
  - Engine B = independent auditor/reconciler
  - Auto-halt when A/B disagree beyond threshold.

- “Truth source hierarchy”:
  1. Exchange order/trade state
  2. Local event log
  3. Portfolio snapshots

- Replay simulator from live logs:
  - deterministic replay to reproduce incidents exactly.

- Risk budget by regime:
  - low-liquidity hours auto-reduce exposure and widen guardrails.

---

## 8) Immediate Next Actions (Priority Queue)

1. Implement P0-1, P0-2, P0-3 in one branch and lock with tests.
2. Add reconciliation daemon and close-accounting wiring.
3. Build `--preflight-live` command.
4. Validate against official Polymarket client behavior (or adopt `rs-clob-client` directly).
5. Run controlled canary session with minimal capital and full telemetry.

---

## 9) Bottom Line

Blink can become a serious live Polymarket engine, but **today it should be treated as pre-production for live funds** until P0 gaps are closed.

If this masterplan is executed with discipline, Blink can ship a reliable live system with strong safety posture and become a long-term platform, not just a bot.

---

## 10) Execution Control Protocol (Models, Sub-Agents, Request Budget)

Goal: execute without overloading APIs, internet sources, or runtime systems; run strictly phase-by-phase.

### 10.1 Sub-agent role model (manager pattern)

- **Coordinator (main agent)**  
  Owns phase gating, acceptance criteria, and rollback decisions.

- **Explore agent**  
  Read-only discovery, code path mapping, docs extraction.

- **Task agent**  
  Build/test/check execution and command-heavy validation.

- **General-purpose agent**  
  Multi-file implementation for approved phase scope only.

- **Code-review agent**  
  High-signal defect sweep before phase close.

Rule: no parallel code-changing agents in same files. Exploration can run in parallel; mutation cannot.

### 10.2 Model routing strategy

- Use **fast model** for:
  - search/indexing
  - file inventory
  - baseline command checks
- Use **standard model** for:
  - architecture and risk decisions
  - code edits
  - release gate sign-off
- Use **premium model** only for:
  - final pre-live adversarial review
  - critical security/risk reasoning when ambiguity remains

### 10.3 API & internet throttle policy (anti-overload)

- Global outbound rate budget:
  - max 1–2 internet/documentation calls per 10s window
  - burst cap: 5 requests, then cooldown window
- Retry policy:
  - exponential backoff with jitter
  - hard cap on retries
- Request dedupe:
  - never re-fetch same URL/file in same phase unless content changed
- Caching:
  - cache external docs snapshot per phase
  - prefer local/archive docs first, internet second

### 10.4 CLOB/runtime safety budget

- No high-frequency polling unless phase explicitly requires it.
- During validation:
  - prefer dry/paper mode first
  - shadow-live second
  - live canary last
- Guardrails:
  - strict request-per-second caps
  - max concurrent network operations per phase
  - forced cooldown between stress runs

### 10.5 Phase-by-phase execution contract (strict)

Each phase follows:

1. **Plan lock**
   - scope, files, tests, acceptance criteria frozen.
2. **Implement**
   - only phase-approved changes.
3. **Verify**
   - compile/tests + targeted behavior checks.
4. **Review**
   - independent review pass (code-review agent).
5. **Gate decision**
   - pass -> next phase
   - fail -> rollback/fix loop inside same phase

No jumping across phases.

### 10.6 Rollback and overload protection

- Auto-stop triggers:
  - repeated auth/429 bursts
  - reconciliation drift above threshold
  - failsafe SLO degradation
- On trigger:
  - pause trading
  - cancel/open-order hygiene
  - reconcile state
  - freeze phase and produce incident note

### 10.7 Suggested execution order from here

- **Phase A.1**: live fill accounting + risk close wiring
- **Phase A.2**: signature/funder/nonce/expiration correctness
- **Phase A.3**: reconciliation daemon + intent classifier
- **Phase B.1**: preflight-live + heartbeat/failsafe SLO
- **Phase B.2**: canonical live profile + env validator
- **Phase C**: canary rollout stages

---

