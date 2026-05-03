# Blink Live Go-Live TODO

Snapshot date: 2026-05-02 UTC

Scope: move Blink from the current live-observer/canary posture into a controlled Polymarket live launch without bypassing compliance, risk, reconciliation, or operator gates.

This file is intentionally strict. A live launch is allowed only when every hard gate below is either checked off with evidence or explicitly waived by the named operator and risk reviewer.

## 0. Current Verified State

- [x] Engine service is active under `blink-engine.service`.
- [x] Deployed service uses `/etc/blink-engine.env` and `/opt/blink/blink-engine/target/release/engine`.
- [x] Mode is live: `LIVE_TRADING=true`, `PAPER_TRADING=false`.
- [x] Runtime trading switch is currently true in deployed env/API: `TRADING_ENABLED=true`.
- [x] Deployed `--preflight-live` passed all 9 checks on 2026-05-02.
- [x] Polymarket geoblock guard reports eligible from server location: country `FI`, region `18`.
- [x] pUSD balance visible on-chain: `1.672959`.
- [x] CTF Exchange V2 allowance is non-zero/max allowance.
- [x] Signature type is proxy/gasless path: `POLYMARKET_SIGNATURE_TYPE=2`.
- [x] Signer POL is non-zero: about `5.437144433505970787`.
- [x] Funder POL is zero; acceptable for current gasless CLOB path, not for direct on-chain approvals/migrations.
- [x] Live wallet truth is matched: no local-only or external-only position drift reported.
- [x] Engine unit tests pass with incremental disabled: `CARGO_INCREMENTAL=0 cargo test -p engine --lib` => `234 passed`.
- [ ] Current runtime risk status is not GO: `CIRCUIT_BREAKER`.
- [ ] Circuit breaker reason is `heartbeat_dead_5consecutive_failures`.
- [ ] Negative-risk markets are currently blocked by policy: `neg_risk_market_blocked_until_neg_risk_signing_enabled`.
- [ ] Operator signoff and production-readiness artifacts have not been found.
- [ ] Worktree is dirty and must be release-scoped before promotion.

Execution update: safe execution pass completed 2026-05-02T17:58:11Z. Evidence is stored under `logs/go-live/2026-05-02-live-canary-a/`. Production-readiness gate was run with explicit threshold/signoff paths and returned `TUNE` with 6 hard gate failures, 4 soft gate failures, and 5 missing required artifacts. Circuit breaker was not reset.

## 1. Absolute Hard Stops

Do not reset the circuit breaker, increase capital, or call this production live if any item in this section is true.

- [ ] Geoblock guard is blocked or unverified while `TRADING_ENABLED=true`.
- [ ] `--preflight-live` fails.
- [ ] Heartbeat is currently failing or stale.
- [ ] Wallet truth reports local/exchange drift.
- [ ] Any pending order is stale or unreconciled.
- [ ] Circuit breaker is tripped and has not been reviewed.
- [ ] No rollback path has been previewed.
- [ ] No primary operator signoff exists.
- [ ] No independent risk reviewer signoff exists.
- [ ] Build/test status is unknown.
- [ ] The deployed binary cannot be mapped to a reviewed git revision or release artifact.
- [ ] Plaintext secrets are newly introduced into git-tracked files.
- [ ] Operator is located in or routing through a restricted Polymarket jurisdiction.

## 2. Release Identity and Evidence Freeze

Goal: create an auditable launch packet before touching risk switches.

- [x] Choose a run ID.
  - Suggested format: `2026-05-02-live-canary-a`.
- [x] Record branch and commit.
  - Current observed branch: `feat/category-drift-override`.
  - Current observed HEAD: `ecdf575 Harden live wallet truth surfaces`.
- [x] Capture deployed binary metadata.
  - Command: `ls -l /opt/blink/blink-engine/target/release/engine`.
- [x] Capture systemd unit.
  - Command: `systemctl cat blink-engine`.
