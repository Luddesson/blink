---
name: blink-trader
description: >
  Control and monitor the Blink high-frequency trading engine for Polymarket prediction markets.
  Use to query portfolio P&L, open/closed positions, engine status, market prices, order books,
  and to place or cancel orders — all via the `blink` CLI. Understands both paper and live mode.
  Use when the user asks about their Blink engine, trading positions, market data, or wants to
  execute trades on Polymarket through the Blink engine.
---

# Blink Trader Skill

## Overview

This skill lets Copilot control the Blink engine — a high-frequency trading daemon for
Polymarket prediction markets — using the `blink` CLI (`blink-engine/crates/blink-cli`).

The `blink` binary communicates with the running engine via its REST API
(`BLINK_HOST`, default `http://localhost:3030`) and directly with Polymarket APIs for
market data.

## Setup Requirements

1. The Blink engine must be running (start with `.\start-blink.ps1` from the repo root).
2. The `blink` binary must be built: `cd blink-engine && cargo build -p blink-cli`.
3. Optionally set `BLINK_HOST` if the engine runs on a non-default port.

## Available Commands

### Portfolio

```bash
# Open positions with unrealised P&L
blink portfolio positions

# Cash balance, NAV, fill rate, win rate
blink portfolio balances

# Last N closed trades and realised P&L (default 20)
blink portfolio pnl
blink portfolio pnl --limit 50

# JSON output for scripting
blink portfolio balances --output json
```

### Markets

```bash
# Discover trending prediction markets (default lens: all)
blink market discover

# Discover crypto markets sorted by 24h volume
blink market discover crypto --sort volume_24h

# Search for a topic
blink market discover --search "bitcoin" --min-liquidity 50000

# Real-time price (bid/ask/mid/spread) for a token
blink market price <TOKEN_ID>

# Order book snapshot
blink market book <TOKEN_ID>

# Recent trades on a market
blink market trades <TOKEN_ID> --limit 30

# Price history with ASCII sparkline
blink market history <TOKEN_ID> --interval 1d --fidelity 100

# Search markets by keyword
blink market search "trump election"
```

### Orders

```bash
# Market buy (shows preview; requires --yes to execute)
blink order buy <TOKEN_ID> "Yes" 25.00 --yes

# Market sell
blink order sell <TOKEN_ID> "Yes" 50.00 --yes

# Limit buy at a specific price
blink order limit-buy <TOKEN_ID> "Yes" --price 0.45 --shares 100 --yes

# Limit sell (GTC, FOK, FAK)
blink order limit-sell <TOKEN_ID> "Yes" --price 0.68 --shares 50 --expiration gtc --yes

# List open positions/orders
blink order list

# Cancel a specific order
blink order cancel <ORDER_ID>

# Cancel all open orders
blink order cancel-all --yes
```

### Engine Control

```bash
# Engine status: WebSocket, trading state, risk, subscriptions
blink engine status

# Pause order execution (engine keeps running, no new orders)
blink engine pause

# Resume order execution
blink engine resume

# Risk manager metrics
blink engine risk

# Latency percentiles (signal → order)
blink engine latency
```

## Workflow Examples

### "What's my current NAV and positions?"
```bash
blink portfolio balances
blink portfolio positions
```

### "Show me trending crypto prediction markets with high liquidity"
```bash
blink market discover crypto --min-liquidity 100000 --sort volume_24h
```

### "Pause the engine and cancel all orders"
```bash
blink engine pause
blink order cancel-all --yes
```

### "Buy $50 of Yes on a specific market"
1. First check the price: `blink market price <TOKEN_ID>`
2. Then buy: `blink order buy <TOKEN_ID> "Yes" 50.00 --yes`

### "Show my P&L for the last 10 closed trades"
```bash
blink portfolio pnl --limit 10
```

### "Is the engine healthy? What's the WebSocket and risk status?"
```bash
blink engine status
blink engine risk
```

## Token IDs vs Slugs

- Token IDs are hex strings (e.g. `0x1234...abcd`) used by the Polymarket CLOB
- Use `blink market discover` or `blink market search` to find markets and their token IDs
- Market slugs (e.g. `will-btc-hit-100k`) can be used in the `discover` search filter

## Output Formats

All commands default to a human-friendly table. Add `--output json` for scripting:

```bash
blink portfolio positions --output json | python -m json.tool
blink market discover --output json
```

## Environment Variables

| Variable              | Default                    | Description                        |
|-----------------------|----------------------------|------------------------------------|
| `BLINK_HOST`          | `http://localhost:3030`    | Blink engine REST API base URL     |
| `SMART_MONEY_ENABLED` | `false`                    | Enable smart money signal poller   |
| `SMART_MONEY_TOP_N`   | `20`                       | Number of top wallets to track     |
| `SMART_MONEY_MIN_TRADE_USD` | `500`               | Min trade size to emit signal (USD)|
| `DISCOVERY_ENABLED`   | `false`                    | Enable automatic market discovery  |
| `DISCOVERY_LENS`      | `all`                      | Comma-separated lens list          |
| `DISCOVERY_MIN_LIQUIDITY` | `10000`                | Min liquidity filter (USD)         |

## Binary Location

After building:
- Windows: `blink-engine\target\debug\blink.exe`
- Linux/macOS: `blink-engine/target/debug/blink`

For the release build (faster): `cargo build --release -p blink-cli`
Release binary: `blink-engine\target\release\blink(.exe)`
