# Blink Live Operator's Quick Start

**Status**: Ready for Stage 1 (Conservative Canary)  
**Capital per Stage 1 order**: $5 USDC max  
**Deployment target**: Ubuntu 22.04 LTS bare-metal server  
**Estimated setup time**: 2–3 hours (first time)

---

## 1. Server Provisioning (First Time Only)

### 1.1 Prerequisites
- Ubuntu 22.04 LTS server (c5.2xlarge or equivalent)
- Root/sudo access
- 50 GB free disk (for ClickHouse + logs)
- Public internet access (no proxy)

### 1.2 Run Provisioning Script

```bash
cd /home/user/blink/blink-engine/infra
chmod +x provision.sh
sudo ./provision.sh
```

**This installs:**
- Rust toolchain (stable)
- ClickHouse (analytics warehouse)
- Foundry (forge/cast)
- systemd service unit
- Base environment file template

### 1.3 Run OS Tuning (CRITICAL for low-latency)

```bash
chmod +x os_tune.sh
sudo ./os_tune.sh
```

**Then edit `/etc/default/grub` and add to `GRUB_CMDLINE_LINUX`:**
```
intel_pstate=disable processor.max_cstate=0 intel_idle.max_cstate=0 idle=poll isolcpus=0-7 nohz_full=0-7 rcu_nocbs=0-7 transparent_hugepage=never
```

**Apply and reboot:**
```bash
sudo update-grub
sudo reboot
```

---

## 2. Credential Acquisition

### 2.1 Create Polymarket API Key

1. Go to https://polymarket.com
2. Verify your wallet (must own Polygon USDC.e)
3. Navigate to **Settings** → **API Keys**
4. Click **Create API Key** or **Derive API Key**
5. Copy the returned:
   - `api_key`
   - `api_secret` (base64)
   - `passphrase`

### 2.2 Export Private Key

For your signer account (EOA or proxy):

```bash
# Using MetaMask Export:
# Settings → Security → Export Private Key

# Store securely (NEVER commit to git or email):
SIGNER_PRIVATE_KEY=abc123def456...  # 64 hex chars, no 0x prefix
```

### 2.3 Get Funder Address

The address that holds capital on Polygon. If using POLY_PROXY:

```bash
# Find via etherscan:
https://polygonscan.com/token/0x2791bca1f2de4661ed88a30c99a7a9449aa84174

# Or query via cast:
cast call 0x2791bca1f2de4661ed88a30c99a7a9449aa84174 \
  "balanceOf(address)(uint256)" 0xYOUR_ADDRESS

# Store as:
POLYMARKET_FUNDER_ADDRESS=0x...  # 42 chars, with 0x prefix
```

---

## 3. Environment Configuration

### 3.1 Create Live Env File

```bash
sudo cp /etc/blink-engine.env /etc/blink-engine.env.backup
sudo nano /etc/blink-engine.env
```

### 3.2 Fill in Required Fields

Copy from `.env.live.template` and substitute your values:

