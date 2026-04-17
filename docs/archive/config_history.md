# Configuration History

## 2026-04-01: RN1 Analysis Update

**Changed settings based on deep research of RN1 whale trader:**

### Paper Trading:
- `STARTING_BALANCE_USDC`: 100 → 5000
  - Reason: $5k allows realistic bet sizing ($100-750 per trade)
  
- `SIZE_MULTIPLIER`: 0.02 → 0.05
  - Reason: 5% base aligns with 1/10th of RN1's positions
  - Will be dynamic (5-15%) based on conviction
  
- `MAX_POSITION_PCT`: 0.10 → 0.15
  - Reason: Allow 15% for high-conviction plays
  
- `MIN_TRADE_USDC`: 0.50 → 100.00
  - Reason: $0.50 orders lose 10% to gas
  - $100 orders lose <1% to gas (viable)

### Research Findings:
- RN1: $6M profit, 69% win rate, $96k avg bet
- Strategy: Market-making + Arbitrage + Synthetic hedging
- Filters needed: $10k+ RN1 bets, $100k+ liquidity markets
- Expected performance: 55-60% win rate, 5-10% monthly ROI

## Previous Settings (pre-2026-04-01):

- STARTING_BALANCE_USDC = 100.0
- SIZE_MULTIPLIER = 0.02 (2%)
- MAX_POSITION_PCT = 0.10 (10%)
- MIN_TRADE_USDC = 0.50
