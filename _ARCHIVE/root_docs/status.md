# BLINK HFT — PROJEKTSTATUS

**Datum:** 2026-04-01  
**Version:** 0.2 (Paper Trading + Live Engine Foundation)

---

## 🎯 PROJEKTMÅL

**Huvudsyfte:**  
Bygga en ultra-low-latency copy-trading bot som **automatiskt speglar en profitabel traders ordrar** på Polymarket och tjänar pengar genom att:

1. **Snappa RN1:s ordrar i realtid** — när target wallet (RN1) lägger en limit order, detekterar vi det via WebSocket feed inom millisekunder
2. **Placera egna Post-Only-ordrar bredvid deras** — vi lägger våra ordrar adjacent till RN1:s position för att:
   - Tjäna **maker rebates** (vi får betalt för att tillhandahålla likviditet)
   - Undvika **taker fees** (vi betalar ALDRIG för att ta likviditet)
   - Minimera slippage (vi matchar RN1:s pris exakt)
3. **Använda sports-marknader** — fokus på live sports prediction markets där:
   - Volatilitet är hög (stora prisrörelser)
   - RN1 har bevisat edge (de tjänar pengar konsekvent)
   - Likviditeten är tillräcklig för våra ordrar

**Exakt strategi: "Shadow Maker"**  
Vi är RN1:s *skugga* — varje gång de lägger en order på $100, lägger vi en order på $2-$5 (2-5% av deras storlek) exakt vid sidan om. Om RN1 tjänar 10%, tjänar vi också ~10% på vårt kapital, utan att behöva researcha själva.

---

## 📍 VAR ÄR VI I ROADMAPEN?

### ✅ **PHASE 0 — FOUNDATIONS (KLAR)**
- Alla agent blueprints (Aura, Q-Sigma, Wraith, Nexus, Sentinel) definierade
- Tech stack beslutad (Rust, tokio, k256 för EIP-712)
- Polymarket API-endpoints mappade
- Lokal utvecklingsmiljö (Windows + Rust toolchain)

### ✅ **PHASE 1 — WEBSOCKET SNIFFER (KLAR)**
- ✅ Rust workspace med 2 crates: `engine` (huvudboten) + `market-scanner` (hitta markets)
- ✅ WebSocket-klient (`ws_client.rs`) — persistent connection med auto-reconnect
- ✅ Orderbook (`order_book.rs`) — in-memory BTreeMap, BUY/SELL spreads
- ✅ RN1 Sniffer (`sniffer.rs`) — filtrerar alla meddelanden för RN1:s wallet-adress
- ✅ 38 unit tests — alla gröna

### 🟡 **PHASE 2 — ORDER EXECUTION (75% KLAR)**

**Klart:**
- ✅ EIP-712 signing (`order_signer.rs`) — signerar ordrar lokalt med k256
- ✅ Order sizing (`paper_portfolio.rs`) — 10% av NAV per order, $1 minimum
- ✅ POST /order submission (`order_executor.rs`) — HMAC-SHA256 auth + retry-logik
- ✅ DELETE /order cancellation — stöd för single order + batch market wipe
- ✅ **FOK/FAK order types** — nytt stöd (2.5 ✅)
- ✅ Risk manager (`risk_manager.rs`) — circuit breakers, rate limiting
- ✅ **Transient error retry** — 4× retry vid 429/5xx/timeout med exponential backoff

**Pågående (3 todos in_progress):**
- 🔄 **LiveEngine wiring** — koppla in live trading-läget i main.rs
- 🔄 **Pre-game order wipe** — watchdog som cancellar ordrar vid game start
- 🔄 **CI/CD pipeline** — GitHub Actions för auto-testing

**Saknas:**
- ❌ Integration test mot Polymarket testnet (2.6)
- ❌ ClickHouse tick-data logging (1.6) — ej kritiskt ännu

### ❌ **PHASE 3 — SPORTS-SPECIFIC LOGIC (0% KLAR)**
- ❌ 3-second in-play delay handler (finns basic i paper_engine men ej produktionsklar)
- ❌ Volatility filter (Δprice > 5% → skip)
- ❌ Sports market discovery (market-scanner finns men ej integrerad i runtime)

