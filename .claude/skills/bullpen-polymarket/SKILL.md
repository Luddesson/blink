# Bullpen Polymarket Skill

> Access Polymarket prediction market data and trading via the Bullpen CLI.

## Overview

This skill wraps [Bullpen CLI](https://cli.bullpen.fi/) commands for use by Blink agents.
All commands support `--output json` for structured data. On Windows, commands run
through WSL2 (`wsl -d Ubuntu -- bullpen ...`).

## Prerequisites

- Bullpen CLI installed (`npm install -g @bullpenfi/cli` on Linux/macOS)
- Authenticated (`bullpen login`)
- Config at `~/.bullpen/config.toml`

## Commands

### Market Discovery

```bash
# Discover markets via 7 lenses: all, sports, crypto, traders, walletscope, flow, eventscope
bullpen polymarket discover <lens> --output json

# Detailed market info by slug
bullpen polymarket market <slug> --output json

# List active markets
bullpen polymarket markets --active --output json
```

### Smart Money Intelligence

```bash
# Smart money signals (top_traders | new_wallet | aggregated)
bullpen polymarket data smart-money <type> --output json

# Trader profile by wallet address (volume, win rate, P&L, specialization)
bullpen polymarket data profile <wallet_address> --output json

# Filtered trade feed (high P&L trades only)
bullpen polymarket feed trades --min-pnl <amount> --output json
```

### Prices & CLOB Data

```bash
# Real-time bid/ask/mid for a market
bullpen polymarket price <slug> --output json

# Full order book
bullpen polymarket clob book --token <token_id> --output json

# Midpoint price
bullpen polymarket clob midpoint --token <token_id> --output json

# Bid-ask spread
bullpen polymarket clob spread --token <token_id> --output json
```

### Portfolio & Orders

```bash
# Open positions with P&L
bullpen polymarket positions --output json

# Account balance
bullpen polymarket balance --output json

# Open orders
bullpen polymarket orders --output json

# Cancel all orders (EMERGENCY)
bullpen polymarket orders --cancel-all --yes --output json
```

### Wallet Tracking

```bash
# Add wallet to tracker
bullpen tracker add <address> --label <name>

# Recent trades from tracked wallets
bullpen tracker trades --output json

# Real-time wallet follow
bullpen tracker follow <address> --output json
```

### Trading (Use with caution — real money)

```bash
# Market buy
bullpen polymarket buy <slug> --amount <usdc> --outcome <name> --yes --output json

# Market sell
bullpen polymarket sell <slug> --shares <n> --outcome <name> --yes --output json

# Limit orders
bullpen polymarket limit-buy <slug> --amount <usdc> --price <price> --outcome <name> --yes
bullpen polymarket limit-sell <slug> --shares <n> --price <price> --outcome <name> --yes

# Redeem resolved positions (gasless)
bullpen polymarket redeem --yes --output json
```

## Agent Use Cases

| Agent | Primary Commands | Purpose |
|-------|-----------------|---------|
| **qsigma-quant** | `price`, `discover`, `smart-money` | Quantitative analysis, market scanning |
| **sentinel-risk** | `positions`, `balance`, `price` | Portfolio health, reconciliation |
| **aura-architect** | `discover`, `data smart-money` | Strategic market selection |
| **wraith-stealth** | `clob book`, `clob spread`, `price` | Execution optimization |
| **nexus-automator** | `discover`, `tracker trades` | Automated discovery, wallet tracking |

## Important Notes

- **Latency**: 2-10 seconds per command (cold path only, never on hot signal path)
- **Rate limit**: ~1 command per second recommended
- **Always** include `--yes` flag for non-interactive automation
- **Always** include `--output json` for structured responses
- **Windows**: Use `wsl -d Ubuntu -- bullpen` prefix
- **Auth expiry**: Run `bullpen login` if commands return auth errors
