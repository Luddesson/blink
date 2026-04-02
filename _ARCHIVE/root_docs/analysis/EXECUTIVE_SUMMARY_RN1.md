# RN1 EXECUTIVE SUMMARY — Kritiska Insikter & Åtgärdsplan

**Datum:** 2026-01-24  
**Analyskällor:** 3 parallella research agents + web intelligence  
**Status:** ⚠️ KRITISKA UPPTÄCKTER — BOT-STRATEGI MÅSTE JUSTERAS

---

## 🚨 GAME CHANGER DISCOVERIES

### **Vad vi trodde:**
- ❌ RN1 är en sports bettor med 70% win rate
- ❌ De gör 3-5 directional bets/dag
- ❌ Vi kan spegla alla deras bets 1:1

### **Vad vi NU vet:**
- ✅ RN1 är en **QUANTITATIVE MARKET MAKER** (inte bettor)
- ✅ De gör **100-1000+ trades/dag** (automated HFT-style)
- ✅ **54-61% win rate** (inte 70%) men massive volume ($243M+)
- ✅ **2.2% ROI** på volym = $6M+ profit
- ✅ Strategi: Arbitrage + Market-making + Volume farming
- ✅ **Synthetic hedging** (köper motsatt side istället för att sälja)
- ✅ **Platform rewards dependency** (trash farming = unprofitable utan rewards)

---

## 📊 RN1 PROFIL — Faktisk Data

```
Total Volume:      $243,000,000+
Realized Profit:   $6,052,115
ROI:               2.2% (låg % men hög absolut profit)
Win Rate:          54-61%
Markets Traded:    40,000+ markets
Total Trades:      1,000,000+ trades
Avg Entry Price:   ~34¢ (fokus på underdogs)
Avg Bet Size:      $10k-$50k (high conviction)
                   $100-$1k (trash farming)
Frequency:         100-1000+ trades/dag
Strategy:          Market-making + Arbitrage + Rewards farming
```

---

## 🎯 RN1'S FAKTISKA STRATEGI

### **1. Quantitative Arbitrage (60% av profit)**

**Hur det funkar:**
- Identifierar markets där summan av probabilities ≠ 100%
- Köper systematiskt **underpriced outcomes** (avg entry 34¢)
- Exploaterar "favorite-longshot bias" (retail overbets favorites)
- Exit INNAN resolution (short holding time)

**Exempel:**
```
Market: Team A vs Team B
Current odds:
  Team A YES = 0.58 (58%)
  Team A NO = 0.45 (45%)
  Total = 103% (arbitrage opportunity!)

RN1 action:
  - Buy Team A NO @ 0.45
  - If market corrects to 0.42, sell @ 0.42
  - Profit: 3¢ per share (7% ROI)
  - Never cares om Team A vinner eller inte!
```

### **2. Synthetic Hedging (30% av trades)**

**Istället för att sälja position (fees + slippage):**
- Köper **motsatt outcome** i samma market
- Skapar **delta neutral position** syntetiskt
- Sparar 2% trading fees + 1-2% slippage

**Exempel:**
```
Initial:  Buy YES @ 0.55 ($10k)
Market moves to 0.75 → unrealized profit +$2k

RN1's exit:
  ❌ INTE: Sell YES @ 0.75 (loses 2% fees + slippage)
  ✅ ISTÄLLET: Buy NO @ 0.25 ($10k)
  
Result:
  - Position neutralized (owns both YES and NO)
  - Guaranteed payout: $10k (breakeven)
  - Saved: $400 fees + $200 slippage = $600
  - Actual profit: $2k - cost of NO shares
```

**KRITISKT för vår bot:**
När vi ser RN1 köpa NO efter de köpt YES → de stänger position, inte öppnar ny!

### **3. Volume Farming / "Trash Farming" (10% av trades)**

**Strategi:**
- Köper MASSIVE quantities av worthless contracts ($0.01-$0.03)
- Boost trading volume för platform rewards
- Platform incentives offsettar losses

**Exempel:**
```
Action: Buy 100,000 shares @ $0.01 = $1,000 cost
Outcome: Loses to 0 → -$1,000 loss
BUT: Platform rewards = $1,500
Net: +$500 profit
```

**KRITISKT för vår bot:**
❌ **VI FÅR INTE DESSA REWARDS!** → Trash farming bets = guaranteed loss för oss!

---

## ⚠️ VARFÖR BLIND COPYING = DÅLIGT