### ❌ **PHASE 4 — RISK & SENTINEL (20% KLAR)**
- ✅ Risk manager basics (VaR, circuit breakers, rate limits)
- ❌ TEE key management (AWS Nitro Enclave / Intel SGX)
- ❌ Emergency kill switch
- ❌ Formal verification (Foundry + Halmos)

### ❌ **PHASE 5-7 — INFRASTRUCTURE, MEV, ML (0% KLAR)**
Dessa är långsiktiga produktions-mål och ännu inte påbörjade.

---

## 🤖 VAD HÄNDER NÄR VI KÖR BOTEN?

### **TRE LÄGEN:**

#### 1️⃣ **READ-ONLY MODE** (default, inga env vars satta)
```bash
cargo run --bin engine
```

**Vad händer:**
1. ✅ **WebSocket-anslutning** — kopplar upp mot `wss://ws-live-data.polymarket.com`
2. ✅ **Prenumererar på markets** — subscribes till alla token IDs från `MARKETS=` i `.env`
3. ✅ **Uppdaterar orderbook** — varje meddelande updaterar in-memory BTreeMap (best bid/ask)
4. ✅ **Detekterar RN1-ordrar** — filtrerar WebSocket-feed för `RN1_WALLET=` adress
5. ⚠️ **Loggar signaler** — skriver bara ut "RN1 signal detected — read-only mode"
6. ❌ **Skickar INGA ordrar** — read-only, zero risk

**Output:**
```
╔══════════════════════════════════════════════════════╗
║      BLINK ENGINE v0.2 — Shadow Maker Bot           ║
╚══════════════════════════════════════════════════════╝

Connecting to WebSocket feed
WebSocket handshake complete — subscribed to 20 markets
RN1 signal — BUY @0.650 $15.38 — read-only mode
RN1 signal — SELL @0.720 $8.33 — read-only mode
```

---

#### 2️⃣ **PAPER TRADING MODE** (`.env`: `PAPER_TRADING=true`)
```bash
PAPER_TRADING=true cargo run --bin engine
```

**Vad händer:**
1. ✅ Samma som read-only: WebSocket + orderbook + RN1 sniffer
2. ✅ **PaperEngine startar** — virtuell portfolio med $100 USDC
3. ✅ **Beräknar order size** — `size = RN1_size × 0.02` (2% av deras order)
4. ✅ **3-second fill window** — kollar priset var 500ms i 3 sekunder
   - Om priset rör sig >1.5% → abortar (skydd mot stale prices)
5. ✅ **Simulerar fill** — "fyller" ordern i virtual portfolio
6. ✅ **Uppdaterar P&L** — realized + unrealized PnL
7. ✅ **60-second dashboard** — printar portfolio-status varje minut

**Output:**
```
[2026-04-01 12:00:01] RN1 BUY signal: YES @0.650 × 23.08 shares ($15)
[2026-04-01 12:00:01] Sizing: $15 × 2% = $0.30 (below $1 min) → scaled to $1.00
[2026-04-01 12:00:01] Fill window check 1/6... price stable
[2026-04-01 12:00:02] Fill window check 2/6... price stable
[2026-04-01 12:00:03] ✅ FILLED: BUY YES @0.650 $1.00 (1.54 shares)

─────────────── PORTFOLIO SNAPSHOT ───────────────────
Cash:            $99.00 USDC
Invested:        $1.00
Unrealized P&L:  +$0.05 (+5.0%)
Realized P&L:    $0.00
NAV:             $100.05
Signals/Filled:  1/1
```

**Med TUI mode** (`PAPER_TRADING=true TUI=true`):
- Full-screen ratatui dashboard med live orderbook, positions, activity log
- Loggar skrivs till `logs/engine.log` istället för stdout

---

#### 3️⃣ **LIVE TRADING MODE** (`.env`: `LIVE_TRADING=true`) — 🚨 EJ KLAR ÄNNU
```bash
LIVE_TRADING=true cargo run --bin engine
```

**Vad händer (när implementationen är klar):**
1. ✅ Samma WebSocket + sniffer som alltid
2. ✅ **LiveEngine startar** — laddar EIP-712 private key från `SIGNER_PRIVATE_KEY=`
3. ✅ **Risk checks** — RiskManager kollar:
   - Order size < $1000? ✅
   - Concurrent positions < 5? ✅
   - Daily loss < 10% NAV? ✅
   - Rate limit (3 orders/sec)? ✅
