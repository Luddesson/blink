# Blink Engine — Live Deployment Readiness Status

**Last Updated**: 2026-04-04  
**Status**: Phase A (Protocol Correctness) — IN PROGRESS  
**Branch**: `claude/trade-bot-live-ready-6zaZf`  
**Owner**: AURA-1 (Systems Architecture)

---

## Executive Summary

Blink Engine has **major foundational infrastructure in place for live Polymarket deployment**, with 70% of Phase A critical fixes completed. The engine is **NOT yet production-ready for meaningful capital**, but is rapidly approaching the gate for controlled Stage 1 canary operations.

**What's working well:**
- Live order fill accounting is now correct (deferred until exchange confirmation)
- RN1 intent classification implemented (skips hedge/flatten by default)
- Reconciliation daemon is active
- Preflight checks for credentials + market sanity
- Canary rollout stages with strict limits
- Risk management framework complete
- Heartbeat + WebSocket resilience

**What still needs completion (Phase A & B):**
- P0-2: Signature type/funder/nonce/expiration config validation
- P0-3: SELL amount precision verification (unit test coverage)
- P0-5: Risk close accounting wiring to `record_close()`
- P0-6: Auth header validation against official SDK
- P1-7: Vault init failure handling (hard fail vs soft degradation)
- Phase B: Canonical live profile + env validator contract

---

## Phase A — Protocol Correctness (Blocker Phase)

### ✅ COMPLETED

| Item | File | Status | Evidence |
|------|------|--------|----------|
| **P0-1**: Live fill accounting — only record after exchange acceptance | `live_engine.rs:391-460` | ✅ FIXED | Order rejection → skip local fill; pending_orders → deferred accounting via reconciliation |
| **P0-4**: Reconciliation daemon spawned | `live_engine.rs:194-201` | ✅ IMPLEMENTED | Worker runs every `reconcile_interval`, calls `run_reconciliation_pass()` |
| **P0-9**: RN1 intent classification | `live_engine.rs:248-272` | ✅ IMPLEMENTED | `classify_signal_intent()` → skip HedgeOrFlatten + Ambiguous by default |
| **Preflight checks** | `main.rs:843-906` | ✅ IMPLEMENTED | 4-step validation: market data, credentials, signature fields, risk config |
| **Canary rollout stages** | `live_engine.rs:85-100, 463-488` | ✅ IMPLEMENTED | Stage 1/2/3 enforcement, max order USDC, session caps, daytime-only gates |
| **Emergency stop** | `main.rs:910-924` | ✅ IMPLEMENTED | Cancels all open orders, writes incident flag |
| **Heartbeat + session keep-alive** | `main.rs:532-553` | ✅ IMPLEMENTED | 29s interval to prevent session expiry |
| **Failsafe metrics + monitoring** | `live_engine.rs:45-74` | ✅ IMPLEMENTED | Trigger count, drift tracking, fill confirmation rate |

### 🔶 IN PROGRESS

| Item | File | Status | Owner | Deadline |
|------|------|--------|-------|----------|
| **P0-2**: Signature type/funder/nonce/expiration config | `config.rs`, `order_signer.rs` | 🔶 PARTIAL | Config fields exist; validator in `validate_live_profile_contract()` | Phase A end |
| **P0-3**: SELL amount precision model + unit tests | `order_signer.rs:compute_amounts()` | 🔶 IN REVIEW | Logic exists; needs comprehensive round-trip tests | Phase A end |
| **P0-5**: Risk close wiring in live path | `live_engine.rs`, `paper_portfolio.rs` | 🔶 DEFERRED | Risk fills recorded; close accounting needs explicit wire | Phase A.3 |
| **P0-6**: Auth headers vs official SDK | `order_executor.rs` | 🔶 DEFERRED | Custom HMAC-SHA256 implementation; validate against `rs-clob-client` | Phase B.1 |

### 🔴 NOT STARTED (Non-blocking)

| Item | File | Status | Notes |
|------|------|--------|-------|
| **P1-7**: Vault init hard-fail on LIVE_TRADING=true | `live_engine.rs:105-180` | 🔴 DEFER | Currently degrades gracefully; should fatal-error instead |
| **P1-10**: Failsafe SLO dashboard + runbook | Observability | 🔴 DEFER | Metrics collected; need dedicated alert rules + incident handler |
| **P1-11**: Canonical live profile document | Docs | 🔴 DEFER | Template exists (.env.live.template); needs reference spec |

---

## Phase B — Operational Hardening

### 📋 Pre-requisites (depends on Phase A.x completion)

- [ ] All P0 items must be fixed + tested before Phase B gates
- [ ] Integration test suite with mock Polymarket CLOB passes
- [ ] Paper + shadow-live soak test runs 24+ hours without reconciliation drift

### 🎯 Planned Work (Post-Phase A)

