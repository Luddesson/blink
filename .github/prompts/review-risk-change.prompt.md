---
mode: agent
description: Safety checklist for any change to the Blink risk manager or position sizing logic.
---

Review the staged/proposed changes to risk or sizing code against this checklist:

**Risk manager (`risk_manager.rs`)**
- [ ] All 7 checks still run in the correct order (kill switch → circuit breaker → daily loss → position cap → order size → rate limit → VaR)
- [ ] `daily_pnl` is only updated in `record_close()`, never in `record_fill()`
- [ ] Circuit breaker auto-trip on daily loss limit still works
- [ ] New config fields use `from_env()` + `unwrap_or(safe_default)` — never panic on missing var
- [ ] Unit tests cover the new check in isolation

**Sizing (`paper_engine.rs` / `conviction_multiplier`)**
- [ ] `SIZE_MULTIPLIER` is still the master knob — changes here affect all position sizes
- [ ] `PAPER_CONFIDENCE_DISCOUNT` (0.35) still discounts mid-range prices (0.40–0.60)
- [ ] `MAX_POSITION_PCT` and `CASH_RESERVE` still enforced before conviction boosts
- [ ] Bullpen boosts are still cold-path (never blocking the hot path)

**Exit strategy (`exit_strategy.rs`)**
- [ ] `evaluate_exits()` remains a pure function with no side effects
- [ ] Tiered `AUTOCLAIM_TIERS` parsing still handles malformed input gracefully

**General**
- [ ] No floats introduced into hot-path order logic
- [ ] `run_autoclaim()` not called from signal or TUI paths
- [ ] `cargo test -p engine` passes — especially proptest suite
- [ ] Formal proofs still hold: `cd blink-engine/formal && make verify`

Flag any change that weakens a safety invariant, even if functionally correct.