- [x] Capture env mode fields without secrets.
  - Command: `rg -n "^(TRADING_ENABLED|LIVE_TRADING|PAPER_TRADING|POLYMARKET_SIGNATURE_TYPE|POLYMARKET_FUNDER_ADDRESS|MAX_SINGLE_ORDER_USDC|MAX_DAILY_LOSS_PCT|BLINK_ALLOW_NEG_RISK)=" /etc/blink-engine.env`.
- [x] Capture API snapshots.
  - `curl -sS http://127.0.0.1:3030/api/status`
  - `curl -sS http://127.0.0.1:3030/api/risk`
  - `curl -sS http://127.0.0.1:3030/api/failsafe`
  - `curl -sS http://127.0.0.1:3030/api/live/portfolio`
  - `curl -sS http://127.0.0.1:3030/api/live/executions`
  - `curl -sS http://127.0.0.1:3030/api/geoblock`
- [x] Store the captured output under a run-specific folder.
  - Suggested path: `logs/go-live/2026-05-02-live-canary-a/`.
- [x] Add a short `README.md` in that folder with:
  - launch intent,
  - operator,
  - reviewer,
  - exact UTC timestamps,
  - allowed capital,
  - expected rollback action.

Acceptance criteria:

- [x] A reviewer can reconstruct the runtime state from saved artifacts without relying on terminal scrollback.
- [x] No saved artifact contains raw private keys, API secrets, passphrases, or keystore passphrases.

## 3. Worktree and Release Hygiene

Goal: avoid running an unreviewable mix of local changes, generated UI assets, env files, and benchmark artifacts.

- [x] Review `git status --short`.
- [x] Separate changes into four buckets:
  - engine runtime logic,
  - UI/build artifacts,
  - local secrets/env files,
  - profiling/generated files.
- [ ] Ensure the following remain uncommitted or ignored:
  - `ai-agent/.env`,
  - `blink-engine/.env.live`,
  - `blink-engine/callgrind.out.202975`,
  - transient UI build markers/logs.
- [ ] Decide whether newly generated static UI assets are intended deploy artifacts.
- [ ] Commit or tag the exact code intended for canary.
- [ ] Build the release binary from that reviewed state.
  - Command: `cargo build --release -p engine`.
- [ ] Deploy the binary atomically.
  - Existing pattern: install to `engine.new`, then move into release path.
- [ ] Restart service only after the release artifact is known.

Acceptance criteria:

- [ ] `git diff --stat` contains only intentional unreleased changes or is clean.
- [ ] `git status --short` contains no accidental secrets.
- [ ] Deployed binary timestamp matches the intended build/deploy event.
- [ ] Operator can answer: "What exact code is live?"

## 4. Preflight Gate

Goal: prove that the current deploy can safely reach required dependencies without placing an order.

- [x] Run deployed preflight from `/opt/blink/blink-engine`.
  - Command used: `set -a; . /etc/blink-engine.env; set +a; /opt/blink/blink-engine/target/release/engine --preflight-live`.
- [x] Confirm CLOB credentials validate via `GET /data/orders`.
- [x] Confirm heartbeat endpoint OK.
- [x] Confirm vault can sign a test digest.
- [x] Confirm persistence paths are writable.
- [x] Confirm risk limits are coherent.
- [x] Confirm geoblock is eligible.
- [x] Confirm pUSD balance and allowance.
- [ ] Re-run preflight after any env, binary, network, or capital change.
- [x] Save preflight output to the run evidence folder.

Acceptance criteria:

- [x] Latest preflight output is newer than the latest service restart.
- [x] Latest preflight output is newer than the latest env edit.
- [x] Latest preflight output is attached to signoff.

## 5. Circuit Breaker Recovery Gate

Current blocker: `risk_status=CIRCUIT_BREAKER`, reason `heartbeat_dead_5consecutive_failures`.

Goal: reset only after proving the underlying heartbeat issue has recovered and the reset is intentional.

