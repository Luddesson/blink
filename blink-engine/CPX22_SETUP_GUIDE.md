# Blink Engine — CPX22 Setup Guide
## 24h Paper Trading Soak Test

**Server**: Hetzner CPX22 (3 vCPU, 4 GB RAM, 80 GB NVMe)  
**OS**: Ubuntu 22.04 LTS  
**Mode**: Paper trading — zero real money  
**Time to set up**: ~25 minutes  

---

## Before You Start

You need two things:
1. SSH access to your Hetzner server
2. A Polymarket wallet address to track (the RN1 wallet)

If you don't know which wallet to track yet, you can use any active wallet as a placeholder to test the feed.

---

## Step 1 — SSH Into Your Server

From your terminal:

```bash
ssh root@YOUR_SERVER_IP
```

If you set up an SSH key in Hetzner, it will connect automatically. If not, Hetzner will have sent you a password by email.

Once in, confirm the OS:

```bash
lsb_release -a
# Should show: Ubuntu 22.04.x LTS
```

Check available resources:

```bash
nproc        # Should show: 3
free -h      # Should show: ~3.8 GB RAM
df -h /      # Should show: ~75 GB free
```

---

## Step 2 — Download the Setup Script

```bash
curl -fsSL \
  https://raw.githubusercontent.com/Luddesson/blink/claude/trade-bot-live-ready-6zaZf/blink-engine/infra/provision-cpx22.sh \
  -o provision-cpx22.sh

chmod +x provision-cpx22.sh
```

---

## Step 3 — Run the Setup Script

```bash
sudo ./provision-cpx22.sh
```

**What this does (takes 10–20 minutes):**

| Step | What happens | Time |
|------|-------------|------|
| System update | `apt upgrade` | ~2 min |
| Install dependencies | gcc, clang, openssl, git, etc. | ~1 min |
| Create `blink` user | Service user at `/opt/blink` | instant |
| Install Rust | rustup stable toolchain | ~2 min |
| Install ClickHouse | analytics warehouse | ~3 min |
| Clone repository | from GitHub, branch `claude/trade-bot-live-ready-6zaZf` | ~1 min |
| Build binary | `cargo build --release` | **5–15 min** |
| Create service | systemd unit installed | instant |
| Create env file | `/etc/blink-engine.env` from template | instant |

The **build step is the longest** — Rust compiles ~200 crates for the first time.

You'll see this at the end when it's done:
```
[BLINK] ======================================================
[BLINK]  SETUP COMPLETE
[BLINK] ======================================================
```

---

## Step 4 — Configure the Environment File

Open the config file:

```bash
nano /etc/blink-engine.env
```

You only need to fill in **two fields** for the paper soak test:

### 4.1 — Set the wallet to track (RN1_WALLET)

Find the line:
```
RN1_WALLET=REPLACE_WITH_TARGET_WALLET_ADDRESS
```

Change it to the Polymarket wallet you want to shadow, for example:
```
RN1_WALLET=0x1234567890abcdef1234567890abcdef12345678
```

> **How to find a good wallet to track:**  
> Go to polymarket.com → Leaderboard → click on a top trader → copy their wallet address from the URL.

### 4.2 — Set the markets to monitor (MARKETS)

Find the line:
```
MARKETS=REPLACE_WITH_TOKEN_ID_1,REPLACE_WITH_TOKEN_ID_2
```

Get token IDs from Polymarket:
1. Go to any active market on polymarket.com
2. Open your browser's address bar — the URL contains the market slug
3. Get the token ID via the API:

```bash
curl -s "https://gamma-api.polymarket.com/markets?active=true&limit=5" \
  | python3 -m json.tool \
  | grep -E '"conditionId"|"question"' \
  | head -20
```

Pick 2–3 active markets and add their `conditionId` values:
```
MARKETS=0xabc123...,0xdef456...,0x789ghi...
```

### 4.3 — Save and verify

Press `Ctrl+X`, then `Y`, then `Enter` to save.

Verify the file looks correct:
```bash
cat /etc/blink-engine.env | grep -E "PAPER_TRADING|LIVE_TRADING|RN1_WALLET|MARKETS"
```

Expected output:
```
PAPER_TRADING=true
LIVE_TRADING=false
RN1_WALLET=0x...         (your wallet)
MARKETS=0x...,0x...      (your token IDs)
```

---

## Step 5 — Start the Engine

```bash
sudo systemctl start blink-engine
```

Check it started cleanly:

