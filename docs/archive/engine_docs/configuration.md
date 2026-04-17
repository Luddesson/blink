# Configuration Reference

All configuration is loaded from environment variables. Copy `.env.example` to `.env` and fill in your values.

Load order: `.env` file (via `dotenvy`) → process environment. Process environment overrides `.env`.

---

## Core (always required)

| Variable | Type | Description | Example |
|----------|------|-------------|---------|
| `CLOB_HOST` | URL | Polymarket CLOB REST API base URL | `https://clob.polymarket.com` |
| `WS_URL` | URL | Polymarket WebSocket feed URL | `wss://ws-live-data.polymarket.com` |
| `RN1_WALLET` | hex string | Ethereum wallet address of the RN1 target to track. Case-insensitive (normalised to lowercase internally). | `0xabcdef...` |
| `MARKETS` | comma-list | One or more Polymarket token IDs to subscribe to. Use `cargo run -p market-scanner` to discover IDs. | `12345,67890` |

---

## Operating mode flags

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `PAPER_TRADING` | bool | `false` | Simulate orders with $100 virtual USDC. No network requests to CLOB. |
| `TUI` | bool | `false` | Enable the ratatui terminal dashboard. Requires `PAPER_TRADING=true`. When active, tracing output is redirected to `logs/engine.log`. |
| `LIVE_TRADING` | bool | `false` | Submit real orders via Polymarket CLOB REST API. All credential vars below must be set. |

Bool values accept `true` / `1` (case-insensitive). Any other value, or absence, is treated as `false`.

---

## Live trading credentials

Required only when `LIVE_TRADING=true`. The engine exits at startup with a clear error message if any of these is missing.

| Variable | Description |
|----------|-------------|
| `SIGNER_PRIVATE_KEY` | 64-character hex secp256k1 private key (with or without `0x` prefix). Used to produce the EIP-712 signature for each order. |
| `POLYMARKET_FUNDER_ADDRESS` | Your Polymarket funder/proxy-wallet address (`0x...`). This is the `maker` field in every order and the address that funds are debited from. |
| `POLYMARKET_API_KEY` | Polymarket L2 API key. Obtain via `createOrDeriveApiKey` from the Polymarket SDK. |
| `POLYMARKET_API_SECRET` | Polymarket L2 API secret. **Base64-encoded** — do not decode before setting. |
| `POLYMARKET_API_PASSPHRASE` | Polymarket L2 API passphrase. |

---

## Risk management

These values are read by `RiskConfig::from_env()`. If a variable is missing or unparseable, the safe default is used.

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `TRADING_ENABLED` | bool | `false` | Master kill switch. Must be explicitly `true` before any order can be submitted. This is separate from `LIVE_TRADING` — both must be true for orders to go through. |
| `MAX_DAILY_LOSS_PCT` | f64 | `0.10` | Maximum cumulative daily loss as a fraction of starting NAV (e.g. `0.10` = 10%). When exceeded, the circuit breaker trips automatically and blocks all further orders until the engine is restarted. |
| `MAX_CONCURRENT_POSITIONS` | usize | `5` | Maximum number of simultaneously open positions. Set `0` for unlimited. |
| `MAX_SINGLE_ORDER_USDC` | f64 | `20.0` | Hard cap per individual order in USDC. Orders sized above this are rejected before submission. |
| `MAX_ORDERS_PER_SECOND` | u32 | `3` | Per-second rate limit. Uses a sliding 1-second window. Prevents accidental burst submissions. |

### Kill switch vs circuit breaker

- **Kill switch** (`TRADING_ENABLED`): set once at startup in `.env`. Flip to `false` and restart to disable all trading immediately.
- **Circuit breaker**: tripped automatically by the risk manager when `MAX_DAILY_LOSS_PCT` is exceeded. Also trippable manually via `RiskManager::trip_circuit_breaker()`. Cannot be reset without restarting the engine.

---

## Game-start watcher

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `GAME_WATCHER_INTERVAL_MS` | u64 | `500` | How frequently (milliseconds) the watcher polls CLOB prices. Lower values detect in-play transitions faster but increase API request volume. |

Detection logic: if `GET /price` returns an error, or if both `BUY` and `SELL` prices are `0.0`, the market is considered to have gone in-play and a `GameStartSignal` is fired to cancel open orders for that market.