- [x] Inspect heartbeat trend.
  - Command: `curl -sS http://127.0.0.1:3030/api/failsafe`.
- [x] Confirm:
  - `heartbeat_consecutive_fail_count=0`,
  - `heartbeat_last_ok_ms` is less than 60 seconds old,
  - heartbeat has remained healthy for at least 10 minutes,
  - no new `heartbeat_dead_*` logs during that window.
- [x] Inspect recent service logs.
  - Command: `journalctl -u blink-engine --since "2026-05-02 16:05:36 UTC" --no-pager`.
- [x] Classify the original heartbeat failure:
  - transient Polymarket/API issue,
  - local network issue,
  - DNS/TLS issue,
  - process starvation,
  - service restart race,
  - unknown.
  - Current classification: external heartbeat endpoint/network instability (`GET /time network error`), recovered after 2026-05-02T17:13:24Z.
- [x] If cause is unknown, keep canary capital at `$1` and extend observation window.
- [x] Confirm `WEB_OPERATOR_TOKEN` is configured for protected reset endpoint.
- [ ] Reset via API only after operator and risk reviewer approval.
  - Endpoint: `POST /api/risk/reset_circuit_breaker`.
  - Required header: `x-operator-token: <token>` or `Authorization: Bearer <token>`.
- [ ] Immediately verify reset.
  - `curl -sS http://127.0.0.1:3030/api/status`
  - `curl -sS http://127.0.0.1:3030/api/risk`
- [ ] Record reset timestamp, operator, reviewer, and rationale.

Acceptance criteria:

- [ ] `risk_status` is no longer `CIRCUIT_BREAKER`.
- [ ] `circuit_breaker_tripped=false`.
- [ ] Heartbeat remains healthy for 10 minutes after reset.
- [ ] No orders are submitted during reset unless explicitly approved.

Rollback criteria:

- [ ] Breaker re-trips within 30 minutes.
- [ ] Heartbeat consecutive failures return.
- [ ] Any stale order appears.
- [ ] Wallet truth becomes unmatched.

## 6. Runtime Mode Gate

Goal: make the intended trading mode explicit, not accidental.

- [ ] Decide target mode for the next session:
  - Option A: live observer, `TRADING_ENABLED=false`.
  - Option B: live canary, `TRADING_ENABLED=true`, max order `$1`.
  - Option C: rollback paper mode, `LIVE_TRADING=false`, `PAPER_TRADING=true`.
- [ ] If live canary:
  - set `LIVE_TRADING=true`,
  - set `PAPER_TRADING=false`,
  - set `TRADING_ENABLED=true`,
  - set `MAX_SINGLE_ORDER_USDC=1.00`,
  - set `MAX_DAILY_LOSS_PCT` to the canary threshold,
  - keep `BLINK_ALLOW_NEG_RISK=false` unless negative-risk signing is explicitly approved.
- [x] Confirm `/api/mode` reflects the intended state.
- [x] Confirm `/api/risk` reflects the intended state.
- [ ] Confirm `/api/status` is not `CIRCUIT_BREAKER`.
- [x] Confirm strategy controller remains in `mirror` unless a different strategy is explicitly approved.

Acceptance criteria:

- [x] Runtime API state matches env state.
- [x] Operator can state exactly whether the engine is observer-only or order-capable.
- [ ] Risk reviewer approves the target mode.

## 7. Negative-Risk Market Policy

Current behavior: negative-risk markets are blocked because `BLINK_ALLOW_NEG_RISK=false`.

Goal: decide whether the launch should skip these markets or support them intentionally.

Default policy for canary:

- [x] Keep `BLINK_ALLOW_NEG_RISK=false`.
- [x] Treat `neg_risk_market_blocked_until_neg_risk_signing_enabled` as a protective skip, not a runtime error.

Tasks if keeping blocked:

