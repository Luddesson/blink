# Blink Project Context

**Project:** Blink — High-frequency shadow-maker bot for Polymarket CLOB  
**Tech Stack:** Rust (backend) + TypeScript/React (frontend)  
**Status:** Active development (v0.2.0)

---

## Architecture Overview

### 1. Backend (Rust)
**Location:** `blink-engine/` — Cargo workspace with 4 crates

#### Core Components
- **engine** (main) — High-frequency trading bot
  - WebSocket connections to Polymarket CLOB
  - Real-time order management & execution
  - Market data streaming
  
- **market-scanner** — Market surveillance & data ingestion
  - Continuous market state updates
  - Event processing pipeline
  
- **tee-vault** — Secure credential/key management
  - Encryption & cryptographic operations (k256, sha3, hmac)
  - Zeroization of sensitive data
  
- **bpf-probes** — eBPF instrumentation (Linux production only)
  - Performance monitoring
  - System-level event tracing

#### Key Dependencies
- **Async runtime:** Tokio (full features)
- **Networking:** tokio-tungstenite (WebSockets), Axum (HTTP), tower-http
- **Data:** simd-json, ClickHouse client (analytics/persistence)
- **Observability:** tracing + tracing-subscriber (JSON structured logs)
- **Security:** k256, sha3, hmac, sha2, base64, zeroize

#### Build Profile
- **Release:** LTO (fat), 1 codegen unit, symbol stripping (minimal binary)
- **Dev:** Opt-level 1 (faster iteration)

### 2. Frontend (TypeScript/React)
**Location:** `blink-ui/`

- **Framework:** React 19.2.4 + Vite
- **Styling:** Tailwind CSS 4.2.2
- **Charting:** Recharts 3.8.1
- **TypeScript:** ~5.9.3 (strict)
- **Linting:** ESLint 9.39.4 + TS plugin
- **Build output:** Optimized Vite bundles

#### Running Locally
```bash
cd blink-ui
npm run dev      # Vite dev server (hot reload)
npm run build    # TypeScript + Vite production build
npm run lint     # Check code quality
```

### 3. Agent System
**Location:** `Blink-agents/` — 6 trading/analytics agents

- **aura-architect** — Strategic analysis
- **meridian-api-sync** — API integrity, reconciliation, visual truth, and low-latency sync
- **nexus-automator** — Automated task execution
- **qsigma-quant** — Quantitative analysis
- **sentinel-risk** — Risk monitoring & alerts
- **wraith-stealth** — Stealth/optimization strategies

Each agent has its own TODO tracking.

### 4. Skills System
**Location:** `awesome-agent-skills/` + `.agents/skills/` + `skills/`

Currently active skills:
- **blink-trader** — Blink engine control and portfolio/market/order operations
- **bullpen-polymarket** — Bullpen CLI access for Polymarket data, positions, and execution
- **evm-swiss-knife** — EVM utility functions
- **market-sentiment** — Market sentiment analysis
- **trading-strategist** — Strategy recommendations

---

## Development Patterns

### Code Organization
- **Rust:** Modular crate structure; use workspace dependencies to avoid version drift
- **TypeScript:** Separate src, types, and configs; ESLint enforces consistency
- **Git:** Atomic commits with clear messages (feat/, fix/, chore/, etc.)

### Logging & Observability
- **Rust:** Use `tracing::info!`, `tracing::warn!`, `tracing::error!`
- **Log format:** JSON (structured) for production, human-readable for dev
- **Environment:** Check `.env` file for `RUST_LOG` level

### Testing
- **Rust:** proptest for property-based testing
- **TypeScript:** (check vite.config.ts for test setup)

### Security
- **Credentials:** `.env` files (git-ignored) — never commit secrets
- **Keys/PEM files:** Always git-ignored; use tee-vault for storage
- **Sensitive data:** Use `zeroize::Zeroize` trait in Rust

---

## Common Tasks

### Run the Backend
```bash
cd blink-engine
cargo build --release
cargo run --bin engine
```

### Run the Frontend
```bash
cd blink-ui
npm run dev
```

