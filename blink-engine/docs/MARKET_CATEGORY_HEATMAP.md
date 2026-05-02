# Market category heatmap

`tools/run_eval_cycle.py market-category-heatmap` builds deterministic category-level analytics for eval runs.

## Inputs

- `snapshot-*.json` (required; uses latest snapshot for category/trade/rejection/alpha views)
- `funnel-rollup.json` (optional fallback for window timestamps)
- `report.json` (optional, for run-level return context in summary)
- `decision.json` (optional, included in summary when present)
- `recommendation-confidence-v2.json` (optional, included in summary when present)

Missing category metadata is handled gracefully via fallback order:
1. explicit category fields (for example `fee_category`)
2. market title keyword classification
3. `uncategorized`

## Output

Default output: `logs/eval-cycle/<run-id>/market-category-heatmap.json`

Machine-readable sections:

- `summary` — run-level totals and optional recommendation confidence context
- `categories[]` — per-category metrics:
  - trade returns / win-rate
  - fee drag
  - rejection pressure
  - confidence/recommendation outcomes (from alpha signal history when present)
- `heatmap.columns` + `heatmap.rows` — deterministic UI/report matrix payload
- `warnings` — explicit data-coverage gaps, never silent failure

Example artifact: `tools/examples/market-category-heatmap.sample.json`

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a market-category-heatmap
```

With explicit output path:

```bash
python tools/run_eval_cycle.py --run-id sample-run market-category-heatmap \
  --out-file logs/eval-cycle/sample-run/market-category-heatmap.json
```

## Full-cycle integration

Optional non-fatal hook:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --run-market-category-heatmap \
  --market-category-heatmap-out-file logs/eval-cycle/2026-04-19-paper-a/market-category-heatmap.json
```