- [ ] Add a dashboard/report counter for negative-risk blocked signals.
- [ ] Confirm blocked signals do not count as failed order submissions.
- [ ] Confirm blocked signals do not create local positions.
- [ ] Confirm blocked signals do not trip rate/loss breakers.
- [ ] Include blocked count in canary post-run report.

Tasks if enabling later:

- [ ] Verify Polymarket negative-risk signing requirements against official docs/client behavior.
- [ ] Add parity tests for negative-risk payloads/signatures.
- [ ] Add unit tests for metadata classification.
- [ ] Add integration tests against mock CLOB.
- [ ] Add a separate env gate, not only `BLINK_ALLOW_NEG_RISK`.
- [ ] Run a dedicated negative-risk canary with a separate run ID.

Acceptance criteria for current canary:

- [ ] Negative-risk markets remain blocked.
- [ ] Non-negative-risk eligible markets can still be considered.
- [ ] Operator understands that many RN1 signals may be skipped.

## 8. Capital and Wallet Gate

Goal: capital level matches launch intent.

Current state:

- [x] pUSD visible: `1.672959`.
- [x] Current capital supports only very small canary.
- [x] Current open wallet positions are visible and matched.
- [ ] NAV is below current max single order `$2.00`, so order cap must be reduced or capital increased before normal canary.

Canary capital policy:

- [ ] Stage 0: observer only, no orders.
- [ ] Stage 1: max single order `$1.00`, max total new spend `$1.00`.
- [ ] Stage 2: max single order `$1.00`, max total session spend `$5.00`.
- [ ] Stage 3: max single order `$2.00`, only after clean Stage 2.
- [ ] No stage may start if wallet truth is unmatched.
- [ ] No stage may start if existing positions cannot be explained.

Funding tasks:

- [ ] Decide whether to top up pUSD.
- [ ] If topping up, migrate/wrap funds to pUSD through compliant Polymarket flow.
- [ ] Re-run `--preflight-live` after top-up.
- [ ] Confirm pUSD balance through `/api/live/portfolio`.
- [ ] Confirm allowance remains non-zero.
- [ ] Confirm funder and signer addresses are still the intended addresses.

Acceptance criteria:

- [ ] Capital allocated to the run is written in signoff.
- [ ] Order cap is less than or equal to available canary budget.
- [ ] Existing positions are either accepted as starting inventory or closed before launch.

## 9. Existing Position Gate

Current wallet truth reports 3 open positions with current value `0.0` and unrealized PnL around `-3.3599`.

Goal: do not mix legacy inventory ambiguity into launch metrics.

- [x] Export current open positions from `/api/live/portfolio`.
- [ ] Classify each existing position:
  - legacy/manual,
  - bot-created,
  - resolved/expired,
  - still live but illiquid,
  - data issue.
- [ ] Decide handling:
  - keep as accepted starting inventory,
  - close manually,
  - ignore from canary new-trade metrics,
  - investigate data source.
- [ ] Ensure canary metrics distinguish:
  - starting NAV,
  - starting position value,
  - new orders,
  - new fills,
  - realized PnL during canary,
  - open PnL from legacy inventory.

Acceptance criteria:

- [ ] Canary success/failure is not polluted by old positions.
- [ ] `local_only_positions_count=0`.
- [ ] `external_only_positions_count=0`.
- [ ] `reality_status=matched`.

## 10. Order Flow Canary Gate

Goal: allow at most one tiny real order path through, then pause and reconcile.

Preconditions:

- [ ] Preflight passed after latest restart.
- [ ] Circuit breaker reset and stable.
- [ ] `TRADING_ENABLED=true`.
- [ ] `MAX_SINGLE_ORDER_USDC=1.00`.
- [ ] `BLINK_ALLOW_NEG_RISK=false`.
- [ ] Wallet truth matched.
- [ ] Operator is watching logs and API.

Execution:

