# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

---

## [Unreleased]

### Changed
- `order_executor`: added exponential backoff retry loop (up to 4 attempts) to
  `submit_order` for transient Polymarket API errors (HTTP 429, 5xx, and
  `success=false` where `errorMsg` contains `"transient"`).
- Auth headers (`POLY-TIMESTAMP`) are now rebuilt fresh on every retry attempt
  to prevent stale-timestamp authentication failures.
- `live_engine`: fixed missing `TimeInForce::Gtc` argument on `submit_order`
  call — was a compile error introduced when `TimeInForce` was added to the
  function signature.

---

## [0.2.0] — Initial live-trading release

### Added
- **Live trading engine** (`live_engine.rs`): full pipeline from RN1 signal to
  signed CLOB order submission. Mirrors paper engine logic but submits real
  orders via `OrderExecutor`.
- **EIP-712 order signing** (`order_signer.rs`): manual Keccak256/secp256k1
  implementation — no `alloy` dependency. Signs orders for Polymarket CTF
  Exchange (Polygon chain ID 137).
- **Order executor** (`order_executor.rs`): HMAC-SHA256 authenticated REST
  client for `POST /order`, `DELETE /order/{id}`,
  `DELETE /orders/market/{id}`, `GET /order/{id}`.
- **Risk manager** (`risk_manager.rs`): pre-order kill switch, circuit breaker,
  daily loss limit, concurrent position cap, single-order size cap, per-second
  rate limiter.
- **Game-start watcher** (`game_start_watcher.rs`): polls CLOB prices every
  500 ms to detect in-play transitions; fires a wipe signal to cancel open
  orders.
- `LIVE_TRADING`, `SIGNER_PRIVATE_KEY`, `POLYMARKET_FUNDER_ADDRESS`,
  `POLYMARKET_API_KEY`, `POLYMARKET_API_SECRET`, `POLYMARKET_API_PASSPHRASE`
  env vars added to `Config`.
- `TimeInForce` enum added to `types.rs` (`Gtc`, `Fok`, `Fak`).

### Changed
- `submit_order` now requires a `TimeInForce` parameter (breaking change from
  Phase 1 API).

---

## [0.1.0] — Paper trading release (Phase 1)

### Added
- **WebSocket client** (`ws_client.rs`): persistent connection to Polymarket
  live-activity feed with automatic exponential-backoff reconnection
  (100 ms → 30 s ceiling).
- **Order book store** (`order_book.rs`): thread-safe multi-market CLOB book
  backed by `BTreeMap` (per market) + `DashMap` (across markets). Supports
  delta updates and full snapshots.
- **Sniffer** (`sniffer.rs`): case-insensitive RN1 wallet filter; emits
  `RN1Signal` on matching `"order"` WebSocket events.
- **Paper engine** (`paper_engine.rs`): signal → size → risk check → 3-second
  fill window → virtual fill. Full P&L tracking.
- **Paper portfolio** (`paper_portfolio.rs`): $100 virtual USDC, open
  positions, closed trades, P&L aggregates.
- **TUI dashboard** (`tui_app.rs`): ratatui terminal UI with live portfolio,
  order book, activity log, and latency stats panels.
- **Activity log** (`activity_log.rs`): 200-entry ring buffer for TUI display.
- **Latency tracker** (`latency_tracker.rs`): rolling min/max/avg/p99 in µs.
- **CLOB client** (`clob_client.rs`): read-only REST client for price, order
  book, and market endpoints.
- **Market scanner** (`market-scanner`): CLI tool to discover top sports
  markets by 24h volume using the Gamma API.
- All prices stored as `u64 × 1000` to eliminate floating-point in hot path.
- `parse_price` / `format_price` utilities in `types.rs`.
- Comprehensive unit test suites across all modules.
