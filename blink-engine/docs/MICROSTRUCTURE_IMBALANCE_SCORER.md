# Microstructure imbalance scorer

`tools/run_eval_cycle.py microstructure-imbalance-scorer` computes deterministic token-level and market-level microstructure risk scores from eval snapshots.

## Inputs

- Default snapshots source: `logs/eval-cycle/<run-id>/snapshot-*.json`
- Optional override: `--snapshots-dir`
- Uses orderbook-like fields when available (`/api/orderbooks.orderbooks` or `order_books` map)
- Graceful proxy fallback when books are unavailable:
  - `/api/rejections.events` (microstructure reason filter)
  - `derived.gate_rejections_total` window deltas (imbalance/depth/spread/liquidity/drift/failsafe)

## Scoring components

Each token score is normalized to `[0, 1]`:

- `imbalance_pressure` (weight `0.32`)  
  Mean absolute imbalance `|(bid_depth-ask_depth)/(bid_depth+ask_depth)|`
- `spread_stress` (weight `0.24`)  
  Mean spread normalized by 350 bps
- `depth_thinness` (weight `0.18`)  
  Inverse normalized total depth (`bid_depth + ask_depth`)
- `directional_adverse_flow` (weight `0.16`)  
  Fraction of observations where position side is against imbalance direction
- `proxy_gate_pressure` (weight `0.10`)  
  Microstructure rejection pressure from events + gate deltas

Risk levels:

- `LOW`: `< 0.55`
- `MEDIUM`: `>= 0.55` and `< 0.75`
- `HIGH`: `>= 0.75`

When direct orderbook observations are missing, conservative defaults are applied and warnings are emitted (no hard failure).

## Output

Default output path:

- `logs/eval-cycle/<run-id>/microstructure-imbalance.json`

Contains:

- `token_scores[]`
- `market_scores[]`
- `summary` (counts, coverage, mean/median/p95)
- `component_definitions` and `scoring_formula`
- `warnings`

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a microstructure-imbalance-scorer
```

With explicit paths:

```bash
python tools/run_eval_cycle.py --run-id sample-run microstructure-imbalance-scorer \
  --snapshots-dir logs/eval-cycle/sample-run \
  --out-file logs/eval-cycle/sample-run/microstructure-imbalance.sample.json
```

## Full-cycle integration

`full-cycle` now runs the scorer after `report` and includes summary fields in its final JSON line output.

Optional output override:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --baseline-json artifacts/backtest-baseline.json \
  --microstructure-out-file logs/eval-cycle/2026-04-19-paper-a/microstructure-imbalance.json
```
