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
**Location:** `blink-engine/web-ui/` or `blink-ui/`

- **Framework:** React 19.2.4 + Vite
- **Styling:** Tailwind CSS 4.2.2
- **Charting:** Recharts 3.8.1
- **TypeScript:** ~5.9.3 (strict)
- **Linting:** ESLint 9.39.4 + TS plugin
- **Build output:** Optimized Vite bundles

#### Running Locally
```bash
cd blink-engine/web-ui
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
cd blink-engine/web-ui
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

*Last updated: 2026-04-05*
