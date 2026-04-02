# Blink Bot Implementation Plan — RN1 Smart Selective Shadow

**Created:** 2026-04-01  
**Objective:** Transform Blink from blind copy-trading to smart selective shadow with filters  
**Based on:** Deep research of RN1 whale trader ($6M profit, 69% win rate)

---

## 🎯 PROBLEM STATEMENT

**Current State:**
- Blink blindly mirrors ALL RN1 trades (100-1000/day)
- No filtering → copies trash farming, hedges, low-conviction bets
- $100 starting capital with $0.50 minimum bets (gas costs kill profit)
- Expected to fail due to execution lag + hidden hedges

**Desired State:**
- Smart selective copying with 6-stage filter pipeline
- Only mirror high-conviction RN1 trades ($10k-$100k bets)
- Detect and skip synthetic hedges (30% of RN1's trades)
- $5k capital with dynamic sizing (5-15% of RN1's bet)
- Expected: 55-60% win rate, 5-10% monthly ROI

---

## 📊 KEY RESEARCH FINDINGS

### RN1 Trader Profile:
```
Total Profit:      $6,052,115
Total Volume:      $243M+
ROI:               2.2% (massive volume strategy)
Win Rate:          69% (analyzed 150 trades)
Avg Bet:           $96,501
Markets:           40,000+ markets
Trades:            1,000,000+ trades
Strategy:          Market-making + Arbitrage + Synthetic hedging
Execution:         Automated HFT-style
```

### Critical Insights:
1. **RN1 is NOT a traditional bettor** → Quantitative market maker
2. **30% of trades are synthetic hedges** → Must detect and skip
3. **40% of trades are trash farming** (<$10k) → Skip for profitability
4. **Sports = 47% focus** (Soccer 37%, NFL 26%, NBA 11%)
5. **Average entry: 7 days before event** (2-72h window optimal)

### Why Blind Copying Fails:
- ❌ Execution lag (milliseconds vs seconds) → missed arbitrage
- ❌ Hidden hedges → copy closed positions unknowingly
- ❌ Platform rewards → trash trades unprofitable for us
- ❌ Capital gap → cannot replicate diversification

---

## ✅ APPROACH — "Smart Selective Shadow"

### Phase 1: Core Filters (CRITICAL)
**Goal:** Filter out 90% of noise, keep 10% high-signal trades

**6-Stage Filter Pipeline:**
1. ✅ Bet Size: $10k-$100k only (skip trash + mega-whale)
2. ✅ Hedge Detection: Skip synthetic hedges (opposite side, similar size)
3. ✅ Market Liquidity: $100k+ volume only
4. ✅ Category: Sports (priority) + high-liquidity politics
5. ✅ Entry Price: 25-65¢ range (RN1's sweet spot)
6. ✅ Timing: 2-72h before event

### Phase 2: Dynamic Sizing
**Goal:** Size bets proportional to RN1's conviction

**Conviction Signals:**
- Base: 5% of RN1's bet
- +5% if RN1 bet >$50k (whale-level)
- +2% if market liquidity >$200k
- +2% if sports category
- +1% if preferred sport (Soccer/NFL/NBA)
- Cap: 15% max, balance * 0.15 per trade

### Phase 3: Independent Exits
**Goal:** Don't wait for RN1 to exit

**Exit Strategy:**
- Take-profit: +12%
- Stop-loss: -8%
- Trailing stop: -5% from peak after +10%
- Time limit: 5 days max hold
- Optional: Follow RN1 exit signals

### Phase 4: Risk Management
**Limits:**
- Max 15% per trade
- Max 5 concurrent positions
- Daily loss limit: 20% of capital
- Weekly review & adjustment

---

## 📋 IMPLEMENTATION TODOS

**Status Legend:**
- ⏳ = In Progress (agent working)
- ✅ = Done
- ⬜ = Pending

### PHASE 1: Core Filters (Days 1-2)

⏳ **1. FilterConfig struct** (`filter-config-impl` agent)
   - Create FilterConfig in types.rs
   - Add SkipReason enum
   - Implement Default with RN1 values
   - Add from_env() loader

⏳ **2. PositionTracker** (`position-tracker-impl` agent)
   - Create src/position_tracker.rs
   - Track RN1 positions in HashMap
   - Implement is_hedge() detection (30% size tolerance)
   - Add cleanup for old positions

⬜ **3. Market Metadata Fetching**
   - fetch_market_metadata() via Gamma API
   - Parse: category, liquidity, event_time, tags
   - Cache results (5 min TTL)

⬜ **4. Filter Logic in Engines**
   - Update paper_engine.rs handle_signal()
   - Update live_engine.rs handle_signal()
   - 6-stage filter with detailed logging
   - Skip reason tracking

### PHASE 2: Dynamic Sizing (Days 3-4)

⬜ **5. Dynamic Sizing Function**
   - calculate_dynamic_size() with conviction bonuses
   - Test edge cases (tiny/huge bets)
   - Integrate with FilterConfig

⬜ **6. Position Monitoring**
   - monitor_position() async task
   - Spawn per filled order
   - Check P&L every minute
   - Execute exit conditions

⬜ **7. Exit Strategy Module**
   - src/exit_strategy.rs
   - ExitManager with take-profit/stop-loss
   - Trailing stop logic
   - Time-based exits

### PHASE 3: Testing (Days 5-7)

⏳ **8. Update Paper Config** (`update-config-values` agent)
   - STARTING_BALANCE: 100 → 5000
   - SIZE_MULTIPLIER: 0.02 → 0.05
   - MIN_TRADE: 0.50 → 100.00
   - MAX_POSITION_PCT: 0.10 → 0.15

⬜ **9. Unit Tests**
   - Bet size filtering (5 tests)
   - Hedge detection (5 scenarios)
   - Liquidity checks (3 tests)
   - Dynamic sizing (10 cases)
   - Target: 100% coverage

⬜ **10. 48h Paper Trading**
   - Run with new filters
   - Log all signals (executed + filtered)
   - Measure: filter effectiveness, win rate, ROI
   - Generate performance report

### PHASE 4: Live Trading Prep (Days 8-10)

⬜ **11. Monitoring Dashboard**
   - Real-time TUI dashboard
   - Current positions, P&L, orders
   - Filter stats, risk limits
   - Update every 1 second

⬜ **12. Alerts System**
   - Large loss alerts (-$500+)
   - Daily loss limit warnings
   - Failed order notifications
   - WebSocket disconnect alerts

⬜ **13. Live Trading Checklist**
   - Verify SIGNER_PRIVATE_KEY
   - Test API credentials
   - Fund $5k on Polymarket
   - Test all filters
   - Configure risk limits
   - Test emergency stop
   - Document in LIVE_TRADING_CHECKLIST.md

---

## 🎯 SUCCESS METRICS

### Short-term (1 month):
- ✅ Win rate ≥ 55%
- ✅ ROI ≥ 5% per month
- ✅ Max drawdown < 15%
- ✅ Filter effectiveness > 70%

### Medium-term (3 months):
- ✅ Win rate ≥ 58%
- ✅ ROI ≥ 7% per month
- ✅ Sharpe ratio > 1.5
- ✅ 3/3 profitable months

### Long-term (6-12 months):
- ✅ Win rate ≥ 60%
- ✅ ROI ≥ 9% per month
- ✅ Total profit > $10k (from $5k start)
- ✅ Automated & reliable

---

## 📈 EXPECTED PERFORMANCE

### Conservative ($5k capital):
```
Trades/month:      30 (filtered from ~500 RN1 trades)
Avg bet:           $500 (10% per trade)
Win rate:          55%
Avg profit/win:    +8%
Avg loss/loss:     -6%

Monthly:           $255 profit
ROI:               5.1% per month = 61% annually
```

### Moderate ($10k capital):
```
Trades/month:      40
Avg bet:           $800
Win rate:          58%

Monthly:           $865 profit
ROI:               8.7% per month = 104% annually
```

### Aggressive ($50k capital):
```
Trades/month:      50
Avg bet:           $3,000
Win rate:          60%

Monthly:           $4,800 profit
ROI:               9.6% per month = 115% annually
```

---

## ⚠️ RISK MANAGEMENT

### Execution Risks:
- Lag: 3-10s delay vs RN1's milliseconds
- Slippage: Orders fill worse than expected
- Failed fills: Low liquidity rejections

### Strategy Risks:
- Hidden hedges: Copying closed positions
- Capital mismatch: Cannot replicate diversification
- Platform rewards: RN1 gets rewards we don't

### Market Risks:
- Black swans: Event cancellations
- Resolution disputes: Polymarket conflicts
- Liquidity crunch: Cannot exit

### Mitigation:
- Start small ($5k max initially)
- Strict limits (15% per trade, 20% daily loss)
- Continuous monitoring
- Emergency kill switch
- Weekly reviews

---

## 🔄 DEPENDENCIES

```
Phase 1 (Core Filters):
  filter-config-struct
    ├─> position-tracker (depends on FilterConfig)
    ├─> market-metadata-fetch (depends on FilterConfig)
    └─> filter-logic-engine (depends on both above)

Phase 2 (Dynamic Sizing):
  filter-logic-engine
    ├─> dynamic-sizing
    ├─> position-monitoring (depends on dynamic-sizing)
    └─> exit-strategy (depends on position-monitoring)

Phase 3 (Testing):
  update-paper-config (parallel, no deps)
  filter-unit-tests (depends on filter-logic-engine)
    └─> paper-trading-48h (depends on tests + config)

Phase 4 (Live Prep):
  paper-trading-48h
    ├─> monitoring-dashboard
    ├─> alerts-system (depends on dashboard)
    └─> live-trading-checklist (depends on alerts)
```

---

## 📚 REFERENCE DOCUMENTS

**Research Reports (5 files):**
1. `RN1_ANALYSIS.md` — Full strategy breakdown
2. `RN1_DEEP_DIVE_ANALYSIS.md` — Quantitative patterns
3. `EXECUTIVE_SUMMARY_RN1.md` — Critical insights
4. `FINAL_RECOMMENDATIONS.md` — Implementation guide ⭐
5. `rn1_analysis_report.json` — 150 trades analyzed

**Visualizations:**
- Bet size distribution
- Time-of-day heatmap
- Market type pie chart
- Cumulative P&L curve

**Config History:**
- `config_history.md` — Track all setting changes

---

## 🚀 CURRENT STATUS

**Active Agents (running now):**
1. ⏳ `filter-config-impl` — Creating FilterConfig struct
2. ⏳ `position-tracker-impl` — Building hedge detection
3. ⏳ `update-config-values` — Updating paper config

**Next Up (after agents complete):**
1. Market metadata fetching
2. Filter logic integration
3. Dynamic sizing implementation

**ETA to Paper Trading:** 2-3 days  
**ETA to Live Trading:** 7-10 days

---

## 💡 LESSONS LEARNED

### From RN1 Research:
1. **Volume beats win rate** — 2.2% ROI × $243M = $6M profit
2. **Synthetic hedging is smart** — Saves fees by buying opposite
3. **Trash farming works IF you get rewards** — We don't, so skip it
4. **Sports >> Politics** — 47% vs 18% of RN1's focus
5. **Timing matters** — 7 days before event is sweet spot

### For Our Bot:
1. **Filters are CRITICAL** — 90% noise reduction required
2. **Hedge detection is MUST-HAVE** — 30% of RN1 trades are hedges
3. **Dynamic sizing beats fixed** — Match conviction level
4. **Independent exits required** — Don't wait for RN1
5. **Start small, scale gradually** — Prove strategy before whale mode

---

**Last Updated:** 2026-04-01 12:25 UTC  
**Status:** Phase 1 in progress (3 agents working)  
**Next Review:** After agent completion (~30 min)