1. **`--preflight-live` enhanced validation**
   - Verify creds parse + can hit `/auth/ok`
   - Check signature type ↔ funder consistency
   - Confirm USDC.e balance + allowance on Polygon
   - Validate market tick-size alignment

2. **Heartbeat watchdog + session expiry handling**
   - Auto-reconnect if heartbeat misses 2 consecutive intervals
   - Reconcile order state after reconnect
   - Alert if heartbeat loop stalls

3. **Failsafe SLO metrics + escalation**
   - Count and track cancel latencies
   - Monitor retry exhaustion
   - Alert if fill confirmation rate drops below 95%

4. **Canonical live profile contract**
   - Frozen list of required + optional env vars
   - Machine-readable validator
   - Documentation alignment

5. **Incident playbook automation**
   - Single `--pause` command to:
     - Stop accepting new signals
     - Cancel open orders
     - Reconcile state
     - Emit incident summary
   - Controlled resume gate (manual operator sign-off)

---

## Phase C — Capital Ramp

### Stage 1 (Conservative Canary)
- **Max per-order**: $5 USDC
- **Max per-session**: 20 orders
- **Time window**: 08:00–22:00 UTC (daytime only)
- **Max rejection streak**: 3 consecutive rejects → auto-halt
- **Market allowlist**: Optional (recommended to restrict to 1–2 proven markets)
- **Duration**: Run until 5 clean sessions with zero reconciliation drift

### Stage 2 (Medium)
- **Max per-order**: $20 USDC
- **Max per-session**: 100 orders
- **Time window**: 24/7
- **Max rejection streak**: 5
- **Automatic breaker drill**: Weekly test of daily loss circuit breaker
- **Duration**: 2+ weeks with zero reconciliation drift + clean breaker tests

### Stage 3 (Full Target)
- **Max per-order**: $500 USDC (configurable)
- **Max per-session**: Unlimited
- **Time window**: 24/7
- **Max rejection streak**: 10
- **Only after**: Risk committee sign-off + operator certification

---

## Infrastructure & Deployment Readiness

### Docker Compose Setup ✅
```bash
# Paper mode (safe default):
docker compose up -d

# Live mode (requires /etc/blink-engine.env):
docker compose --profile live up -d
```

**Status**: ✅ Complete
- `docker-compose.yml` configured for both modes
- ClickHouse warehouse included
- Environment file isolation proper (/etc for live)

### Systemd Service ✅

**File**: `infra/blink-engine.service`

**Features**:
- NUMA pinning (cores 0–7)
- CPU affinity + socket buffer tuning
- File descriptor limits (1M)
- Resource limits (memory, locks, core dumps)
- Security hardening (ProtectSystem=strict, NoNewPrivileges=true)
- Journal logging

**Status**: ✅ Ready to install

### OS Tuning Script ✅

**File**: `infra/os_tune.sh`

**Features**:
- Disable SMT (hyperthreading)
- Disable CPU C-states (wake-up latency)
- CPU frequency locked to max (performance governor)
- NUMA + NIC IRQ affinity
- Huge pages (1024 × 2MB)
- TCP stack optimization
- Network busy-poll tuning

**Status**: ✅ Ready to run (requires root)

### Provisioning Script ✅

**File**: `infra/provision.sh`

**Features**:
- Ubuntu 22.04 LTS base setup
- Rust toolchain (stable)
- ClickHouse installation + systemd
- Foundry tools (forge/cast/anvil)
- Repository clone + release build
- Service unit installation
- Environment file template creation

**Status**: ✅ Ready to run (requires root)

### Configuration Template ✅

**File**: `.env.live.template`

**Features**:
- All required fields documented
- Placeholders for credentials
- Stage 1 canary defaults pre-filled
- Risk limits specified
- Preflight instructions included

**Status**: ✅ Complete

---

## Pre-Live Checklist (Must-Pass Gates)

### Credentials & Wallet
- [ ] `SIGNER_PRIVATE_KEY` (64 hex chars)
- [ ] `POLYMARKET_FUNDER_ADDRESS` (0x-address)
- [ ] `POLYMARKET_API_KEY` (from createOrDeriveApiKey)
- [ ] `POLYMARKET_API_SECRET` (base64)
- [ ] `POLYMARKET_API_PASSPHRASE`
- [ ] `POLYMARKET_SIGNATURE_TYPE` (0=EOA, 1=POLY_PROXY, 2=GNOSIS_SAFE)
- [ ] USDC.e funded on Polygon (≥ Stage 1 capital + 10% buffer)
- [ ] Allowance to CLOB contract verified via etherscan

### System Setup
- [ ] OS tuning script run (`sudo ./infra/os_tune.sh`)
- [ ] GRUB options added + reboot completed
- [ ] ClickHouse running + healthy
- [ ] Systemd service installed + enabled
- [ ] NVMe mounted (if using local cache)