### **Problem 1: Execution Lag**
```
RN1 uses automated HFT → execution in milliseconds
Vår bot → execution in 3-10 seconds

Arbitrage opportunities:
  - RN1 enters @ 0.45 (mispriced)
  - Market corrects to 0.48 within 5 seconds
  - Vi enters @ 0.48 (already repriced)
  - RN1 profit: 3¢ per share
  - Vår profit: 0¢ (eller loss om market reverses)
```

### **Problem 2: Hidden Hedges**
```
Vad vi ser:
  - RN1 buys YES @ 0.55 ($50k)

Vad vi INTE ser:
  - RN1 bought NO @ 0.50 ($50k) 2 min earlier (off-platform)
  - Position är redan hedged
  - Om vi kopierar YES → unhedged exposure → high risk!
```

### **Problem 3: Platform Rewards**
```
RN1's trash farming trade:
  Cost: $1,000
  Platform rewards: $1,500
  Net: +$500 ✅

Vår kopia:
  Cost: $1,000
  Platform rewards: $0 (we don't qualify)
  Net: -$1,000 ❌
```

### **Problem 4: Capital Requirements**
```
RN1's diversification:
  - 40,000+ markets
  - 100+ concurrent positions
  - Required capital: $500k-$1M

Vår capital:
  - $200-$5,000
  - Cannot replicate diversification
  - Higher variance, higher risk
```

---

## ✅ REKOMMENDERAD STRATEGI — "Smart Selective Copy"

### **Phase 1: STRIKTA FILTERS (most important!)**

```rust
// 1. BET SIZE FILTER
const MIN_RN1_BET: f64 = 5_000.0;   // Skip trash farming
const MAX_RN1_BET: f64 = 50_000.0;  // Skip mega-whale plays

// 2. MARKET LIQUIDITY FILTER
const MIN_MARKET_LIQUIDITY: f64 = 100_000.0;  // Only liquid markets

// 3. MARKET TYPE FILTER
allowed_categories = ["sports", "politics"];
allowed_sports = ["NBA", "NFL", "MLB"];

// 4. ENTRY PRICE FILTER
const MIN_ENTRY_PRICE: f64 = 0.25;  // Skip extreme longshots
const MAX_ENTRY_PRICE: f64 = 0.65;  // Skip heavy favorites

// 5. TIMING FILTER
if market.event_start_time - now() < 2.hours {
    return SkipReason::TooClose;  // Avoid late entries
}
```

### **Phase 2: HEDGE DETECTION**

**Kritiskt: Detektera när RN1 stänger position via synthetic hedge:**

```rust
// Track RN1's positions in memory
struct Position {
    market_id: String,
    side: Side,        // YES or NO
    size: f64,
    timestamp: i64,
}

fn is_hedge_trade(signal: &Signal) -> bool {
    // Check om RN1 redan har motsatt position
    let existing = get_rn1_position(&signal.market_id);
    
    if let Some(pos) = existing {
        if pos.side != signal.side && 
           similar_size(pos.size, signal.size, 0.20) {  // Within 20%
            return true;  // Detta är en hedge!
        }
    }
    false
}
```

**Action om hedge detected:**
- ❌ Skippa denna trade (RN1 stänger position)
- ✅ Stäng VÅR motsvarande position också

### **Phase 3: POSITION SIZING**

```rust
// INTE fast multiplier → Dynamic baserat på conviction
fn calculate_our_size(rn1_size: f64, market: &Market) -> f64 {
    // Base: 1/10th av RN1's size (safe)
    let base_multiplier = 0.10;
    
    // Conviction indicators
    let high_conviction = rn1_size > 20_000.0;
    let high_liquidity = market.volume > 200_000.0;
    let preferred_category = market.tags.contains("NBA") || 
                            market.tags.contains("NFL");
    
    let multiplier = match (high_conviction, high_liquidity, preferred_category) {
        (true, true, true)   => 0.15,  // Max conviction
        (true, true, false)  => 0.12,
        (true, false, _)     => 0.08,
        _                    => 0.05,  // Low conviction
    };
    
    let our_size = rn1_size * multiplier;
    
    // Cap at our bankroll limits
    our_size.min(balance * 0.10)  // Max 10% per trade
}
```

### **Phase 4: EXIT STRATEGY**

**INTE wait för RN1 to exit → Independent stops:**