- [ ] Start canary window.
- [ ] Wait for one eligible non-negative-risk signal.
- [ ] Confirm signal passes metadata gate.
- [ ] Confirm submit starts.
- [ ] Confirm exchange response.
- [ ] Confirm reconciliation result:
  - `Fill`,
  - `NoFill`,
  - `StillPending`,
  - `SuspectedStale`.
- [ ] If fill occurs, confirm local position appears only after exchange truth.
- [ ] If no fill occurs, confirm no local position is created.
- [ ] Stop after first accepted submit or first confirmed fill.
- [ ] Pause or disable trading until post-canary review completes.

Acceptance criteria:

- [ ] `pending_orders=0` after reconciliation window.
- [ ] `stale_orders=0`.
- [ ] `local_only_positions_count=0`.
- [ ] `external_only_positions_count=0`.
- [ ] `reality_status=matched`.
- [ ] No circuit breaker trip.
- [ ] No unexpected auth/geoblock rejection.

Immediate rollback criteria:

- [ ] Exchange rejects due to region/compliance.
- [ ] Local position appears without confirmed exchange truth.
- [ ] Pending order becomes stale.
- [ ] Circuit breaker trips.
- [ ] Heartbeat fails consecutively.
- [ ] Daily loss or VaR breaker trips.
- [ ] Unexpected SELL path is attempted.

## 11. Reconciliation and Truth Gate

Goal: exchange state is the source of truth.

- [ ] Confirm every submit has a request ID or equivalent trace.
- [ ] Confirm every accepted order has a reconciliation lifecycle.
- [ ] Confirm every final state is terminal in local router state.
- [ ] Confirm exchange positions are fetched after canary.
- [ ] Confirm data API activity is deduped.
- [ ] Confirm local portfolio does not count queued orders as NAV.
- [ ] Confirm `/api/live/executions` includes canary execution if a trade occurred.
- [ ] Confirm no unverified websocket-only state is presented as truth.

Acceptance criteria:

- [ ] Reconciliation produces zero drift.
- [ ] Post-run report can explain every dollar of NAV movement.
- [ ] If canary had no fill, NAV should be unchanged except external market movement on legacy positions.

## 12. Observability Gate

Goal: operator can detect trouble fast enough to stop the engine.

- [ ] Verify service logs are readable.
  - `journalctl -u blink-engine -n 100 --no-pager`.
- [ ] Verify session log path.
  - `logs/sessions/engine-session-<date>.log`.
- [ ] Verify failsafe endpoint.
  - `/api/failsafe`.
- [ ] Verify activity endpoint.
  - `/api/activity`.
- [ ] Verify live portfolio endpoint.
  - `/api/live/portfolio`.
- [ ] Verify emergency stop endpoint path is visible.
  - `/api/emergency_stop`.
- [ ] Add alerting channel if not present:
  - Slack,
  - Discord,
  - webhook,
  - systemd/journal alert.
- [ ] Alert on:
  - circuit breaker trip,
  - heartbeat consecutive failures,
  - stale order,
  - auth failure burst,
  - geoblock blocked/unverified,
  - wallet truth mismatch,
  - negative-risk blocked spike,
  - submit rejection spike.

Acceptance criteria:

- [ ] Operator does not need SSH log spelunking to know whether the bot is safe.
- [ ] Emergency stop path is known before starting canary.

## 13. Rollback Gate

Goal: rollback is rehearsed before live risk is accepted.

- [x] Review `deploy/ROLLBACK-PLAYBOOK.md`.
- [ ] Preview rollback helper if using the scripted path.
- [ ] Confirm manual rollback commands for current deployed layout.
- [x] Confirm current systemd unit uses `/etc/blink-engine.env`, while rollback docs also mention `/opt/blink/.env`.
- [x] Decide whether rollback must edit both `/etc/blink-engine.env` and `/opt/blink/.env`.
- [ ] Confirm service restart command:
  - `systemctl restart blink-engine`.
- [ ] Confirm verification command:
  - `systemctl is-active blink-engine`.
- [ ] Confirm API recovers after restart:
  - `curl -sS http://127.0.0.1:3030/api/status`.