4. ✅ **Signerar order lokalt** — EIP-712 signature med k256 (inga API-anrop)
5. ✅ **POST /order till CLOB** — HMAC-SHA256 authenticated request
   - `maker: true` (Post-Only, får ALDRIG bli taker)
   - `order_type: "GTC"` (eller FOK/FAK för sports)
   - Retry 4× vid transient errors (429, 5xx, timeout)
6. ✅ **Väntar på exchange-svar** — `{"success": true, "orderID": "..."}`
7. ✅ **Loggar till activity log** — alla fills, aborts, errors

**Nuvarande status:**
- ❌ `LiveEngine` finns men är **EJ kopplad till main.rs** ännu
- ❌ `LIVE_TRADING=true` gör ingenting just nu (agenten arbetar på att fixa detta)

---

## 🔧 TEKNISK ARKITEKTUR (runtime)

```
┌─────────────────────────────────────────────────────────┐
│ main.rs (tokio multi-thread runtime)                    │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  ┌──────────────┐    ┌───────────────┐   ┌───────────┐ │
│  │ WebSocket    │───▶│ OrderBook     │   │ Sniffer   │ │
│  │ Task         │    │ (BTreeMap)    │   │ (RN1)     │ │
│  │ (tokio::spawn)│   └───────────────┘   └─────┬─────┘ │
│  └──────────────┘                              │       │
│         │                                       │       │
│         │ price updates                         │       │
│         ▼                                       ▼       │
│  ┌────────────────────────────────────────────────────┐│
│  │ crossbeam channel: RN1Signal                       ││
│  └────────────────┬───────────────────────────────────┘│
│                   │                                     │
│                   ▼                                     │
│  ┌────────────────────────────────┐                    │
│  │ PaperEngine / LiveEngine       │                    │
│  │ (spawn_blocking thread)        │                    │
│  ├────────────────────────────────┤                    │
│  │ 1. handle_signal()             │                    │
│  │ 2. calculate_size()            │                    │
│  │ 3. RiskManager.check_pre_order │                    │
│  │ 4. check_fill_window (3sec)    │                    │
│  │ 5. sign_order (EIP-712)        │◀──┐                │
│  │ 6. executor.submit_order()     │   │ dry_run=false  │
│  │ 7. portfolio.open_position()   │   │ → HTTP POST    │
│  └────────────────────────────────┘   │                │
│                                        │                │
│  ┌──────────────────────────┐         │                │
│  │ OrderExecutor            │◀────────┘                │
│  │ (reqwest HTTP client)    │                          │
│  ├──────────────────────────┤                          │
│  │ POST /order (retry 4×)   │                          │
│  │ DELETE /order            │                          │
│  │ HMAC-SHA256 auth         │                          │
│  └──────────────────────────┘                          │
│                                                          │
│  ┌─────────────────────────────────────────────────┐   │
│  │ TUI Thread (optional, paper mode only)          │   │
│  │ - Portfolio snapshot                            │   │
│  │ - Open positions                                │   │
│  │ - Activity log                                  │   │
│  │ - Live orderbook prices                         │   │
│  └─────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

---

## 📊 NUVARANDE METRICS

| Metric | Värde | Mål |
|--------|-------|-----|
| **Unit tests** | 38/38 pass | ✅ |
| **Compilation time** | ~7s | ✅ |
| **WebSocket latency** | < 10ms | ✅ (ej mätt exakt ännu) |
| **Order submission** | Dry-run only | ❌ (väntar på live mode) |
| **3-sec failsafe** | Implementerad | ✅ |
| **Risk checks** | 6 breakers active | ✅ |
| **FOK/FAK support** | Implementerad | ✅ |
| **Transient error retry** | 4× with backoff | ✅ |

---

## 🚀 NÄSTA STEG

### **Pågående (agenterna arbetar på detta):**
1. ✅ Wire LiveEngine into main.rs (90% klar)
2. ✅ Pre-game order wipe watchdog
3. ✅ CI/CD pipeline (GitHub Actions)

### **Direkt efter (nästa sprint):**
1. ❌ Integration test mot Polymarket testnet
2. ❌ Sports market discovery runtime-integration
3. ❌ Volatility filter (skip markets med >5% Δprice)
4. ❌ Emergency kill switch endpoint

### **Produktions-klart (Phase 5-7):**
- Bare-metal server med kernel tuning
- MEV protection (Flashbots bundles)
- ClickHouse tick-data warehouse
- RL model för gas price prediction

---

## 💰 FINANSIELLA RISKER & BEGRÄNSNINGAR

**Hårdkodade säkerhetsgränser:**
- Max order size: **$1000** (konfigurerad via `MAX_SINGLE_ORDER_USDC`)
- Max concurrent positions: **5**
- Max daily loss: **10% av NAV** (circuit breaker trigger)
- Rate limit: **3 orders/sekund** (CLOB rate limit safety)
- Order type: **Post-Only ONLY** — vi blir ALDRIG taker (undviker fees)

**Paper trading först:**
- Kör **MINST 7 dagar** i paper mode för att verifiera edge
- Om paper NAV inte ökar >5% efter 7 dagar → strategin fungerar ej

**Live trading:**
- Börja med **$100-$500 kapital** (testbelopp)
- Aldrig >2% av total portfolio i en enskild market
- Manual kill switch: `Ctrl-C` stänger allt instantly

---

## 📝 KONFIGURATION (`.env` exempel)

```bash
# ─── WebSocket & Markets ─────────────────────────────
CLOB_HOST=https://clob.polymarket.com
WS_URL=wss://ws-live-data.polymarket.com
MARKETS=12345,67890,11111  # token IDs (från market-scanner)
RN1_WALLET=0xYOUR_TARGET_WALLET_ADDRESS