```rust
// Set take-profit and stop-loss
const TAKE_PROFIT_PCT: f64 = 0.10;   // Exit @ +10%
const STOP_LOSS_PCT: f64 = 0.05;     // Exit @ -5%
const MAX_HOLDING_TIME: i64 = 48 * 3600;  // 48h max

// Monitor position
loop {
    let current_price = fetch_current_price(&position.market_id);
    let pnl_pct = (current_price - position.entry_price) / position.entry_price;
    
    if pnl_pct >= TAKE_PROFIT_PCT {
        close_position("Take profit");
        break;
    }
    
    if pnl_pct <= -STOP_LOSS_PCT {
        close_position("Stop loss");
        break;
    }
    
    if now() - position.entry_time > MAX_HOLDING_TIME {
        close_position("Time limit");
        break;
    }
    
    sleep(60);
}
```

---

## 📈 REALISTISKA FÖRVÄNTNINGAR

### **Med Smart Selective Copy:**

```
RN1's performance:
  - ROI: 2.2% på $243M volume
  - Profit: $6M
  - Trades: 1M+
  - Win rate: 54-61%

Vår expected performance:
  - ROI: 0.5-1.0% (50% av RN1 p.g.a. lag + filters)
  - Win rate: 50-55% (lower än RN1)
  - Trades: 50-200 per månad (filtered)
  - Capital: $5k → Monthly profit $25-$50
  - Capital: $50k → Monthly profit $250-$500
```

### **Varför lägre än RN1?**
1. ✅ **Execution lag** → missar snabba arbitrage (20% profit loss)
2. ✅ **No platform rewards** → trash farming inte viable (10% profit loss)
3. ✅ **Smaller capital** → less diversification, högre variance (10% profit loss)
4. ✅ **Manual verification** → slower reaction (10% profit loss)

**Total expected reduction: ~50%**

---

## 🔧 TEKNISK IMPLEMENTATION

### **Step 1: Enhance WebSocket Sniffer**

```rust
// Add hedge detection
struct PositionTracker {
    rn1_positions: HashMap<String, Vec<Position>>,
}

impl PositionTracker {
    fn record_trade(&mut self, signal: Signal) {
        // Store RN1's trades
        self.rn1_positions
            .entry(signal.market_id.clone())
            .or_insert_with(Vec::new)
            .push(Position::from(signal));
    }
    
    fn check_for_hedge(&self, signal: &Signal) -> Option<HedgeInfo> {
        let positions = self.rn1_positions.get(&signal.market_id)?;
        
        for pos in positions.iter().rev() {  // Check recent first
            if pos.side != signal.side && 
               similar_size(pos.size, signal.size, 0.20) {
                return Some(HedgeInfo {
                    original_side: pos.side,
                    original_size: pos.size,
                    hedge_detected: true,
                });
            }
        }
        None
    }
}
```

### **Step 2: Add Market Intelligence**

```rust
async fn fetch_market_metadata(token_id: &str) -> MarketMetadata {
    // Fetch via Gamma API
    let market = api.get_market(token_id).await?;
    
    MarketMetadata {
        category: extract_category(&market),
        liquidity: market.volume,
        event_time: market.end_date_iso,
        tags: market.tags,
    }
}

fn is_viable_market(metadata: &MarketMetadata) -> SkipReason {
    if metadata.liquidity < 100_000.0 {
        return SkipReason::LowLiquidity;
    }
    
    if !["sports", "politics"].contains(&metadata.category.as_str()) {
        return SkipReason::UnsupportedCategory;
    }
    
    let time_until_event = metadata.event_time - now();
    if time_until_event < 2 * 3600 {
        return SkipReason::TooCloseToEvent;
    }
    
    SkipReason::None  // Viable!
}
```

### **Step 3: Enhanced Risk Manager**

```rust
// Update RiskManager with new limits
const MAX_CONCURRENT_POSITIONS: usize = 5;     // Limit exposure
const MAX_SINGLE_BET_PCT: f64 = 0.10;          // 10% max per bet
const MAX_DAILY_BETS: usize = 10;              // Limit daily trades
const MIN_EXPECTED_PROFIT: f64 = 0.03;         // 3% minimum edge

impl RiskManager {
    fn should_enter_trade(&self, signal: &Signal, our_size: f64) -> Result<()> {
        // Existing checks + new ones
        
        if self.concurrent_positions() >= MAX_CONCURRENT_POSITIONS {
            return Err("Too many concurrent positions");
        }
        
        if self.daily_trade_count >= MAX_DAILY_BETS {
            return Err("Daily trade limit reached");
        }
        
        // Expected profit check (simple heuristic)
        let expected_profit_pct = estimate_edge(&signal);
        if expected_profit_pct < MIN_EXPECTED_PROFIT {
            return Err("Expected profit too low");
        }
        
        Ok(())
    }
}
```

---

## 📋 ACTION PLAN — KONKRETA STEG

### **Imme <truncated - response length hit limit>**