### Engine Validation
- [ ] Release binary built (`cargo build --release`)
- [ ] All tests pass (`cargo test`)
- [ ] Preflight check passes: `cargo run --release -p engine -- --preflight-live`
  - Market data reachable
  - Auth credentials valid
  - Signature fields correct
  - Risk config non-zero
- [ ] TUI or logs show "ALL PREFLIGHT CHECKS PASSED"

### Monitoring & Observability
- [ ] ClickHouse schema created + data flowing
- [ ] Log files are writable to `logs/sessions/`
- [ ] Post-run review script functional
- [ ] Alerting system prepared (Slack/Discord/email/webhook)

### Operational Safety
- [ ] RiskManager breakers tested in paper mode
- [ ] Kill-switch tested: `--emergency-stop`
- [ ] Runbook document prepared (incident playbook)
- [ ] Operator certified on control procedures
- [ ] Backup plan if Polymarket CLOB goes down

### Live-Specific Config
- [ ] `LIVE_TRADING=true` (only on prod server)
- [ ] `PAPER_TRADING=false`
- [ ] `TRADING_ENABLED=true`
- [ ] `BLINK_LIVE_PROFILE=canonical-v1` (immutable)
- [ ] `LIVE_ROLLOUT_STAGE=1` (start conservative)
- [ ] `LIVE_CANARY_MAX_ORDER_USDC=5.0` (Stage 1)
- [ ] `LIVE_CANARY_MAX_ORDERS_PER_SESSION=20` (Stage 1)
- [ ] `LIVE_CANARY_DAYTIME_ONLY=true` (Stage 1)
- [ ] `LIVE_CANARY_START_HOUR_UTC=8`
- [ ] `LIVE_CANARY_END_HOUR_UTC=22`
- [ ] `MAX_DAILY_LOSS_PCT=0.10` (circuit breaker)
- [ ] `MAX_SINGLE_ORDER_USDC=5.0` (matches CANARY limit)
- [ ] `CLICKHOUSE_URL=http://localhost:8123` (or production ClickHouse)

---

## Known Limitations & Future Work

### Deferred to Phase B+
1. **auth header validation**: Currently uses custom HMAC; should validate against `rs-clob-client` reference
2. **Vault hard-fail on LIVE_TRADING=true**: Currently degrades to dry-run if vault unavailable
3. **Failsafe SLO dashboard**: Metrics collected; needs dedicated alerting rules
4. **Dual-engine audit mode**: Advanced feature for future (independent reconciler)

### Nice-to-Have (Post-Live Stabilization)
1. io_uring optimization (currently Tokio-based)
2. MEV router integration (currently routing placeholder)
3. Dual-wallet redundancy
4. Advanced hedge detection via ML signal filtering

---

## Immediate Next Steps (Priority Queue)

### This Week
1. **Complete P0-3**: SELL amount precision tests
   - Round-trip conversion tests for fractional sizes
   - Unit test coverage for edge cases (1 wei, MAX_INT, etc)
2. **Validate P0-2**: Signature type/funder config behavior
   - Run against mock CLOB with different signature types
   - Document compatibility matrix
3. **Test reconciliation daemon**: Run 24h paper mode + verify no drift

### Next Week
1. **Integrate auth validation** against official SDK or reference
2. **Soak test**: Run `--preflight-live` + 8h controlled shadow-live
3. **Prepare Phase B**: Write enhanced preflight + heartbeat logic

### Before Capital Deploy
1. **Risk committee review** of masterplan + checklist
2. **Incident runbook** written + operator trained
3. **Stage 1 approval** from technical lead + CTO

---

## Testing Summary

### Unit Tests
- ✅ Config loading + validation
- ✅ Risk manager checks + breaker logic
- ✅ Order book state machine
- ✅ RN1 intent classifier (open/add/hedge/close)
- 🔶 Amount conversion (SELL precision — in progress)

### Integration Tests
- ✅ Paper mode full cycle (signal → fill → close)
- ✅ Canary gate enforcement
- 🔶 Mock Polymarket CLOB (in progress)
- 🔴 Live soak test (deferred to Phase B)

### Manual Validation
- ✅ Preflight checks all 4 gates
- ✅ Heartbeat keep-alive functional
- ✅ Reconciliation worker active
- ✅ Risk manager halts on breaker
- ✅ Emergency stop cancels orders

---

## Contact & Escalation

**Technical Lead**: AURA-1  
**On-Call**: TBD (to be assigned before Stage 1)  
**Escalation**: Risk committee + CTO

---

## Revision History

| Date | Version | Change |
|------|---------|--------|
| 2026-04-04 | 1.0 | Initial live readiness assessment. Phase A 70% complete. Phase B planned. |

---
