# Architecture

This document describes the internal structure of Blink Engine — how data flows from the Polymarket WebSocket feed through to order submission (or virtual fill).

---

## High-Level Overview

Blink Engine is a single Tokio-based async process split into loosely coupled tasks that communicate via channels and shared atomic state.

```
┌──────────────────────────────────────────────────────────────────────────┐
│                            Blink Engine process                           │
│                                                                          │
│  ┌──────────────────────┐    ┌─────────────────────────────────────────┐ │
│  │    Tokio task: WS    │    │       Tokio task: Signal Consumer       │ │
│  │                      │    │                                         │ │
│  │  ws_client::run_ws   │    │  (paper_engine or live_engine or        │ │
│  │                      │    │   read-only warn logger)                │ │
│  │  ┌──────────────┐    │    │                                         │ │
│  │  │ connect_and  │    │    │  ┌───────────────────────────────────┐  │ │
│  │  │    _run()    │    │    │  │ handle_signal(RN1Signal)          │  │ │
│  │  │              │    │    │  │                                   │  │ │
│  │  │ ┌──────────┐ │    │    │  │ 1. calculate_size_usdc()         │  │ │
│  │  │ │OrderBook │ │    │    │  │ 2. risk_manager.check_pre_order() │  │ │
│  │  │ │ .apply_  │ │    │    │  │ 3. check_fill_window() [3s]      │  │ │
│  │  │ │ update() │ │    │    │  │ 4. sign_order() [live only]      │  │ │
│  │  │ └──────────┘ │    │    │  │ 5. submit_order() [live only]    │  │ │
│  │  │              │    │    │  │ 6. open_position() [portfolio]   │  │ │
│  │  │ ┌──────────┐ │    │    │  └───────────────────────────────────┘  │ │
│  │  │ │ Sniffer  │─┼────┼───▶│                                         │ │
│  │  │ │.check_   │ │    │    │           crossbeam channel             │ │
│  │  │ │order_    │ │ RN1Signal bound(1024)                             │ │
│  │  │ │event()   │ │    │    └─────────────────────────────────────────┘ │
│  │  │ └──────────┘ │    │                                                │
│  │  └──────────────┘    │    ┌─────────────────────────────────────────┐ │
│  └──────────────────────┘    │  OS thread: TUI (optional)              │ │
│                              │  tui_app::run_tui()                     │ │
│  ┌──────────────────────┐    │  reads: portfolio, book_store, activity  │ │
│  │  Tokio task: Ctrl-C  │    └─────────────────────────────────────────┘ │
│  │  sets shutdown flag  │                                                │
│  └──────────────────────┘    ┌─────────────────────────────────────────┐ │
│                              │  Tokio task: Dashboard printer          │ │
│                              │  (paper mode, no TUI only)              │ │
│                              │  prints every 60 s                      │ │
│                              └─────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────┘
```

### Agent RPC control plane

When `AGENT_RPC_ENABLED=true`, an additional Tokio task runs a lightweight local JSON-RPC 2.0 server on `AGENT_RPC_BIND` (default `127.0.0.1:7878`).

- Transport: HTTP `POST /rpc`
- Purpose: machine-to-machine observability and runtime pause control for AI agents/orchestrators
- Methods:
  - `blink_status`
  - `paper_summary`
  - `set_pause`

The server is read-mostly and shares existing engine state via `Arc` handles (no additional persistence layer).

---

## Module Dependency Graph

```
main.rs
├── config          (Config::from_env)
├── ws_client       (run_ws)
│   ├── order_book  (OrderBookStore)
│   ├── sniffer     (Sniffer)
│   └── types       (MarketEvent, RN1Signal)
├── paper_engine    (PaperEngine::handle_signal)
│   ├── paper_portfolio
│   ├── order_book
│   ├── risk_manager
│   └── types
├── live_engine     (LiveEngine::handle_signal)
│   ├── paper_portfolio
│   ├── order_book
│   ├── order_executor  ──▶ Polymarket CLOB REST API
│   ├── order_signer
│   ├── risk_manager
│   └── types
├── clob_client     (ClobClient — read-only REST)
├── tui_app         (run_tui — optional)
│   ├── activity_log
│   ├── latency_tracker
│   └── paper_portfolio
└── activity_log
```

---

## Threading Model

| Component | Thread / Task | Sync primitive |
|-----------|--------------|----------------|
| WebSocket client | Tokio task | — |
| Signal consumer (paper/live) | `tokio::task::spawn_blocking` | crossbeam channel |
| TUI | `std::thread` (blocking) | `Arc<Mutex<...>>` |
| Dashboard printer | Tokio task | `Arc<tokio::sync::Mutex<...>>` |
| Ctrl-C handler | Tokio task | `Arc<AtomicBool>` |
| Risk manager | shared by signal consumer | `std::sync::Mutex` |
| Kernel telemetry | Tokio task (if available) | `Arc<Mutex<KernelSnapshot>>` |

The WebSocket task and TUI run concurrently. The signal consumer is on a blocking thread (via `spawn_blocking`) to avoid blocking the Tokio runtime during the 3-second fill window sleep.

Kernel telemetry attach is now attempted by default; unsupported environments degrade gracefully to a stub snapshot (`available=false`) so the same TUI code path works on Windows development and Linux production.

---

## Order Book

`OrderBook` is a per-market structure:

