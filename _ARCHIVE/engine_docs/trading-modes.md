# Trading Modes

Blink Engine supports three operating modes controlled by environment variables. Each mode runs the full signal detection pipeline but differs in what happens when a signal is received.

---

## Mode 1: Read-only (default)

**Env:** no `PAPER_TRADING` or `LIVE_TRADING` set.

```dotenv
# (nothing — read-only is the default)
```

**What happens:**
1. Connects to Polymarket WebSocket feed
2. Maintains live order books for all configured markets
3. Detects RN1 orders and logs them as `WARN` tracing events
4. Does **nothing else** — no orders, no portfolio tracking

**Use this for:**
- Validating that your `RN1_WALLET` address is correct before committing funds
- Monitoring RN1 activity across different markets
- Watching the CLOB feed without risk

**Console output:**
```
WARN engine::sniffer: 🚨 RN1 order detected  token_id=12345…  side=BUY  price=0.65  size=50000
WARN engine::main: RN1 signal — read-only mode  token_id=12345…  side=BUY  price=0.650  size=50000.000
```

---

## Mode 2: Paper trading

**Env:**
```dotenv
PAPER_TRADING=true
TUI=false   # or true for terminal dashboard
```

**What happens:**
1. All of read-only mode
2. For each RN1 signal:
   - **Size** the mirror order: `2% × RN1_notional`, capped at `10% of NAV` and available cash
   - **Risk check**: same checks as live mode (kill switch, circuit breaker, daily loss, position count, rate)
   - **Fill window** (3 seconds): polls the order book every 500 ms; aborts if price drifts >1.5%
   - **Virtual fill**: records the position in the paper portfolio
3. Tracks cash, open positions, and P&L in memory

**No network calls to the CLOB REST API are made.**

### Sizing logic

```
raw_size      = RN1_notional_usdc × SIZE_MULTIPLIER (0.02)
nav_cap       = current_NAV × MAX_POSITION_PCT (0.10)
size_usdc     = min(raw_size, nav_cap, cash_remaining)
```

If `size_usdc < MIN_TRADE_USDC ($0.50)`, the signal is skipped.

Starting portfolio: **$100.00 USDC** virtual cash.

### Dashboard (text, no TUI)

Every 60 seconds (and after each fill), the paper engine prints:

```
╔════════════════════════════════════════════════════════════╗
║            📄  BLINK PAPER TRADING DASHBOARD              ║
╠════════════════════════════════════════════════════════════╣
║  Cash:             $95.00     USDC                        ║
║  Invested:         $5.00      USDC                        ║
║  Unrealized P&L:   +0.1234 USDC                          ║
║  Realized P&L:     +0.0000 USDC                          ║
║  ─────────────────────────────────────────────────────    ║
║  NAV:              $100.12 (+0.12%)                       ║
╠════════════════════════════════════════════════════════════╣
║  Signals:   3  │  Filled:   1  │  Aborted:   1  │  Skipped:   1  ║
╚════════════════════════════════════════════════════════════╝
```

### TUI dashboard

Set `TUI=true` (requires `PAPER_TRADING=true`) for the full ratatui terminal UI:

```dotenv
PAPER_TRADING=true
TUI=true
```

The TUI shows live panels for:
- Portfolio (NAV, cash, P&L, position table)
- Order book (best bid/ask, spread)
- Activity log (last 200 events with timestamps and colour-coded severity)
- Latency stats (min/avg/max/p99 µs from signal detection to consume)
- WebSocket status and message throughput

**When TUI is active**, all `tracing` output is redirected to `logs/engine.log` to avoid corrupting the terminal display.

Press `q` to quit the TUI cleanly.

If `AGENT_RPC_ENABLED=true`, paper mode can also be monitored and paused/resumed via JSON-RPC (`blink_status`, `paper_summary`, `set_pause`) without interacting with the TUI.

---

## Mode 3: Live trading

**Env:**
```dotenv
LIVE_TRADING=true
TRADING_ENABLED=true   # risk manager kill switch

SIGNER_PRIVATE_KEY=<64-char hex>
POLYMARKET_FUNDER_ADDRESS=0x...
POLYMARKET_API_KEY=...
POLYMARKET_API_SECRET=...    # base64-encoded
POLYMARKET_API_PASSPHRASE=...
```

**What happens:**
1. All of paper mode's signal pipeline
2. Signs orders using EIP-712 (secp256k1 / `k256` crate)
3. Submits orders as post-only maker orders via `POST /order` on the CLOB REST API
4. Records virtual fills in the portfolio (for tracking purposes, even though the real fill is on-chain)

### Why `TRADING_ENABLED` is separate from `LIVE_TRADING`

`LIVE_TRADING=true` enables the live code path (building, signing, and submitting orders).  
`TRADING_ENABLED=true` is the risk manager's kill switch.

Both must be `true` for real orders to be submitted. This two-layer approach means you can:
- Set `LIVE_TRADING=true` in your `.env` permanently
- Use `TRADING_ENABLED` as the operational on/off switch
- Quickly flip `TRADING_ENABLED=false` + restart to halt all trading without touching credentials

### Order type

All orders are submitted as **post-only (maker) GTC (Good Till Cancelled)** orders. The `maker: true` flag in the order body is hardcoded to prevent paying taker fees.

### Retry policy

`submit_order` retries transient failures with exponential backoff:

| Attempt | Delay | Trigger |
|---------|-------|---------|
| 1 | — | Initial attempt |
| 2 | 200 ms | HTTP 429, 5xx, or `"transient"` error |
| 3 | 400 ms | Same |
| 4 | 800 ms | Same |

After 4 failed attempts, the error is propagated up and logged. The order is not retried further.

Non-retryable errors (HTTP 4xx except 429, exchange rejections without "transient") fail immediately.

### Dry-run fallback

When `LIVE_TRADING=true` but `SIGNER_PRIVATE_KEY` is empty or the private key fails to load, the engine logs `DRY-RUN` and skips actual submission. This prevents silent failures if credentials are misconfigured.

---

## Comparison table

| Feature | Read-only | Paper | Live |
|---------|-----------|-------|------|
| WebSocket connection | ✓ | ✓ | ✓ |
| Order book maintenance | ✓ | ✓ | ✓ |
| RN1 detection | ✓ | ✓ | ✓ |
| Order sizing | ✗ | ✓ | ✓ |
| Risk checks | ✗ | ✓ | ✓ |
| Fill window (3s poll) | ✗ | ✓ | ✓ |
| EIP-712 signing | ✗ | ✗ | ✓ |
| REST API submission | ✗ | ✗ | ✓ |
| Portfolio tracking | ✗ | ✓ | ✓ |
| TUI dashboard | ✗ | ✓ | ✗ |
| Real funds at risk | ✗ | ✗ | ✓ |

---

## Switching modes at runtime

Modes are determined at startup from env vars. To switch:

1. Stop the engine (`Ctrl-C` or `q` in TUI)
2. Edit `.env`
3. Restart: `cargo run -p engine`

---

## Recommended progression

1. **Read-only** first — confirm RN1 wallet detection is working, observe signal frequency
2. **Paper trading** for several days — validate sizing, risk checks, and P&L simulation
3. **Paper + TUI** — monitor live dashboard and latency stats
4. **Live with minimal capital** — start with `MAX_SINGLE_ORDER_USDC=5.0` and `MAX_DAILY_LOSS_PCT=0.05`
5. **Scale up** — increase limits only after validating consistent paper P&L
