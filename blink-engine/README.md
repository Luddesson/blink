# ⚡ Blink Engine

> **Shadow Maker Bot** — A high-frequency, low-latency trading engine for [Polymarket](https://polymarket.com) that detects and mirrors orders from a tracked "whale" wallet (RN1) on the CLOB.

```
╔══════════════════════════════════════════════════════╗
║         BLINK ENGINE v0.2 — Shadow Maker Bot        ║
╚══════════════════════════════════════════════════════╝
```

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Prerequisites](#prerequisites)
- [Quick Start](#quick-start)
- [Operating Modes](#operating-modes)
- [Configuration](#configuration)
- [Module Reference](#module-reference)
- [Running](#running)
- [Development](#development)
- [Security](#security)

---

## Overview

Blink Engine connects to Polymarket's live WebSocket feed, maintains an in-memory CLOB order book, and watches for orders from a specific high-performing wallet address (RN1). When RN1 places an order, Blink mirrors it — sizing it proportionally, passing it through a risk manager, and submitting it as a post-only maker order.

**Three operating modes:**

| Mode | Real money | Description |
|------|-----------|-------------|
| Read-only | ✗ | Connect, watch, log. Nothing else. |
| Paper | ✗ | Simulate mirror orders with $100 virtual USDC |
| Live | ✓ | Submit real orders via Polymarket CLOB REST API |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Blink Engine                                 │
│                                                                      │
│  ┌─────────────┐    ┌──────────────┐    ┌────────────────────────┐  │
│  │  ws_client  │───▶│  order_book  │    │     sniffer            │  │
│  │             │    │  (DashMap)   │    │  (RN1 wallet filter)   │  │
│  │  Reconnect  │    │  BTreeMap    │───▶│  → RN1Signal           │  │
│  │  + backoff  │    │  per market  │    └───────────┬────────────┘  │
│  └─────────────┘    └──────────────┘                │               │
│         │                                           │               │
│         │             WS Feed                crossbeam              │
│         ▼           (Polymarket)              channel               │
│  ┌─────────────────────────────────┐           │               │
│  │          Polymarket             │           ▼               │
│  │   wss://ws-live-data.poly...    │  ┌──────────────────────┐ │
│  └─────────────────────────────────┘  │   paper_engine  OR   │ │
│                                       │    live_engine        │ │
│                                       │                       │ │
│                                       │  1. size order        │ │
│                                       │  2. risk_manager      │ │
│                                       │  3. fill window       │ │
│                                       │  4. order_signer      │ │
│                                       │  5. order_executor    │ │
│                                       └──────────┬────────────┘ │
│                                                  │               │
│                                                  ▼               │
│                                       ┌──────────────────────┐  │
│                                       │  Polymarket CLOB API │  │
│                                       │  POST /order         │  │
│                                       │  (retry w/ backoff)  │  │
│                                       └──────────────────────┘  │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  Web UI dashboard — active local dashboard on :5173        │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

### Data flow (hot path)

```
WebSocket frame
  → parse MarketEvent (serde_json)
  → OrderBookStore::apply_update    ← O(log n) BTreeMap
  → Sniffer::check_order_event      ← O(1) equality check
  → [if RN1] crossbeam_channel::send
  → Engine::handle_signal
      → calculate_size_usdc
      → RiskManager::check_pre_order
      → check_fill_window (3 s × 500 ms polls)
      → sign_order (EIP-712 / secp256k1)
      → OrderExecutor::submit_order (POST /order, up to 4 attempts)
```

### Workspace crates

| Crate | Binary | Description |
|-------|--------|-------------|
| `engine` | `engine` | Main trading engine — all signal/order logic |
| `market-scanner` | `market-scanner` | CLI tool to discover and select Polymarket markets |

---

## Prerequisites

- **Rust** 1.78+ (`rustup toolchain install stable`)
- **Cargo** (bundled with Rust)
- A `.env` file — copy from `.env.example` and fill in values

---

## Quick Start

### 1. Clone and configure

```bash
git clone <repo>
cd blink-engine
cp .env.example .env
# Edit .env with your wallet address and market IDs
```

### 2. Discover markets (optional)

```bash
cargo run -p market-scanner
# Prints top sports markets by 24h volume
# Optionally auto-writes MARKETS= to .env
```

### 3. Run in read-only mode

```bash
cargo run -p engine
# Connects to WebSocket, watches for RN1 orders, prints signals
```

### 4. Run paper trading with the Web UI

```bash
PAPER_TRADING=true WEB_UI=true cargo run -p engine
# Web dashboard on http://localhost:5173 — no real funds used
```

### 5. Run in live mode (⚠️ real money)

Ensure all live credentials are set in `.env`, then:

```bash
LIVE_TRADING=true cargo run --release -p engine
```

---

## Operating Modes

### Read-only mode (default)

```
PAPER_TRADING=  (not set)
LIVE_TRADING=   (not set)
```

Connects to the WebSocket feed, maintains order books, logs every RN1 signal detected. **No orders are ever placed.** Useful for monitoring and validating the sniffer before committing funds.

### Paper trading mode

```
PAPER_TRADING=true
WEB_UI=true        # active dashboard
```

Simulates mirror orders using $100 virtual USDC. The full pipeline runs (sizing, risk checks, fill window) but no HTTP requests are made to the CLOB. Closed trades, P&L, and position history are tracked in memory.

**Fill window:** After a signal is detected, the engine polls the order book every 500 ms for 3 seconds. If the price drifts more than 1.5% from the entry price, the order is aborted (simulating an in-play failsafe for sports markets).

### Live trading mode

```
LIVE_TRADING=true
# All credential env vars must be set (see Configuration)
```

Submits real maker orders via `POST /order` on the Polymarket CLOB. Orders are EIP-712 signed using your private key. The `order_executor` retries transient API errors (HTTP 429, 5xx, `"transient"` error messages) with exponential backoff (200 ms → 400 ms → 800 ms, up to 4 attempts).

> ⚠️ **Never set `LIVE_TRADING=true` without also verifying the risk parameters in `.env`.**

---

## Configuration

Copy `.env.example` to `.env` and fill in all required values.

### Core (required)

| Variable | Description | Example |
|----------|-------------|---------|
| `CLOB_HOST` | Polymarket CLOB REST API base URL | `https://clob.polymarket.com` |
| `WS_URL` | WebSocket feed URL | `wss://ws-live-data.polymarket.com` |
| `RN1_WALLET` | Lowercase hex wallet address to track | `0xabcd...` |
| `MARKETS` | Comma-separated Polymarket token IDs | `12345,67890` |

### Trading mode flags

| Variable | Default | Description |
|----------|---------|-------------|
| `PAPER_TRADING` | `false` | Enable paper trading simulation |
| `TUI` | `false` | Archived legacy flag; ignored at runtime |
| `TUI_MODERN_THEME` | `false` | Start TUI in modern theme (toggle live with `t`) |
| `LIVE_TRADING` | `false` | Enable real order submission |

### Live trading credentials (required when `LIVE_TRADING=true`)

| Variable | Description |
|----------|-------------|
| `SIGNER_PRIVATE_KEY` | 64-char hex secp256k1 private key (for EIP-712 signing) |
| `POLYMARKET_FUNDER_ADDRESS` | Your funder/proxy-wallet address (`0x...`) |
| `POLYMARKET_API_KEY` | Polymarket L2 API key |
| `POLYMARKET_API_SECRET` | Polymarket L2 API secret (base64-encoded) |
| `POLYMARKET_API_PASSPHRASE` | Polymarket L2 API passphrase |
| `POLYMARKET_SIGNATURE_TYPE` | EIP-712 signature type (`0..2`) |
| `POLYMARKET_ORDER_NONCE` | Explicit order nonce (default `0`) |
| `POLYMARKET_ORDER_EXPIRATION` | Unix expiry (`0` or future timestamp) |
| `BLINK_LIVE_PROFILE` | Must be `canonical-v1` in live mode |

### Canary rollout guardrails (Phase C)

| Variable | Description |
|----------|-------------|
| `LIVE_ROLLOUT_STAGE` | Rollout stage (`1`, `2`, `3`) |
| `LIVE_CANARY_MAX_ORDER_USDC` | Hard cap per accepted live order |
| `LIVE_CANARY_MAX_ORDERS_PER_SESSION` | Hard cap on accepted orders per process run (`0` disables cap) |
| `LIVE_CANARY_DAYTIME_ONLY` | Restrict live execution to UTC window |
| `LIVE_CANARY_START_HOUR_UTC` | Inclusive start hour (`0..23`) |
| `LIVE_CANARY_END_HOUR_UTC` | Exclusive end hour (`0..23`) |
| `LIVE_CANARY_MAX_REJECT_STREAK` | Auto-halt after N consecutive submit failures |
| `LIVE_CANARY_ALLOWED_MARKETS` | Optional comma-separated allowlist of token IDs |

### Risk management

| Variable | Default | Description |
|----------|---------|-------------|
| `MAX_DAILY_LOSS_PCT` | `0.10` | Maximum daily loss as fraction of starting NAV (10%) |
| `MAX_CONCURRENT_POSITIONS` | `5` | Maximum simultaneous open positions |
| `MAX_SINGLE_ORDER_USDC` | `20.0` | Maximum USDC per single order |
| `MAX_ORDERS_PER_SECOND` | `3` | Per-second order rate limit (CLOB safety) |
| `TRADING_ENABLED` | `false` | Master kill switch — must be `true` to submit orders |

### Logging

| Variable | Default | Description |
|----------|---------|-------------|
| `LOG_LEVEL` | `info` | Tracing filter: `trace`, `debug`, `info`, `warn`, `error` |
| `WS_RECONNECT_DEBOUNCE_MS` | `1500` | Minimum delay between forced WS reconnects |
| `WS_PARSE_ERROR_PREVIEW_CHARS` | `120` | Max chars from raw WS payload logged on parse failure |
| `TWIN_AUTOCLAIM_TARGET_PNL_PCT` | `100` | Optional Blink Twin-only autoclaim threshold |
| `AUTOCLAIM_TIERS` | `40:0.30,70:0.30,100:1.0` | Tiered partial exits for paper autoclaim |
| `TWIN_AUTOCLAIM_TIERS` | `40:0.30,70:0.30,100:1.0` | Tiered partial exits for Blink Twin autoclaim |
| `PAPER_TOKEN_MAX_EXPOSURE_PCT` | `0.20` | Per-token max exposure cap as share of NAV |
| `PAPER_REALISM_MODE` | `false` | Enables conservative fill/fee/marking assumptions for paper mode |
| `PAPER_ADVERSE_FILL_BPS` | `10` | Entry fill worsened against you (bps) when realism mode is on |
| `PAPER_TAKER_FEE_BPS` | `7` | Fee applied on entry+exit notional in paper mode |
| `PAPER_EXIT_HAIRCUT_BPS` | `12` | Unrealized NAV marked conservatively via exit haircut |
| `AUTO_POSTRUN_REVIEW` | `true` | Generate an automatic post-run evaluation report on graceful shutdown |
| `POSTRUN_REVIEW_DIR` | `logs\reports` | Directory where post-run evaluation files are written |

Runtime log files are always persisted:

- `logs/engine.log.YYYY-MM-DD` (daily rotated engine log)
- `logs/sessions/engine-session-YYYYMMDD-HHMMSS.log` (one file per process run)
- `logs/LATEST_SESSION_LOG.txt` (pointer to the newest session log file)
- `logs/reports/postrun-review-YYYYMMDD-HHMMSS.txt` (structured automatic run evaluation)
- `logs/LATEST_POSTRUN_REVIEW.txt` (pointer to latest evaluation)

### Game-start watcher

| Variable | Default | Description |
|----------|---------|-------------|
| `GAME_WATCHER_INTERVAL_MS` | `500` | Poll interval for in-play detection (milliseconds) |

---

## Module Reference

### `engine` crate

| Module | Purpose |
|--------|---------|
| `types` | Core types: `OrderSide`, `TimeInForce`, `MarketEvent`, `RN1Signal`, price helpers |
| `config` | Runtime config loaded from env — call `Config::from_env()` once at startup |
| `ws_client` | Persistent WebSocket client with exponential-backoff reconnection |
| `order_book` | In-memory CLOB order book (`BTreeMap` per market, `DashMap` across markets) |
| `sniffer` | RN1 wallet filter — emits `RN1Signal` on matching `"order"` events |
| `rn1_poller` | REST RN1 trade poller with adaptive interval, error backoff, and cooldown guard |
| `paper_engine` | Paper trading engine — full pipeline simulation, no network orders |
| `live_engine` | Live trading engine — EIP-712 signing + CLOB REST submission |
| `paper_portfolio` | Virtual $100 USDC portfolio state, sizing, and P&L tracking |
| `order_signer` | EIP-712 order signing (k256 / secp256k1, manual Keccak256) |
| `order_executor` | CLOB REST API client (`POST/DELETE/GET /order`) with retry logic |
| `clob_client` | Read-only CLOB REST client (prices, order books, markets) |
| `risk_manager` | Pre-order risk checks: kill switch, circuit breaker, rate limit, daily loss |
| `activity_log` | Thread-safe ring buffer of engine events for the web dashboard and legacy tooling |
| `latency_tracker` | Rolling-window latency stats (min/max/avg/p99 in µs) |
| `tui_app` | archived ratatui terminal dashboard (no longer launched) |
| `game_start_watcher` | Polls CLOB prices to detect in-play transitions; fires order wipe signals |

### `market-scanner` crate

Standalone CLI binary. Queries the Polymarket Gamma API for active markets, displays top sports and general markets sorted by 24h volume, and can auto-update `MARKETS=` in your `.env`.

```bash
cargo run -p market-scanner
```

---

## Running

### Build

```bash
# Debug (fast compile, slow runtime)
cargo build

# Release (slow compile, max performance)
cargo build --release
```

### Run tests

```bash
cargo test
```

### Run with verbose logging

```bash
LOG_LEVEL=debug cargo run -p engine
```

RN1 poller now adapts between fast and idle polling, applies exponential backoff on repeated request failures, and activates a cooldown guard on sustained consecutive errors.

### Legacy TUI status

```bash
TUI=true
```

The ratatui dashboard is archived. `TUI=true` is ignored and Blink stays on the web UI flow.

---

## Development

### Code organization

- All modules are declared in `crates/engine/src/lib.rs`
- Business logic lives in individual modules; `main.rs` only wires them together
- Hot-path code (order book updates, sniffer checks) uses integer math exclusively (`u64` ×1000)
- All types use `#[tracing::instrument]` for automatic span logging

### Adding a new module

1. Create `crates/engine/src/my_module.rs`
2. Add `pub mod my_module;` to `lib.rs`
3. Add `//!` module-level doc comment at the top of the file

### Pricing convention

All prices and sizes are stored as `u64` scaled by **1,000**:

```rust
// "0.65" → 650
// "1500" → 1_500_000
use engine::types::parse_price;
```

Use `format_price(p)` to convert back to a human-readable decimal string.

### Retry policy (order submission)

`OrderExecutor::submit_order` retries up to **4 attempts** with exponential backoff:

| Attempt | Delay before attempt |
|---------|---------------------|
| 1 | — (immediate) |
| 2 | 200 ms |
| 3 | 400 ms |
| 4 | 800 ms |

Retryable conditions: HTTP 429, HTTP 5xx, `success=false` with `"transient"` in `errorMsg`.
Non-retryable: HTTP 4xx (except 429), `success=false` with permanent rejection message.

Auth headers (`POLY-TIMESTAMP`) are rebuilt on each attempt to avoid stale timestamps.

---

## Security

- **Never commit `.env` to version control.** It is already in `.gitignore`.
- Private keys should be set via environment variables, not hardcoded.
- The `TRADING_ENABLED` flag is `false` by default — you must explicitly opt in to live trading.
- The circuit breaker automatically trips when daily loss limits are exceeded, blocking further orders until the engine is restarted.
- All post-only orders use `maker: true` (enforced in `order_executor`) to prevent paying taker fees.

---

## License

Private / proprietary. All rights reserved.