```env
# ─── Endpoints ───────────────────────────────────────────────
CLOB_HOST=https://clob.polymarket.com
WS_URL=wss://ws-subscriptions-clob.polymarket.com/ws/market
GAMMA_API=https://gamma-api.polymarket.com

# ─── Your wallet ─────────────────────────────────────────────
RN1_WALLET=0x...  # Trading bot's monitoring address

# ─── Markets ─────────────────────────────────────────────────
MARKETS=TOKEN_ID_1,TOKEN_ID_2  # e.g., 0x123,0x456
# To discover markets: cargo run -p market-scanner

# ─── Operating Mode (REQUIRED FOR LIVE) ──────────────────────
LIVE_TRADING=true
PAPER_TRADING=false
TUI=false
TRADING_ENABLED=true

# ─── Canonical Live Profile (DO NOT CHANGE) ──────────────────
BLINK_LIVE_PROFILE=canonical-v1

# ─── Stage 1 Canary (Conservative) ──────────────────────────
LIVE_ROLLOUT_STAGE=1
LIVE_CANARY_MAX_ORDER_USDC=5.0
LIVE_CANARY_MAX_ORDERS_PER_SESSION=20
LIVE_CANARY_DAYTIME_ONLY=true
LIVE_CANARY_START_HOUR_UTC=8
LIVE_CANARY_END_HOUR_UTC=22
LIVE_CANARY_MAX_REJECT_STREAK=3
# LIVE_CANARY_ALLOWED_MARKETS=TOKEN_ID_1,TOKEN_ID_2  # Recommended: lock to 1 market

# ─── Live Credentials (FILL THESE IN) ────────────────────────
SIGNER_PRIVATE_KEY=FILL_WITH_64_HEX_CHARS
POLYMARKET_FUNDER_ADDRESS=0xFILL_WITH_YOUR_PROXY_WALLET
POLYMARKET_API_KEY=FILL_WITH_API_KEY
POLYMARKET_API_SECRET=FILL_WITH_BASE64_SECRET
POLYMARKET_API_PASSPHRASE=FILL_WITH_PASSPHRASE

# ─── Signature Type ──────────────────────────────────────────
# 0 = EOA (direct wallet)
# 1 = POLY_PROXY (most common for funded accounts)
# 2 = GNOSIS_SAFE
POLYMARKET_SIGNATURE_TYPE=1

# ─── Nonce Strategy ─────────────────────────────────────────
# 0 = auto-increment (recommended)
POLYMARKET_ORDER_NONCE=0

# ─── Order Expiration ───────────────────────────────────────
# 0 = GTC (good-til-cancelled, recommended)
POLYMARKET_ORDER_EXPIRATION=0

# ─── Risk Management ────────────────────────────────────────
MAX_DAILY_LOSS_PCT=0.10
MAX_CONCURRENT_POSITIONS=5
MAX_SINGLE_ORDER_USDC=5.0
MAX_ORDERS_PER_SECOND=2

# ─── Observability ─────────────────────────────────────────
LOG_LEVEL=info
CLICKHOUSE_URL=http://localhost:8123
AUTO_POSTRUN_REVIEW=true
POSTRUN_REVIEW_DIR=logs/reports

# ─── Reconciliation ────────────────────────────────────────
LIVE_RECONCILE_INTERVAL_SECS=10

# ─── WebSocket Resilience ─────────────────────────────────
WS_RECONNECT_DEBOUNCE_MS=1500
```

### 3.3 Secure the File

```bash
sudo chmod 600 /etc/blink-engine.env
sudo chown root:root /etc/blink-engine.env
```

**Verify:**
```bash
sudo ls -la /etc/blink-engine.env
# Should show: -rw------- root root
```

---

## 4. Pre-Flight Validation

### 4.1 Run Preflight Check

```bash
cargo run --release -p engine -- --preflight-live
```

**Expected output:**
```
✅ preflight-live [1/4] market data: token=0x... buy=0.45 sell=0.55 mid=0.50
✅ preflight-live [2/4] auth credentials valid (GET /auth/ok)
✅ preflight-live [3/4] order config: signature_type=1 nonce=0 expiration=0
✅ preflight-live [4/4] risk limits: max_single_order_usdc=5.0 max_daily_loss_pct=0.1

🟢  ALL PREFLIGHT CHECKS PASSED — safe to go live
```

**If it fails**, check:
- Network connectivity to Polymarket
- API credentials are correct (try curl manually)
- Market token IDs are valid and live
- Risk config values are > 0

### 4.2 Verify USDC.e Balance

```bash
# Check via Polygonscan or:
cast call 0x2791bca1f2de4661ed88a30c99a7a9449aa84174 \
  "balanceOf(address)(uint256)" \
  0xYOUR_FUNDER_ADDRESS

# Should return >= (Stage 1 capital + 10% buffer)
# For $5/order × 20 orders = $100 min + $10 buffer = $110
```