Manual rollback target state:

```bash
TRADING_ENABLED=false
LIVE_TRADING=false
PAPER_TRADING=true
ALPHA_TRADING_ENABLED=false
```

Acceptance criteria:

- [ ] Rollback preview is attached to signoff.
- [ ] Operator knows whether the live service reads `/etc/blink-engine.env`, `/opt/blink/.env`, or both.
- [ ] Rollback can be completed in under 2 minutes.

## 14. Operator Signoff Gate

Goal: a human takes explicit responsibility for the launch decision.

- [x] Generate or write signoff record.
  - Suggested path: `deploy/signoffs/2026-05-02-live-canary-a-<utc>.json`.
  - Draft generated at `deploy/signoffs/2026-05-02-live-canary-a-draft.json`; it is intentionally unsigned and `promotion_allowed=false`.
- [ ] Primary operator signs.
- [ ] Secondary risk reviewer signs.
- [ ] Attach preflight output.
- [ ] Attach API snapshots.
- [ ] Attach test output.
- [ ] Attach rollback preview.
- [ ] Attach capital policy.
- [ ] Attach negative-risk policy.
- [ ] Attach circuit breaker reset rationale.
- [ ] Record final decision:
  - `GO`,
  - `TUNE`,
  - `ROLLBACK`.

Required signoff fields:

- [ ] run ID,
- [ ] environment,
- [ ] git commit,
- [ ] deployed binary path,
- [ ] UTC start time,
- [ ] max single order,
- [ ] max session spend,
- [ ] max daily loss,
- [ ] allowed markets policy,
- [ ] negative-risk policy,
- [ ] rollback owner,
- [ ] emergency contact,
- [ ] promotion allowed boolean.

Acceptance criteria:

- [ ] No live canary begins without both signers.
- [ ] A `ROLLBACK` decision immediately disables trading.

## 15. Post-Canary Review Gate

Goal: decide whether to continue, tune, or roll back based on evidence.

- [ ] Freeze post-run API snapshots.
  - `/api/status`
  - `/api/risk`
  - `/api/failsafe`
  - `/api/live/portfolio`
  - `/api/live/executions`
  - `/api/activity`
- [ ] Compare start and end NAV.
- [ ] Compare start and end pUSD cash.
- [ ] Count:
  - signals observed,
  - metadata-blocked signals,
  - negative-risk blocked signals,
  - eligible signals,
  - submit attempts,
  - accepted submits,
  - rejected submits,
  - fills,
  - no-fills,
  - stale orders.
- [ ] Confirm no drift.
- [ ] Confirm no unreviewed legacy inventory changes.
- [ ] Record final post-canary decision.

GO criteria for next stage:

- [ ] No circuit breaker trip.
- [ ] No heartbeat instability.
- [ ] No stale orders.
- [ ] No wallet truth mismatch.
- [ ] No compliance/geoblock rejection.
- [ ] No unclassified order submit.
- [ ] Reconciliation explains all exchange state changes.

TUNE criteria:

- [ ] Negative-risk blocking is too broad but safe.
- [ ] Too few eligible signals.
- [ ] Latency or heartbeat warning without loss of control.
- [ ] Non-critical UI/API reporting issue.

ROLLBACK criteria:

- [ ] Any truth mismatch.
- [ ] Any stale order.
- [ ] Any order accepted outside intended caps.
- [ ] Any local fill without exchange confirmation.
- [ ] Any geoblock/compliance error.
- [ ] Any breaker trip during canary.

## 16. Stage Promotion Plan

Stage 0: observer stabilization

- [ ] `TRADING_ENABLED=false`.
- [ ] 2 hours uptime.
- [ ] Heartbeat stable.
- [ ] Wallet truth matched.
- [ ] Negative-risk blocked count understood.

Stage 1: single-order canary

- [ ] `MAX_SINGLE_ORDER_USDC=1.00`.
- [ ] Max new spend `$1.00`.
- [ ] Stop after first accepted submit or fill.
- [ ] Post-canary review required.