---

## Kernel telemetry (eBPF)

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `EBPF_TELEMETRY` | bool | `true` (auto-attach attempt) | Controls whether the engine should attempt to attach kernel telemetry probes. |

### Behavior by platform

- **Windows/macOS**: graceful no-op stub; TUI shows `eBPF: N/A`.
- **Linux without ebpf feature**: graceful no-op stub; TUI shows `eBPF: N/A`.
- **Linux with ebpf feature**: full kernel telemetry (RTT/scheduler/syscalls).

### Linux production build

```bash
cargo build -p engine --release --features bpf-probes/ebpf-telemetry
```

If you want to disable attach attempts explicitly (even on Linux):

```dotenv
EBPF_TELEMETRY=false
```

---

## Logging

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LOG_LEVEL` | string | `info` | Tracing filter directive. Passed to `tracing_subscriber::EnvFilter`. |

Examples:

```bash
LOG_LEVEL=debug                    # all debug output
LOG_LEVEL=engine=debug,warn        # debug for engine crate, warn for everything else
LOG_LEVEL=trace                    # extremely verbose (includes WS frame parsing)
LOG_LEVEL=engine::order_executor=debug,info  # debug only for order_executor
```

When `TUI=true`, all log output is redirected to `logs/engine.log` (ANSI codes disabled) to avoid corrupting the terminal dashboard.

---

## Agent RPC (JSON-RPC 2.0)

Optional local control/status endpoint for orchestrator agents.

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `AGENT_RPC_ENABLED` | bool | `false` | Enables the embedded JSON-RPC server over HTTP. |
| `AGENT_RPC_BIND` | host:port | `127.0.0.1:7878` | Bind address for the RPC listener. |

### Endpoint

- `POST http://<AGENT_RPC_BIND>/rpc`
- Body: JSON-RPC 2.0 payload

### Methods

- `blink_status`
  - Returns WS status, pause flag, message counters, risk status, subscriptions, and paper summary when paper mode is active.

- `paper_summary`
  - Returns paper KPIs: NAV, open/closed positions, fill/reject rates, slippage, queue delay, realism gap.
  - Returns JSON-RPC error if paper mode is not active.

- `set_pause`
  - Params: `{"paused": true|false}`
  - Sets runtime pause/resume without restarting engine.

### Example calls

```bash
curl -s http://127.0.0.1:7878/rpc ^
  -H "Content-Type: application/json" ^
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"blink_status\",\"params\":{}}"
```

```bash
curl -s http://127.0.0.1:7878/rpc ^
  -H "Content-Type: application/json" ^
  -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"set_pause\",\"params\":{\"paused\":true}}"
```

---

## Example `.env` for paper trading

```dotenv
CLOB_HOST=https://clob.polymarket.com
WS_URL=wss://ws-live-data.polymarket.com
RN1_WALLET=0xabcdef1234567890abcdef1234567890abcdef12
MARKETS=71321045679252212594626385532706912750332728571942532289631379312455583992563

PAPER_TRADING=true
TUI=true
AGENT_RPC_ENABLED=true
AGENT_RPC_BIND=127.0.0.1:7878

TRADING_ENABLED=false
LOG_LEVEL=info
```

## Example `.env` for live trading

```dotenv
CLOB_HOST=https://clob.polymarket.com
WS_URL=wss://ws-live-data.polymarket.com
RN1_WALLET=0xabcdef1234567890abcdef1234567890abcdef12
MARKETS=71321045679252212594626385532706912750332728571942532289631379312455583992563

LIVE_TRADING=true
SIGNER_PRIVATE_KEY=abcdef01234567890abcdef01234567890abcdef01234567890abcdef01234567
POLYMARKET_FUNDER_ADDRESS=0xYourFunderAddress
POLYMARKET_API_KEY=your-api-key
POLYMARKET_API_SECRET=your-base64-secret
POLYMARKET_API_PASSPHRASE=your-passphrase

TRADING_ENABLED=true
MAX_DAILY_LOSS_PCT=0.05
MAX_CONCURRENT_POSITIONS=3
MAX_SINGLE_ORDER_USDC=10.0
MAX_ORDERS_PER_SECOND=2
LOG_LEVEL=info
```