```bash
sudo systemctl status blink-engine
```

You should see:
```
● blink-engine.service - Blink Trading Engine (Paper Soak Test)
     Loaded: loaded (/etc/systemd/system/blink-engine.service; enabled)
     Active: active (running) since ...
```

If it shows `failed`, jump to the Troubleshooting section at the end.

---

## Step 6 — Watch the Logs

```bash
sudo journalctl -u blink-engine -f
```

**Within the first 30 seconds you should see:**

```
BLINK ENGINE v0.2 — Shadow Maker Bot

[engine] Engine started — PAPER=true TUI=false RN1=0x...
[engine] ClickHouse connected: http://127.0.0.1:8123
[engine] WebSocket connection established
[engine] Subscribed to market: 0x...
[engine] Subscribed to market: 0x...
```

**Healthy ongoing output looks like:**

```
[engine] WS message received (book update)
[paper]  SIGNAL BUY 0.45 $23.50 — evaluating...
[paper]  PAPER FILL BUY @0.45 $23.50 — position opened
[risk]   check_pre_order ok: size=23.50 nav=1000.00 open=1
```

**Warnings to expect (normal):**
```
[warn] INTENT-SKIP hedge_or_flatten token=0x... side=SELL
```
This is correct — the engine is skipping RN1 hedge positions as designed.

Press `Ctrl+C` to stop following logs (the engine keeps running).

---

## Step 7 — Let It Run for 24 Hours

The engine runs as a background service. You can:

- **Disconnect SSH** — it keeps running via systemd
- **Check in anytime:** `sudo journalctl -u blink-engine -n 100`
- **Check ClickHouse:** see section below

You don't need to babysit it. Go sleep, come back tomorrow.

---

## Monitoring During the Test

### Quick health check (run any time):

```bash
# Is it running?
sudo systemctl is-active blink-engine

# How long has it been up?
sudo systemctl status blink-engine | grep "Active:"

# Last 50 log lines
sudo journalctl -u blink-engine -n 50

# Any errors?
sudo journalctl -u blink-engine | grep -i "ERROR\|FATAL\|panic" | tail -20

# Resource usage
ps aux | grep engine
```

### ClickHouse queries:

```bash
clickhouse-client
```

Then run SQL:

```sql
-- How many signals received?
SELECT COUNT(*) FROM trades;

-- Signals in last hour
SELECT COUNT(*) FROM trades
WHERE timestamp > now() - INTERVAL 1 HOUR;

-- Paper fills summary
SELECT
  side,
  COUNT(*) as orders,
  SUM(size_usdc) as total_usdc,
  AVG(size_usdc) as avg_usdc
FROM trades
GROUP BY side;

-- Exit ClickHouse
exit
```

---

## Step 8 — Review Results After 24 Hours

### 8.1 Stop the engine

```bash
sudo systemctl stop blink-engine
```

### 8.2 Find the session log

```bash
cat /opt/blink/blink/blink-engine/logs/LATEST_SESSION_LOG.txt
```

This prints the path to the full session log. View it:

```bash
SESSION_LOG=$(cat /opt/blink/blink/blink-engine/logs/LATEST_SESSION_LOG.txt)
echo "Session log: $SESSION_LOG"
wc -l "$SESSION_LOG"             # How many lines
tail -100 "$SESSION_LOG"         # Last 100 lines
```

### 8.3 Check for errors

```bash
SESSION_LOG=$(cat /opt/blink/blink/blink-engine/logs/LATEST_SESSION_LOG.txt)

echo "=== ERRORs ==="
grep -i "ERROR\|error\|FATAL" "$SESSION_LOG" | grep -v "#\[allow" | head -30

echo "=== WARNs ==="
grep -i "WARN\|warn" "$SESSION_LOG" | head -30

echo "=== WebSocket events ==="
grep -i "reconnect\|disconnect\|WS" "$SESSION_LOG" | head -20

echo "=== Risk blocks ==="
grep -i "BLOCKED\|breaker\|halt" "$SESSION_LOG" | head -20

echo "=== Fills ==="
grep -i "PAPER FILL\|FILL" "$SESSION_LOG" | head -30
```

### 8.4 ClickHouse final report