### 4.3 Verify Allowance

```bash
cast call 0x2791bca1f2de4661ed88a30c99a7a9449aa84174 \
  "allowance(address,address)(uint256)" \
  0xYOUR_FUNDER_ADDRESS 0x7474fe84820298eb4e57d1f1b1d36c49a1b83b87

# CLOB contract: 0x7474fe84820298eb4e57d1f1b1d36c49a1b83b87
# Should return >= $1000 (or increase via MetaMask)
```

---

## 5. Start the Engine

### 5.1 Start via systemd

```bash
sudo systemctl start blink-engine
sudo systemctl enable blink-engine
```

### 5.2 Monitor Logs

```bash
# Real-time logs:
sudo journalctl -u blink-engine -f

# Or check the session log:
cat /opt/blink/blink-engine/logs/LATEST_SESSION_LOG.txt
tail -50f "$(cat logs/LATEST_SESSION_LOG.txt)"
```

### 5.3 Verify Startup

Look for these in logs:
```
[BLINK] Engine started — PAPER=false TUI=false RN1=0x...
[BLINK] eBPF kernel telemetry attached
[BLINK] ClickHouse connected: http://localhost:8123
[BLINK] WebSocket connection established
```

---

## 6. Operational Commands

### 6.1 Check Status

```bash
sudo systemctl status blink-engine
sudo journalctl -u blink-engine -n 50  # Last 50 lines
```

### 6.2 Emergency Stop (Cancels All Orders)

```bash
cargo run --release -p engine -- --emergency-stop
```

**Creates**: `logs/EMERGENCY_STOP.flag`

### 6.3 Graceful Shutdown

```bash
sudo systemctl stop blink-engine
# Engine will cancel open orders and exit cleanly (up to 30s timeout)
```

### 6.4 Rotate Credentials

If API key is compromised:

```bash
# 1. Create new API key on Polymarket
# 2. Update /etc/blink-engine.env with new values
# 3. Stop the engine
sudo systemctl stop blink-engine

# 4. Run emergency stop to clear old orders
cargo run --release -p engine -- --emergency-stop

# 5. Restart
sudo systemctl start blink-engine
```

---

## 7. Monitoring & Alerts

### 7.1 ClickHouse Queries

```sql
-- Top markets by order volume
SELECT token_id, COUNT(*) as orders, SUM(size_usdc) as volume
FROM trades
WHERE timestamp > now() - INTERVAL 1 HOUR
GROUP BY token_id
ORDER BY volume DESC;

-- Fill success rate (last hour)
SELECT 
  countIf(confirmed_fills > 0) * 100.0 / COUNT(*) as success_pct
FROM trades
WHERE timestamp > now() - INTERVAL 1 HOUR;

-- Average fill latency (ms)
SELECT
  avg(fill_latency_ms) as avg_latency,
  max(fill_latency_ms) as max_latency
FROM trades
WHERE timestamp > now() - INTERVAL 1 HOUR;
```

### 7.2 Key Metrics to Watch

| Metric | Target | Alert If |
|--------|--------|----------|
| Auth success rate | >99.5% | <99% for 10 min |
| Fill confirmation rate | >95% | <90% for 1 min |
| Reconciliation drift | 0 | Any unexplained divergence |
| Heartbeat OK | 100% | Failures > 2 consecutive |
| Daily loss | <10% | Approach circuit breaker |

### 7.3 Post-Run Report

After each session, check:

```bash
ls -la logs/reports/
# Review: summary.txt, fills.csv, risk.txt
```

---

## 8. Troubleshooting

### Problem: Auth Fails (401/403)

**Check:**
```bash
curl -H "POLY-API-KEY: $POLYMARKET_API_KEY" \
     -H "POLY-SIGNATURE: ..." \
     https://clob.polymarket.com/auth/ok
```