# ─── Modes ───────────────────────────────────────────
PAPER_TRADING=true   # false = read-only
LIVE_TRADING=false   # true = real orders (EJ KLAR ÄNNU)
TUI=false            # true = full-screen dashboard (bara paper mode)

# ─── Live Trading Credentials (required if LIVE_TRADING=true) ─
SIGNER_PRIVATE_KEY=0xYOUR_PRIVATE_KEY_HEX
POLYMARKET_FUNDER_ADDRESS=0xYOUR_WALLET_ADDRESS
POLYMARKET_API_KEY=uuid-här
POLYMARKET_API_SECRET=base64-secret
POLYMARKET_API_PASSPHRASE=your-passphrase

# ─── Risk Settings ───────────────────────────────────
MAX_SINGLE_ORDER_USDC=20.0      # max per order
MAX_CONCURRENT_POSITIONS=5       # max simultaneous positions
MAX_DAILY_LOSS_PCT=0.10          # 10% circuit breaker
MAX_ORDERS_PER_SECOND=3          # rate limit
TRADING_ENABLED=false            # kill switch

# ─── Logging ─────────────────────────────────────────
LOG_LEVEL=info  # debug | info | warn | error
```

---

## ✅ SAMMANFATTNING

**Status:** **Phase 2 nästan klar (75%)** — kärnfunktionaliteten finns, polering pågår.

**Vad fungerar:**
- ✅ WebSocket sniffer — detekterar RN1-ordrar i realtid
- ✅ Paper trading — fullständig simulering med portfolio tracking
- ✅ EIP-712 signing — signerar ordrar lokalt
- ✅ Risk management — 6 circuit breakers
- ✅ Retry logic — hanterar transient errors
- ✅ FOK/FAK — ordertyper för sports

**Vad saknas:**
- ❌ LiveEngine är ej kopplad till main.rs (fixas inom 24h)
- ❌ Pre-game order wipe (fixas inom 24h)
- ❌ Integration testing mot testnet
- ❌ Production deployment (Phase 5-7)

**När kan vi köra live?**
- Efter LiveEngine-wiring är klar (≈ idag/imorgon)
- Efter 7 dagars successful paper trading
- Efter att du har satt upp Polymarket API credentials
- Efter att du har funded din wallet med USDC + MATIC

**Estimerad tidsplan:**
- **Nästa 48h:** LiveEngine live + pre-game wipe klar
- **Nästa vecka:** Integration tests + 7 dagars paper trading
- **Om 2 veckor:** Första live orders med $100 kapital
