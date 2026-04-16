# Blink Engine — Copilot Instructions

Blink is a high-frequency **shadow-maker bot** for [Polymarket](https://polymarket.com). It tracks a specific whale wallet (RN1), mirrors their CLOB orders as post-only maker orders, and optionally runs an AI sidecar (Grok/GPT) that generates autonomous alpha signals. Rust backend + React/Vite dashboard + Python AI sidecar.

---

## Commands

All Rust commands: run from `blink-engine/`. All web commands: run from `blink-engine/web-ui/`.

### Rust

```bash
# Build
cargo build                          # debug (opt-level 1)
cargo build --release                # LTO fat + 1 codegen unit + symbol strip

# Run — ALL mode switching is via .env, never CLI args
cargo run -p engine                                 # read-only
PAPER_TRADING=true TUI=true cargo run -p engine    # paper + ratatui dashboard
LIVE_TRADING=true cargo run --release -p engine    # live ⚠️ real money

# Special CLI bypasses (not env-driven)
cargo run -p engine -- --backtest ticks.csv [--output report.json]
cargo run -p engine -- --preflight-live   # validate live config, no trades
cargo run -p engine -- --emergency-stop   # cancel all open orders, then exit

# Test
cargo test --workspace                    # full suite (~152 tests)
cargo test -p engine <name_substring>     # single test
cargo test --workspace -- proptest        # property-based only (10k iters each)

# Lint / format (CI enforces -D warnings)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --workspace -- --check
```

### Web UI

```bash
cd blink-engine/web-ui
npm run dev     # Vite dev server (hot reload)
npm run build   # tsc + vite build
npm run lint    # ESLint 9 + TS plugin
```

### Alpha Sidecar (Python)

```bash
cd blink-engine/alpha-sidecar
pip install -e .
alpha-sidecar          # or: python -m alpha_sidecar.main
# Engine must be running first — sidecar connects to port 7878
```

### Formal Verification

```bash
cargo test --workspace -- proptest                 # property-based (built-in)
pip install halmos && cd blink-engine/formal && make verify  # Halmos symbolic
cargo kani --harness verify_compute_amounts_no_overflow      # Kani (Linux only)
```

---

## Architecture

### Operating modes

`PAPER_TRADING` and `LIVE_TRADING` are mutually exclusive — the engine exits with an error if both are set.

| Mode | Flags | What runs |
|------|-------|-----------|
| Read-only | _(none)_ | WS + order book + sniffer + logging. Zero orders ever. |
| Paper | `PAPER_TRADING=true` | Full pipeline, virtual $100 USDC, no HTTP orders. |
| Paper+TUI | `PAPER_TRADING=true TUI=true` | Same + ratatui. Logs become file-only (no stderr). |
| Live | `LIVE_TRADING=true` | EIP-712 signed real orders. Needs all creds + `TRADING_ENABLED=true` + `BLINK_LIVE_PROFILE=canonical-v1`. |

### Dual signal sources

The engine receives `RN1Signal` from two independent paths simultaneously:

1. **WS sniffer** — `ws_client` → `sniffer` → `crossbeam_channel`. Fastest. Subject to Cloudflare RSTs.
2. **REST poller** — `rn1_poller` polls `https://data-api.polymarket.com/activity?user={RN1_WALLET}` every **1.2 s** (idle: 2 s). No auth needed. **This is the primary/resilience source.**

Both emit `RN1Signal` into the same crossbeam channel. Deduplication happens inside `handle_signal()` via a `seen_order_ids: HashSet<String>` (first gate in the signal pipeline).

The **Alpha sidecar** (Python/Grok) submits `AlphaSignal` via JSON-RPC `submit_alpha_signal` on port 7878. These enter through a separate channel and get their own risk layer.

### Full hot-path data flow

```
WebSocket frame
  → parse MarketEvent (serde_json, tagged enum on "event_type" field)
  → OrderBookStore::apply_update    [O(log n) BTreeMap; size=0 → remove level]
  → Sniffer::check_order_event      [O(1) lowercase str equality on "order" events]
  → crossbeam_channel::send(RN1Signal)

handle_signal() — signal filtering pipeline (ordered; first failing gate returns early):
  1.  order_id dedup          → seen_order_ids HashSet (capacity 512)
  2.  per-token dedup         → skip if already holding position on token_id
  3.  match concentration     → MAX_POSITIONS_PER_MATCH (default 2) per event title prefix
  4.  event horizon           → skip if event_start_time > now + 6h
  5.  min notional            → MIN_SIGNAL_NOTIONAL_USD (default $10) on RN1's notional
  6.  extreme price filter    → skip if price < 0.10 or > 0.95 ("no edge")
  7.  market category block   → skip esports: "esports","lol:","cs2:","cs:go","dota",
                                 "valorant","league of legends","counter-strike",
                                 "overwatch","bo3)","bo5)","lec ","lck ","lpl ","vct "
  8.  fee-aware cash gate     → skip high-fee (>4%) when cash < 50% of NAV
  9.  fee-to-edge filter      → skip if est. fee > 60% of est. edge
  10. priority queue          → BinaryHeap<PrioritySignal> ordered by edge_score (f64)
  11. metadata enrichment     → Gamma API (cached 5 min); fills market_title, outcome, event times
  12. conviction sizing       → SIZE_MULTIPLIER × RN1 notional × conviction_multiplier()
                                + Bullpen discovery_boost + convergence_boost (cold path)
  13. RiskManager::check_pre_order [7 checks — see Risk section]
  14. per-token drift cooldown → DRIFT_ABORT_COOLDOWN_SECS (default 30s) blocks re-entry
  15. InPlayFailsafe::run()   → 3s countdown, poll /price every 100ms, abort if >150bps drift
  16. sign_order              → EIP-712 / k256 secp256k1 / manual Keccak256
  17. OrderExecutor::submit_order → POST /order, up to 4 attempts (0→200→400→800ms)

Each rejected signal is tracked in RejectionAnalytics with reason + timestamp.
```

### Engines

| Engine | File | Key distinction |
|--------|------|-----------------|
| `PaperEngine` | `paper_engine.rs` | Full pipeline, no HTTP orders. Maintains `WarmState` for crash recovery. |
| `LiveEngine` | `live_engine.rs` | Real orders. TEE vault for key isolation. `CanaryPolicy` for phased rollout. Fill recording **deferred** until `truth_reconciler` confirms via `GET /order/{id}`. |
| `BacktestEngine` | `backtest_engine.rs` | Tick CSV replay through `VirtualClock`. Anti-lookahead: entry price always at signal time; fill window only looks forward for drift checks. |
| `BlinkTwin` | `blink_twin.rs` | Adversarial shadow engine running in parallel. Adds extra latency + slippage penalty + tighter drift multiplier. Self-mutates parameters each generation to find profitability boundary. |

### Workspace crates

| Crate | Purpose |
|-------|---------|
| `engine` | Main trading engine — all signal/order logic |
| `market-scanner` | CLI for market discovery (Gamma API), can auto-write `MARKETS=` to `.env` |
| `tee-vault` | Private key isolation task; k256 + zeroize |
| `bpf-probes` | eBPF instrumentation — no-op stub on non-Linux dev machines |

### Complete module map

| Module | Hot/Cold | Role |
|--------|----------|------|
| `types` | hot | Core types; `parse_price`/`format_price`; `MarketEvent` tagged enum |
| `config` | startup | All env config; `Config::from_env()` once; wrap in `Arc<Config>` |
| `ws_client` | hot | WS connection; exponential backoff; application-level PING text frames |
| `order_book` | hot | `BTreeMap` per market + `DashMap` across markets; size=0 → remove |
| `sniffer` | hot | Wallet filter; address normalized to lowercase at construction |
| `rn1_poller` | warm | REST polling 1.2s/2s; adaptive interval; circuit breaker (10 errors → 30s) |
| `paper_engine` | warm | Full pipeline simulation; `WarmState` for hot-restart; A/B experiments |
| `live_engine` | warm | Real orders; `CanaryPolicy`; `truth_reconciler` SSOT; TEE vault |
| `backtest_engine` | offline | Tick CSV replay; `VirtualClock`; anti-lookahead guaranteed |
| `blink_twin` | warm | Adversarial shadow; self-mutating `TwinConfig` each generation |
| `paper_portfolio` | accounting | Virtual $100 USDC; `PersistedPaperPortfolio` serde layer for evolution |
| `order_signer` | warm | Manual EIP-712; Polygon chain 137; CTF Exchange `0x4bFb…8982E` |
| `order_executor` | warm | HMAC-SHA256 auth; `POST/DELETE/GET /order`; retry only 429/5xx/transient |
| `clob_client` | cold | Read-only CLOB REST: prices, books, markets |
| `risk_manager` | warm | 7-check pre-order gate; circuit breaker; VaR; rate limit |
| `exit_strategy` | warm | Pure `evaluate_exits()` function; 7 exit action types |
| `in_play_failsafe` | warm | 3s countdown; 100ms polls; `FailsafeResult::{Stable,DriftAbort,FetchError}` |
| `truth_reconciler` | warm | SSOT fill confirmation; `PendingOrder` → `Confirmed`/`NoFill`; stale flag >300s |
| `game_start_watcher` | warm | Polls CLOB 500ms; detects in-play; fires order-wipe signal |
| `alpha_signal` | warm | `AlphaSignal` type; `SignalSource` enum (Rn1Copytrade/AiAutonomous/SmartMoneyConvergence) |
| `agent_rpc` | cold | JSON-RPC 2.0 on port 7878: `blink_status`, `paper_summary`, `set_pause`, `alpha_status`, `submit_alpha_signal` |
| `web_server` | cold | Axum REST + WS broadcast (`WEB_UI=true`); `AppState` shared via `State<>` |
| `tui_app` | cold | ratatui dashboard (`TUI=true`); reads from `ActivityLog` ring buffer |
| `activity_log` | hot | Thread-safe ring buffer (200 entries); `EntryKind` enum for TUI coloring |
| `latency_tracker` | warm | Rolling window stats (min/max/avg/p99 µs); window size configurable |
| `tick_recorder` | cold | ClickHouse batch writer for tick events (`CLICKHOUSE_URL` activates) |
| `clickhouse_logger` | cold | Extended data warehouse: books, signals, executions, metrics |
| `bullpen_bridge` | cold | Bullpen CLI wrapper; **NEVER call from hot path**; semaphore (max 3 concurrent) |
| `bullpen_discovery` | cold | Market discovery → `DiscoveryStore`; provides `conviction_boost(token_id)` |
| `bullpen_smart_money` | cold | Whale convergence → `ConvergenceStore`; provides `convergence_boost(token_id)` |
| `execution_provider` | stub | `ExecutionProvider` trait for future Fireblocks/BitGo integration |
| `market_metadata` | cold | Gamma API cache; 5 min TTL per market |
| `gas_oracle` | cold | Moving-average gas price for Polygon (`ETHERSCAN_API_KEY`) |
| `heartbeat` | cold | Periodic liveness ping for external monitors |
| `mev_router`, `mev_shield`, `tx_router`, `io_uring_net` | stub | Infrastructure scaffolding for future on-chain work |

`main.rs` **only wires modules together**. All business logic lives in individual modules.

### Web UI + API

- React 19 + Vite + Tailwind CSS 4 + Recharts 3; TypeScript strict
- Served by Axum (`WEB_UI=true`) from `blink-engine/static/`
- Real-time state via WS broadcast every `WS_BROADCAST_INTERVAL_SECS` (default 10 s)
- Key REST: `GET /status`, `GET /book/:token_id`, `GET /activity`, `POST /pause`, `GET /twin`

---

## Conventions — Read Every One

### 1. Price/size scaling — the most critical rule in this codebase

All prices and sizes are `u64` scaled by **×1,000** — zero floats in the hot path:

```rust
parse_price("0.65")   // → 650
parse_price("1500")   // → 1_500_000
format_price(650)     // → "0.65"
```

`f64` is only acceptable in `PaperPortfolio`, `ClosedTrade`, and `PaperPosition` (accounting paths). **Never use floats in `OrderBook`, `Sniffer`, `ws_client`, or anything on the WebSocket event path.** When converting back in `paper_engine.rs`: `signal.price as f64 / 1_000.0`.

### 2. Order book delta protocol

Polymarket uses deltas: a `size == 0` price level means **remove that level**. Both `apply_bids_delta()` and `apply_asks_delta()` handle this — never skip zero-size levels.

BTreeMap ordering:
- Bids: ascending keys → `best_bid()` = `.keys().next_back()` (highest)
- Asks: ascending keys → `best_ask()` = `.keys().next()` (lowest)

### 3. Config loading pattern — used everywhere

Every module with tunable parameters has a `from_env()` constructor. It always falls back to a safe default — never panics on a missing var. The exact pattern used throughout:

```rust
let value = std::env::var("MY_VAR")
    .ok()
    .and_then(|v| v.parse::<f64>().ok())
    .unwrap_or(DEFAULT_VALUE);
```

`Config::from_env()` is called **once in `main.rs`** and wrapped in `Arc<Config>`. Never construct it more than once. `RiskConfig::from_env()`, `ExitConfig::from_env()`, `BullpenConfig::from_env()`, `InPlayFailsafeConfig::from_env()` follow the same pattern.

### 4. Adding a new module

1. Create `crates/engine/src/my_module.rs`
2. Add `pub mod my_module;` to `lib.rs` (maintain alphabetical grouping)
3. Start the file with a `//!` doc comment: one-line purpose + hot/cold path classification
4. If the module has tunable params, add `struct MyConfig` + `impl MyConfig { pub fn from_env() -> Self }`
5. If the module is cold-path only, add the warning: `//! **Latency class: COLD PATH.  Never call from the signal → order hot path.**`

### 5. Workspace dependencies

**Never add a version number directly in a crate's `Cargo.toml`.** All versions live once in `blink-engine/Cargo.toml` under `[workspace.dependencies]`. Reference with `{ workspace = true }`.

### 6. Logging — always structured fields

Use `tracing` macros, never `println!`/`eprintln!` in library code. Use structured key=value fields:

```rust
tracing::info!(token_id = %signal.token_id, side = ?signal.side, price, "RN1 signal detected");
tracing::warn!(drift_bps, elapsed_ms, "fill window aborted — price drifted");
tracing::error!(err = ?e, order_id = %id, "order submit failed");
```

In TUI mode, logs become file-only (stderr suppressed — do not add `eprintln!` for TUI debugging).

### 7. Concurrency — match the right primitive to the context

| State | Primitive | Reason |
|-------|-----------|--------|
| Portfolio, signal queue, twin state | `Arc<tokio::sync::Mutex<T>>` | Held across `.await` points |
| Risk manager, fill window, drift cooldown | `Arc<std::sync::Mutex<T>>` | Short sync critical sections, no async needed |
| WS health counters, equity tick | `Arc<AtomicU64>`, `AtomicBool` | Lock-free reads from multiple tasks |
| Signal hot path | `crossbeam_channel` | Lock-free MPSC, bounded |
| Bullpen concurrent access | `Arc<tokio::sync::RwLock<T>>` | Many readers, rare writers |

**`run_autoclaim()` runs on a 5-second background timer in `main.rs` only.** Never call it from signal handling or TUI rendering — it acquires the portfolio lock and will cause starvation.

### 8. Risk manager — 7 checks in order, all must pass

`RiskManager::check_pre_order` is the last gate before signing/submitting. Checks in sequence:
1. Kill switch: `TRADING_ENABLED=true` required
2. Circuit breaker: manually tripped or auto-tripped by daily loss
3. Daily loss limit: `MAX_DAILY_LOSS_PCT` (default 10% of starting NAV)
4. Position cap: `MAX_CONCURRENT_POSITIONS` (default 5)
5. Order size cap: `MAX_SINGLE_ORDER_USDC` (default $20)
6. Rate limit: `MAX_ORDERS_PER_SECOND` (default 3)
7. Rolling VaR: 60s window, `VAR_THRESHOLD_PCT` (default 5% of NAV)

Circuit breaker auto-trips when daily loss limit is exceeded. **Engine restart required to reset.** `daily_pnl` is updated **only by `record_close()` with realized P&L** — never in `record_fill()`.

### 9. Exit strategy — keep it pure

`exit_strategy::evaluate_exits(positions, config)` is a **pure function** with no side effects. It returns `Vec<(position_id, ExitAction)>`. The caller (`PaperEngine`, `LiveEngine`) executes the exits. **Do not add side effects to this function.** Testability depends on purity.

Exit action types: `TakeProfit` (partial, tiered), `StopLoss`, `TrailingStop`, `StagnantExit`, `Resolved` (price ≥0.99 or ≤0.01), `MarketNotLive`, `MaxHoldExpired`.

Tiered exits via `AUTOCLAIM_TIERS` env var format: `"40:0.30,70:0.30,100:1.0"` = at +40% close 30%, at +70% close 30%, at +100% close 100%.

### 10. Live engine: SSOT reconciliation (critical for live mode)

In `LiveEngine`, **no fill is recorded locally until the exchange confirms it**. The flow:

```
submit_order() → PendingOrder { lifecycle: AwaitingConfirmation }
     ↓ (every LIVE_RECONCILE_INTERVAL_SECS, default 10s)
truth_reconciler::process_order_status() — GET /order/{id}
     ↓ matched/filled → ReconciliationOutcome::Fill { actual_size_usdc }
LiveEngine records fill using ACTUAL exchange amounts, not expected amounts
     ↓ rejected/cancelled/expired → ReconciliationOutcome::NoFill
     No fill recorded. Position never opened.
```

Orders pending >300s are flagged as stale and an operator alert is emitted. The `detect_position_drift()` function alerts when local vs exchange size diverges >5%.

### 11. TEE vault — private key isolation

In live mode, the private key is immediately handed to `tee_vault::VaultHandle::spawn()` and runs in an isolated task. **If vault init fails when `LIVE_TRADING=true`, the engine PANICS** — this is intentional; there is no silent fallback to dry-run. The signing key never exists in the engine's main tasks after vault init.

### 12. Canary rollout policy (live mode)

`CanaryPolicy` applies additional live trading guardrails on top of risk manager:
- `LIVE_ROLLOUT_STAGE`: phase 1/2/3 (controls which guardrails apply)
- `LIVE_CANARY_MAX_ORDER_USDC`: hard cap per order
- `LIVE_CANARY_MAX_ORDERS_PER_SESSION`: auto-halt after N accepted orders
- `LIVE_CANARY_DAYTIME_ONLY=true`: restrict to UTC window `[START_HOUR, END_HOUR)`
- `LIVE_CANARY_MAX_REJECT_STREAK`: auto-halt after N consecutive API rejections
- `LIVE_CANARY_ALLOWED_MARKETS`: optional allowlist of token IDs

### 13. EIP-712 signing specifics

Domain: `name="Polymarket CTF Exchange"`, `version="1"`, `chainId=137`, `verifyingContract=0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E`. No `alloy` dependency — all manual via `k256` + `sha3::Keccak256`. **Auth headers (`POLY-TIMESTAMP`) must be rebuilt on every retry attempt** — stale timestamps cause 401s.

`nonce_counter` is `AtomicU64`, incremented per submission. Initial value from `POLYMARKET_ORDER_NONCE`.

### 14. WebSocket protocol — exact Polymarket spec

```
URL:       wss://ws-subscriptions-clob.polymarket.com/ws/market
Subscribe: {"type":"market","assets_ids":["token_id_1","token_id_2"]}
           (note: "assets_ids" plural is Polymarket's key — not a typo)
Dynamic:   {"operation":"subscribe","assets_ids":["new_token_id"]}
Ping:      Send "PING" text frame every 10s (NOT protocol ping — text only)
Pong:      Server replies "PONG" text frame
```

**TCP_NODELAY must remain disabled (Nagle on).** Enabling it causes near-instant Cloudflare RSTs. Reconnection: backoff 1s → 30s ceiling. After 3 consecutive failures → 45s cooldown. Sessions ≥15s reset both backoff and counter. Pong timeout 45s → force reconnect.

**The WS feed is unreliable.** Cloudflare RSTs the connection intermittently (os error 10054) after 2–60s. The REST poller (`rn1_poller`) is the primary reliability guarantee.

### 15. State persistence — warm restarts

`PaperEngine` persists `WarmState` to disk for hot-restarts:

```rust
WarmState {
    schema_version: u32,        // bump when adding fields; readers skip unknown versions
    market_subscriptions,       // re-subscribed on WS reconnect
    order_books,                // avoid cold-start book latency
    portfolio_path: String,
    rejection_analytics,        // reason → timestamp history
    comparator: ShadowComparator, // expected vs actual fill tracking
    experiments: ExperimentSwitches,
    checksum: u64,              // CRC of serialized data; mismatch → reload rejected
}
```

All portfolio writes use `atomic_write_with_backup()` (write to temp → rename) to prevent corruption on crash. Portfolio uses a separate `Persisted*` serde layer (not the runtime struct) so fields can be added with `#[serde(default)]` without breaking existing saves.

### 16. A/B experiment system

`ExperimentSwitches` enables runtime A/B experiments without config changes:
- `EXPERIMENT_SIZING_B=true` — alternative sizing formula
- `EXPERIMENT_AUTOCLAIM_B=true` — alternative autoclaim tier schedule
- `EXPERIMENT_DRIFT_B=true` — alternative drift threshold

Results tracked per-variant in `ExperimentMetrics`. Compare via `paper_summary` RPC.

### 17. Signal priority queue

Incoming signals are immediately pushed to `BinaryHeap<PrioritySignal>` ordered by `edge_score: f64`. `edge_score` is computed by `compute_edge_score()` which factors in RN1 notional size, price midpoint confidence, market category, and Bullpen boosts. The highest-edge signal is popped and processed first — low-edge signals may be dropped under load.

### 18. Portfolio accounting — f64 is acceptable here

Inside `PaperPortfolio`/`PaperPosition`/`ClosedTrade`, `f64` is used for all monetary values. This is intentional — these are cold-path accounting structures, not the hot path. Positions track: `entry_price`, `shares`, `usdc_spent`, `entry_fee_paid_usdc`, `current_price`, `peak_price` (for trailing stop), `entry_slippage_bps`, `queue_delay_ms`, `experiment_variant`, `fee_category`, `fee_rate`.

### 19. Fee detection and accounting

`detect_fee_category(market_title: &str) -> (&'static str, f64)` uses keyword matching:
- **geopolitics** (0.0%): "geopolit", "sanction", "nato", "war ", "military", "treaty", "united nations", "diplomacy"
- **sports** (0.01%): "vs ", "nba", "nfl", "mlb", "nhl", "soccer", "tennis", "f1 ", "premier league", etc.
- **politics** (0.01%): "president", "election", "congress", "trump", "biden", "poll", "vote", etc.
- **crypto** (0.01%): "bitcoin", "btc", "ethereum", "eth ", "solana", "defi", "nft", etc.
- **other** (0.01%): default fallback

Override with `POLYMARKET_FEE_RATE` env var. Fee is applied on both entry and exit notional.

### 20. Alpha sidecar integration

- Sidecar connects to `BLINK_RPC_URL=http://127.0.0.1:7878` (engine must start first)
- Default LLM: Grok-3 via xAI (`XAI_API_KEY`); switch to OpenAI via `LLM_BASE_URL=https://api.openai.com/v1` + `OPENAI_API_KEY`
- Sidecar pre-filter: confidence ≥ `ALPHA_CONFIDENCE_FLOOR` (default 0.65) + edge ≥ `ALPHA_MIN_EDGE_BPS` (default 500 = 5%)
- Engine then applies a second risk layer independently: `ALPHA_TRADING_ENABLED`, `ALPHA_MAX_CONCURRENT_POSITIONS`, `ALPHA_MAX_DAILY_LOSS_PCT`, standard circuit breaker
- **Both layers must pass before execution**
- Monitor: `curl http://127.0.0.1:7878/rpc -d '{"jsonrpc":"2.0","id":"1","method":"alpha_status","params":{}}'`

### 21. Bullpen bridge — cold path only

`BullpenBridge` is explicitly a cold-path component (500ms–10s latency). Never call it from signal handling or order execution. Semaphore limits to `BULLPEN_MAX_CONCURRENT=3` concurrent commands. Circuit breaker trips after 10 consecutive errors → 30s cooldown. On Windows dev machines: `BULLPEN_USE_WSL=true`, `BULLPEN_CLI_PATH="wsl -d Ubuntu -- bullpen"` (~7s latency per discover command).

### 22. OrderExecutor retry policy

Retryable: HTTP 429, HTTP 5xx, `success=false` where `errorMsg` contains `"transient"`.
Non-retryable: HTTP 4xx (except 429), permanent rejection messages.
Schedule: attempt 1 immediate → +200ms → +400ms → +800ms (4 attempts max).
Auth headers rebuilt fresh on every attempt (stale timestamp = auth failure).

### 23. Security constraints

- Never commit `.env` — already `.gitignore`d; contains private key, API secrets
- In `tee-vault`: all key material uses `zeroize::Zeroize` (wiped on drop)
- `TRADING_ENABLED` defaults to `false` — must explicitly opt in
- `LIVE_TRADING=true` without `BLINK_LIVE_PROFILE=canonical-v1` → `config.validate_live_profile_contract()` panics at startup
- TEE vault init failure in live mode → intentional panic, no silent degradation

### 24. Log file layout

```
blink-engine/logs/
  engine.log.YYYY-MM-DD                    ← daily-rotated main log
  sessions/engine-session-YYYYMMDD-HHMMSS.log  ← one file per process run
  LATEST_SESSION_LOG.txt                   ← pointer to newest session log
  reports/postrun-review-YYYYMMDD-HHMMSS.txt   ← auto on graceful shutdown
  LATEST_POSTRUN_REVIEW.txt                ← pointer to latest review
  paper_portfolio_state.json               ← last-known portfolio (panic recovery)
  paper_portfolio_state.json.bak           ← backup before each atomic write
```

`AUTO_POSTRUN_REVIEW=true` (default) generates the structured evaluation on shutdown. Check `LATEST_POSTRUN_REVIEW.txt` after every paper session.

---

## Anti-patterns — Never Do These

- **Float arithmetic in hot path** — `OrderBook`, `Sniffer`, `ws_client`. Use `u64` + `parse_price`.
- **Call `run_autoclaim()` outside the 5s timer** — portfolio lock starvation.
- **Call `BullpenBridge` from signal → order path** — 500ms+ latency, breaks HFT guarantees.
- **Record a fill before `truth_reconciler` confirms it (live mode)** — breaks SSOT invariant.
- **Side effects in `exit_strategy::evaluate_exits()`** — must remain pure for testability.
- **Version numbers in crate `Cargo.toml`** — always use `{ workspace = true }`.
- **Enable TCP_NODELAY on the WS connection** — immediate Cloudflare RSTs.
- **Set both `PAPER_TRADING=true` and `LIVE_TRADING=true`** — engine exits with error.
- **Skip zero-size levels in order book deltas** — those are removals, not noise.
- **Add new fields to runtime structs for persistence** — use the `Persisted*` serde layer with `#[serde(default)]`.