```rust
pub struct OrderBook {
    bids: BTreeMap<u64, u64>,  // price×1000 → size×1000
    asks: BTreeMap<u64, u64>,
}
```

`BTreeMap` keeps keys sorted automatically:
- `best_bid()` = `bids.keys().next_back()` — O(log n)
- `best_ask()` = `asks.keys().next()` — O(log n)

`OrderBookStore` wraps a `DashMap<String, OrderBook>` for lock-free concurrent multi-market access.

Delta updates: a `size == 0` entry removes the level; any other size upserts it. This matches the Polymarket WebSocket delta protocol.

---

## Price Scaling

All prices and sizes use `u64 × 1000` to avoid floating-point in the hot path:

```
"0.65"  → 650
"1.000" → 1_000
"50000" → 50_000_000
```

`parse_price(s: &str) -> u64` handles up to 3 decimal digits (truncates, does not round).  
`format_price(p: u64) -> String` formats back to `"0.650"`.

---

## Signal Pipeline

```
WebSocket "order" event
         │
         ▼
  Sniffer::check_order_event()
  ├── event is not "order"? → None
  ├── owner != rn1_wallet?  → None
  └── match → Some(RN1Signal { token_id, side, price×1000, size×1000, order_id, detected_at })
         │
         ▼
  crossbeam_channel::try_send(signal)
  (bounded, capacity 1024; drops if full and logs warn)
         │
         ▼
  Engine::handle_signal(signal)
  1. calculate_size_usdc(rn1_notional)
     └── 2% × notional, capped at 10% NAV and cash remaining
  2. risk_manager.check_pre_order(size, positions, nav, start_nav)
     ├── kill switch off?          → Err(KillSwitchOff)
     ├── circuit breaker tripped?  → Err(CircuitBreakerTripped)
     ├── daily_pnl < -limit?       → Err(DailyLossLimitExceeded) + trip breaker
     ├── positions >= max?         → Err(TooManyPositions)
     ├── size > max_single_order?  → Err(OrderTooLarge)
     └── rate > max/sec?           → Err(RateLimitExceeded)
  3. check_fill_window(token_id, entry_price)
     └── 6 × 500 ms polls; abort if mid_price drifts > 1.5%
  4. [live only] sign_order(private_key, params) → SignedOrder (EIP-712)
  5. [live only] submit_order(signed, GTC) → OrderResponse
     └── retry loop: up to 4 attempts, backoff 200/400/800 ms
  6. open_position(token_id, side, price, size, order_id)
```

---

## EIP-712 Signing

`order_signer::sign_order` implements Polymarket's EIP-712 order format manually:

- **Domain:** `"Polymarket CTF Exchange"`, version `"1"`, chain ID `137` (Polygon), verifying contract `0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E`
- **Order struct:** 13 fields including `salt`, `maker`, `signer`, `taker`, `tokenId`, `makerAmount`, `takerAmount`, `side`, `signatureType`
- **Hash:** `keccak256(0x1901 || domainSeparator || orderHash)`
- **Signature:** `k256` ECDSA, 65 bytes (r + s + v), `v = recovery_id + 27`

No `alloy` or `ethers` dependency — only `k256` and `sha3`.

---

## REST API Authentication

Every mutating CLOB request requires Polymarket L2 headers:

```
POLY-ADDRESS:    <funder wallet>
POLY-SIGNATURE:  base64(HMAC-SHA256(secret, timestamp + METHOD + path + body))
POLY-TIMESTAMP:  <unix seconds>
POLY-NONCE:      0
POLY-API-KEY:    <api key>
POLY-PASSPHRASE: <passphrase>
```

The `api_secret` is stored as base64 and decoded to raw bytes before HMAC use. Headers are rebuilt on every retry attempt since `POLY-TIMESTAMP` must be current.

---

## Reconnection Strategy

`ws_client` uses exponential backoff for WebSocket reconnections:

```
initial_backoff = 100 ms
max_backoff     = 30 s
multiplier      = 2×

On clean server close → reconnect immediately (reset to initial_backoff)
On error             → sleep(backoff); backoff = min(backoff × 2, max_backoff)
```

A 30-second heartbeat ping is sent to keep the connection alive.

---

## Workspace Structure

```
blink-engine/
├── Cargo.toml              Workspace manifest + shared dependencies
├── Cargo.lock
├── .env.example            Template for environment configuration
├── .gitignore
├── README.md
├── CHANGELOG.md
├── docs/
│   ├── architecture.md     (this file)
│   ├── configuration.md    All environment variables
│   └── trading-modes.md    Mode guide: read-only / paper / live
└── crates/
    ├── engine/             Main trading engine crate
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs      Module declarations
    │       ├── main.rs     Entry point + task wiring
    │       ├── types.rs
    │       ├── config.rs
    │       ├── ws_client.rs
    │       ├── order_book.rs
    │       ├── sniffer.rs
    │       ├── paper_engine.rs
    │       ├── paper_portfolio.rs
    │       ├── live_engine.rs
    │       ├── order_signer.rs
    │       ├── order_executor.rs
    │       ├── clob_client.rs
    │       ├── risk_manager.rs
    │       ├── activity_log.rs
    │       ├── latency_tracker.rs
    │       ├── tui_app.rs
    │       └── game_start_watcher.rs
    └── market-scanner/     Market discovery CLI
        ├── Cargo.toml
        └── src/
            └── main.rs
```