**Fix:** Regenerate API credentials on Polymarket.com

### Problem: WebSocket Disconnects

**Check logs** for:
```
WS connection lost — reconnecting in Xms
```

**Fix:** Usually transient. If persistent, check:
- Network stability (ping polymarket.com)
- Firewall rules for port 443
- Credentials expiry

### Problem: Orders Rejected

**Check:**
```
LIVE REJECTED BUY @0.45 $5.00
```

**Causes:**
- Insufficient balance
- Order size > market liquidity
- Invalid signature type for account
- Rate limited (too many orders/sec)

**Fix:** 
- Verify balance + allowance
- Reduce stage 1 order size
- Check signature type matches account model

### Problem: Engine Won't Start

**Check:**
```bash
cargo run --release -p engine -- --preflight-live
# Should show detailed error
```

**Common issues:**
- Missing LIVE_TRADING=true
- Bad credentials
- Wrong signature type
- Market token IDs don't exist

---

## 9. Incident Playbook (If Something Goes Wrong)

### Step 1: Pause

```bash
# The engine monitors for reconciliation drift.
# If drift detected, it auto-pauses. Otherwise, manual pause:

# (For now, no --pause command; use kill-switch approach)
sudo systemctl stop blink-engine
```

### Step 2: Cancel All Orders

```bash
cargo run --release -p engine -- --emergency-stop
# Writes: logs/EMERGENCY_STOP.flag
```

### Step 3: Reconcile

```bash
# Check ClickHouse for unrealized positions
SELECT token_id, side, size, entry_price, current_price
FROM active_positions
WHERE status = 'open';

# Manually close via Polymarket UI or CLI if needed
```

### Step 4: Assess Damage

```bash
tail -100 logs/sessions/engine-session-*.log | grep -i error
# Review post-run report
cat logs/reports/summary.txt
```

### Step 5: Communicate

- Notify risk committee
- Document incident in tickets
- Prepare remediation plan

### Step 6: Controlled Resume

- Fix the root cause
- Run `--preflight-live` again
- Get sign-off from technical lead
- Restart engine

---

## 10. Operational Sign-Off Checklist

**Before starting Stage 1:**

- [ ] Server OS tuned + rebooted
- [ ] Provisioning script completed
- [ ] Credentials acquired + stored securely
- [ ] Environment file created + permissions 600
- [ ] Preflight check passed
- [ ] USDC.e balance verified (≥$110 for Stage 1)
- [ ] Allowance verified (≥$1000)
- [ ] Engine logs show healthy startup
- [ ] ClickHouse connected + recording
- [ ] Operator trained on incident playbook
- [ ] On-call rotation assigned
- [ ] Risk committee approved Stage 1 limits
- [ ] Kill-switch tested in paper mode

**Sign-off by:**
- Technical Lead: _____________
- Risk Officer: _____________
- Operations: _____________

**Date**: _____________

---

## 11. Quick Reference

### Critical Commands

```bash
# Preflight (before starting)
cargo run --release -p engine -- --preflight-live

# Run (normal operation)
sudo systemctl start blink-engine

# Check status
sudo journalctl -u blink-engine -f

# Emergency stop (cancels all orders)
cargo run --release -p engine -- --emergency-stop

# Restart
sudo systemctl restart blink-engine
```

### Critical Paths

```
/etc/blink-engine.env          # Live credentials (chmod 600)
/opt/blink/logs/              # Session logs
/opt/blink/logs/reports/      # Post-run analysis
logs/LATEST_SESSION_LOG.txt    # Current session pointer
```

### Support

- **Technical Issues**: Check logs + run `--preflight-live`
- **Urgent Outage**: `--emergency-stop` + systemctl stop
- **Credentials Compromise**: Rotate on Polymarket + update env file + restart

---

**Last updated**: 2026-04-04  
**Version**: 1.0  
**Approval**: Ready for Stage 1 canary deployment