Stage 2: micro session

- [ ] Max single order `$1.00`.
- [ ] Max session spend `$5.00`.
- [ ] At least 3 eligible opportunities or 2-hour window.
- [ ] Zero drift.

Stage 3: small live session

- [ ] Max single order `$2.00`.
- [ ] Max session spend `$10.00`.
- [ ] Alerting active.
- [ ] Rollback drill completed within previous 24 hours.

Stage 4: controlled production

- [ ] Risk owner approves capital increase.
- [ ] Production-readiness gate returns `GO`.
- [ ] Canary history has at least 3 clean sessions.
- [ ] Negative-risk policy either remains blocked or has a separate approved launch packet.

## 17. Engineering Backlog Before Larger Capital

These are not blockers for a one-dollar canary if all gates above pass, but they are blockers for meaningful capital.

- [ ] Add a machine-readable go-live evidence collector script.
- [ ] Remove `jq` dependency from `scripts/pre_pol_check.sh` or install/document it.
- [ ] Make preflight write structured JSON output.
- [ ] Add production-readiness gate invocation to runbook.
- [ ] Add mock CLOB integration tests for:
  - accepted order,
  - rejected order,
  - partial fill,
  - stale pending,
  - heartbeat failure,
  - auth failure,
  - geoblock failure.
- [ ] Add chaos tests for transient 429/5xx.
- [ ] Add dedicated negative-risk signing test suite.
- [ ] Add alert transport.
- [ ] Add durable signoff artifact generation.
- [ ] Add exact release revision to `/api/status`.
- [ ] Add deployed binary build info endpoint.
- [ ] Add "runtime trading switch changed" audit log.
- [ ] Add "circuit breaker reset" audit log with operator identity.
- [ ] Add automatic post-canary report generator.

## 18. Suggested Immediate Next Commands

Read-only status:

```bash
systemctl status blink-engine
curl -sS http://127.0.0.1:3030/api/status
curl -sS http://127.0.0.1:3030/api/risk
curl -sS http://127.0.0.1:3030/api/failsafe
curl -sS http://127.0.0.1:3030/api/live/portfolio
curl -sS http://127.0.0.1:3030/api/geoblock
```

Preflight:

```bash
cd /opt/blink/blink-engine
set -a
. /etc/blink-engine.env
set +a
/opt/blink/blink-engine/target/release/engine --preflight-live
```

Test:

```bash
cd /root/blink_src/blink-engine
CARGO_INCREMENTAL=0 cargo test -p engine --lib
```

Circuit breaker reset shape, only after signoff:

```bash
curl -sS -X POST \
  -H "x-operator-token: <WEB_OPERATOR_TOKEN>" \
  http://127.0.0.1:3030/api/risk/reset_circuit_breaker
```

Emergency stop shape:

```bash
curl -sS -X POST \
  -H "x-operator-token: <WEB_OPERATOR_TOKEN>" \
  http://127.0.0.1:3030/api/emergency_stop
```

## 19. Definition of Done

The system is ready to call "live canary GO" only when:

- [ ] latest preflight passes,
- [ ] circuit breaker is clear,
- [ ] heartbeat is stable,
- [ ] wallet truth is matched,
- [ ] negative-risk policy is explicit,
- [ ] max order and capital match the stage,
- [ ] rollback has been previewed,
- [ ] primary operator has signed,
- [ ] secondary risk reviewer has signed,
- [ ] evidence folder exists,
- [ ] exact deployed revision is known,
- [ ] operator is actively watching the first canary window.

The system is ready to call "production live GO" only when:

- [ ] at least 3 canary sessions are clean,
- [ ] production-readiness gate returns `GO`,
- [ ] alerting is active,
- [ ] rollback drill is recent,
- [ ] capital ramp is approved,
- [ ] every unresolved `TUNE` item has a named owner and deadline.