### Check for Issues
```bash
cargo clippy --all       # Linting
npm run lint             # TypeScript linting
```

### Add Dependencies
- **Rust:** Use workspace dependencies (edit `Cargo.toml` root → `[workspace.dependencies]`)
- **Node:** `npm install <package>` → update `package.json`

---

## Key Files & Decisions

| What | Where | Notes |
|------|-------|-------|
| Env vars | `.env` (git-ignored) | Tokio features, log level, API keys |
| Dependencies | `Cargo.toml` + `package.json` | Workspace-managed in Rust |
| Git ignore | `.gitignore` | Secrets, build artifacts, OS files |
| Logs | `blink-engine/logs/` | `engine.pid`, `vite.pid` (clean if stuck) |

---

## Optimization Tips for Claude Context

1. **Use `.env` wisely** — Store project-specific config there, not in code
2. **Commit messages** — Use conventional commits (feat, fix, chore); helps me understand changes without reading full diffs
3. **Memory system** — I'll save key architectural decisions & constraints to `C:\Users\Zephyrus g14\.claude\projects\<project>/memory/`
4. **Focused requests** — "Fix WebSocket reconnection in market-scanner" vs. "improve reliability"
5. **Code location** — Reference files as `path/file.rs:line_number` for quick navigation

---

## Status & Constraints

- **Current version:** 0.2.0
- **Main branch:** `master` (production code)
- **Build issues:** Watch `.env` (missing credentials → build failure)
- **Dependencies:** Workspace-managed to minimize version conflicts
- **Platform:** Linux + macOS production; Windows/macOS for dev (no io_uring on those platforms)

---

## Session Handoff — 2026-04-13

### What Was Done This Session (12 commits)

#### 1. Profitability Overhaul (5 commits: 397af8a → c203347)
Complete 5-phase overhaul based on live paper trading data:
- Phase 1: Max order cap $8, exit slippage 10bps, momentum 150bps, stale 60s
- Phase 2: Graduated drawdown sizing, partial momentum exit 50%
- Phase 3: Sharpe/Sortino/fee-drag metrics API endpoints
- Phase 4: Simplified sizing (7→3 multipliers), fee-edge precheck, depth gate
- Phase 5: Confidence floor 0.55, realism mode, entry spread cost

#### 2. UI Bug Fixes (fe7bc77)
- Fixed "Closes in 2682h" (used event_end_time → event_start_time)
- Fixed Polymarket 404 links (market_slug → events[0].slug)

#### 3. Infrastructure Hardening (f550c57)
- Heartbeat circuit breaker (3 consecutive failures → trip)
- Persistent nonce storage (data/live_nonce.json)
- Pending orders WAL (data/pending_orders.json)
- Graceful shutdown (reconcile → cancel → persist)
- Daily risk reset at UTC midnight

#### 4. Zero-Fills Fix (ac6113b)
Fixed 5 stacked blockers that prevented paper engine from filling:
- TRADING_ENABLED=true, VAR_THRESHOLD_PCT=0.35, IMBALANCE_THRESHOLD=0.50
- MIN_SIGNAL_NOTIONAL_USD=3, PAPER_MIN_TRADE_USDC=2

#### 5. Dust Trades Fix (8766d46)
- Added `last_claimed_tier_pct` to prevent autoclaim re-triggering every 5s
- Dust guard: minimum close size $0.25

#### 6. Live Engine Hardening (6f5e265)
- Canary halt now cancels all open exchange orders (bump_reject_streak → async)
- Preflight expanded from 4→7 checks (all tokens, heartbeat, vault sign_digest, writability)
- WAL CRC32 checksum header with corruption detection

#### 7. Profitability Optimization — Phase A+B (c7f97e3)
Data-driven from 817-trade simulation:
- Max order $8→$4 (trades ≤$4 had 100% win rate)
- Stop-loss 25%→40% (stop-losses were bleeding -$147)
- Autoclaim tiers 40/70/100% → 60/100/150%
- Exit slippage 10→100bps (reality was 540bps avg)
- Momentum grace period 60s
- Entry delay (ENTRY_DELAY_SECS, configurable)
- Quadratic price-confidence sizing