```bash
clickhouse-client --query "
SELECT
  'Total signals'      as metric, toString(COUNT(*))         as value FROM trades
UNION ALL
SELECT
  'Paper fills'        as metric, toString(countIf(filled))  as value FROM trades
UNION ALL
SELECT
  'Total volume USDC'  as metric, toString(SUM(size_usdc))   as value FROM trades
UNION ALL
SELECT
  'Max position USDC'  as metric, toString(MAX(size_usdc))   as value FROM trades
UNION ALL
SELECT
  'Avg latency ms'     as metric, toString(round(avg(fill_latency_ms), 1)) as value FROM trades;
"
```

### 8.5 Assess the results

**Green light (pass) criteria:**

| Check | What you want to see |
|-------|----------------------|
| Engine ran 24h without crash | `systemctl status` shows uptime > 23h |
| No unexpected FATAL errors | Zero panics, zero segfaults |
| WebSocket stable | Reconnects < 5 total (Polymarket drops occasionally) |
| Risk manager never false-tripped | No `BLOCKED` or `breaker trip` in logs |
| Fills being recorded correctly | Paper fills match signal count approximately |
| ClickHouse recording | Queries return real data, not empty |
| No reconciliation drift | Log shows no `drift detected` warnings |

**If all of those pass → you're ready to discuss Stage 1 live deployment.**

---

## Troubleshooting

### Engine fails to start

```bash
# Check the exact error
sudo journalctl -u blink-engine -n 50 --no-pager

# Most common cause: bad env file
# Check for syntax errors:
grep -n "=" /etc/blink-engine.env | head -20
```

**Common mistakes in env file:**
- Space around `=` sign: `MARKETS = 0x...` ← wrong, should be `MARKETS=0x...`
- Trailing space after value
- `REPLACE_WITH_...` placeholder still present

### WebSocket won't connect

```bash
# Test connectivity from the server
curl -s https://clob.polymarket.com/markets | head -100

# If that fails, check DNS
nslookup clob.polymarket.com

# Check outbound port 443
curl -v https://clob.polymarket.com 2>&1 | head -20
```

### ClickHouse not running

```bash
sudo systemctl start clickhouse-server
sudo systemctl status clickhouse-server

# Test it
clickhouse-client --query "SELECT 1"
```

### Build failed

```bash
# Try rebuilding manually as the blink user
sudo su - blink
source ~/.cargo/env
cd /opt/blink/blink/blink-engine
cargo build --release -p engine 2>&1 | tail -30
```

### Out of disk space

```bash
df -h
# If /opt is full, clear old logs:
find /opt/blink/blink/blink-engine/logs -name "*.log" -mtime +3 -delete
```

### Restart after changes

```bash
sudo systemctl restart blink-engine
sudo journalctl -u blink-engine -f
```

---

## Quick Reference

### All commands in one place

```bash
# Start
sudo systemctl start blink-engine

# Stop
sudo systemctl stop blink-engine

# Restart
sudo systemctl restart blink-engine

# Live logs
sudo journalctl -u blink-engine -f

# Last 100 lines
sudo journalctl -u blink-engine -n 100

# Check status
sudo systemctl status blink-engine

# Edit config
sudo nano /etc/blink-engine.env

# ClickHouse
clickhouse-client

# Session log path
cat /opt/blink/blink/blink-engine/logs/LATEST_SESSION_LOG.txt
```

### Key file paths

```
/etc/blink-engine.env                               ← config (edit this)
/opt/blink/blink/blink-engine/target/release/engine ← binary
/opt/blink/blink/blink-engine/logs/sessions/        ← session logs
/opt/blink/blink/blink-engine/logs/reports/         ← post-run reports
/etc/systemd/system/blink-engine.service            ← service unit
```

---

## What Success Looks Like

After 24 hours, your log should look something like this:

```
[engine] Engine started — PAPER=true
[engine] ClickHouse connected
[engine] WebSocket connected, subscribed to 3 markets
...
[paper]  SIGNAL BUY token=0x... price=0.45 size=23.50
[paper]  PAPER FILL BUY @0.45 $23.50 — virtual position opened
[warn]   INTENT-SKIP hedge_or_flatten — skipping RN1 hedge
[paper]  SIGNAL BUY token=0x... price=0.62 size=45.00
...
[ws]     WS reconnect after drop (normal) — resubscribing
[ws]     WebSocket reconnected, resubscribed in 1.2s
...
[engine] 24h session complete — 147 signals, 89 fills, 58 skipped
```

That's the green light. Clean, no crashes, no drift, fills recorded correctly.

---

*Guide version: 1.0 — 2026-04-04*  
*Target: Hetzner CPX22 / Ubuntu 22.04 / Paper trading soak test*