#### 8. Encrypted Keystore + Wallet Generator (92377fd)
- AES-256-GCM encrypted keystore with PBKDF2-HMAC-SHA256 (600k iterations)
- CLI commands: `--generate-wallet`, `--encrypt-key`, `--decrypt-key`
- Config auto-loads from KEYSTORE_PATH, falls back to env vars
- `.env.live.template` with complete Canary Stage 1 configuration

### Current Engine Status
- **183 tests passing** (173 engine + 10 tee-vault)
- Paper trading dashboard/API active on port 3030
- Sharpe ratio ~16.5, Sortino ~50
- Web UI dev server on port 5173 (Vite)
- Agent JSON-RPC control plane on port 7878

### What Needs to Happen Next — GO LIVE

The entire code infrastructure for live trading is complete. The remaining work is **operational** (wallet setup + funding):

```bash
cd blink-engine

# Step 1: Generate a fresh trading wallet
cargo run -p engine -- --generate-wallet --save data/keystore.json

# Step 2: Register the generated address on Polymarket
#   → Visit polymarket.com, connect with the wallet address
#   → Note the "funder address" (proxy wallet) shown in account settings

# Step 3: Fund the account with ~$200 USDC on Polygon
#   → Bridge USDC from Ethereum to Polygon (or buy USDC on Polygon directly)
#   → Deposit to Polymarket proxy wallet
#   → Approve token spenders (Exchange, NegRisk Exchange, NegRisk Adapter)

# Step 4: Get CLOB API credentials
#   → POST /auth/api-key with EIP-712 signature from signer wallet
#   → Receive: api_key, api_secret (base64), api_passphrase

# Step 5: Encrypt all credentials into keystore
cargo run -p engine -- --encrypt-key data/keystore.json

# Step 6: Configure live mode
#   → Copy .env.live.template to .env
#   → Set KEYSTORE_PATH=data/keystore.json
#   → Set KEYSTORE_PASSPHRASE=<your-passphrase>
#   → Set RN1_WALLET=<whale-address>

# Step 7: Preflight validation
cargo run --release -p engine -- --preflight-live

# Step 8: Start Canary Stage 1
cargo run --release -p engine
```

### Canary Stage Progression

| Stage | Max Order | Orders/Session | Hours | Duration | Success Criteria |
|-------|-----------|---------------|-------|----------|-----------------|
| 1 | $5 | 20 | UTC 08-22 | 1-3 days | 20+ fills, 0 orphans, heartbeat stable |
| 2 | $8 | 100 | 24/7 | 3-7 days | 100+ fills, ≥80% win rate, Sharpe >5 |
| 3 | Full | Unlimited | 24/7 | Ongoing | Production monitoring |

Advance by changing `LIVE_ROLLOUT_STAGE` in .env (1→2→3).

### Key Architecture Decisions (Don't Change These)
1. **Post-only maker orders** — no taker fees, earns liquidity rebates
2. **Exchange-first reconciliation** — never trust local state for fill confirmation
3. **Canary rollout** — progressive scaling with hard guardrails
4. **Portfolio lock timeouts** — all lock acquisitions use tokio::time::timeout (500ms-2s)
5. **run_autoclaim on 5s timer** — never call from hot signal/TUI paths
6. **Signal consumers use spawn_blocking** — avoids cross-runtime deadlocks

### Critical .env Variables for Live Mode
```
LIVE_TRADING=true
TRADING_ENABLED=true
BLINK_LIVE_PROFILE=canonical-v1
KEYSTORE_PATH=data/keystore.json
KEYSTORE_PASSPHRASE=<passphrase>
LIVE_ROLLOUT_STAGE=1
```

### Build & Test Commands
```bash
cargo build --release -p engine          # Release build (~2min)
cargo test -p engine --lib               # 173 tests (skip doctests)
cargo test -p tee-vault                  # 10 tests (keystore roundtrip)
cargo run -p engine -- --preflight-live  # 7-point live validation
cargo run -p engine -- --emergency-stop  # Cancel all orders + halt
```

---

*Last updated: 2026-04-13*
