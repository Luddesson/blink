# Toxic-flow advisor

`tools/run_eval_cycle.py toxic-flow-advisor` computes a deterministic adverse-selection / toxic-flow score and emits automation-ready guardrail recommendations.

## Inputs

- `snapshot-*.json` (default: `logs/eval-cycle/<run-id>/`)
- `report.json` (default: `logs/eval-cycle/<run-id>/report.json`)
- `rejections.json` (optional explicit file; snapshot `/api/rejections` is used as fallback)
- `microstructure-imbalance.json` (optional, preferred spread/imbalance proxy source)
- `execution-drag-attribution.json` (optional fallback for spread/drag proxies)

Missing artifacts are handled with warnings and conservative fallback scoring (no hard failure).

## Scoring components

Each component is normalized to `[0,1]` and combined into `toxic_flow_score_normalized`:

- `adverse_move_proxy` (weight `0.40`)
  - sequential adverse price moves on open positions (`YES` down / `NO` up)
  - uses adverse step ratio + average adverse move bps
- `reject_pressure` (weight `0.25`)
  - rejection/abort pressure vs window signals
  - microstructure rejection share from reasons and gate deltas
- `spread_imbalance_proxy` (weight `0.25`)
  - prefers `microstructure-imbalance.json` score summary
  - falls back to orderbook spread/imbalance proxies from snapshots
  - final fallback uses execution-drag bps/defaults
- `execution_drag_proxy` (weight `0.10`)
  - prefers drag attribution aggregate bps/pct
  - fallback to report fee drag/defaults

Severity tiers:

- `LOW` `< 0.45`
- `ELEVATED` `>= 0.45` and `< 0.65`
- `HIGH` `>= 0.65` and `< 0.82`
- `SEVERE` `>= 0.82`

## Output

Default output: `logs/eval-cycle/<run-id>/toxic-flow-advisor.json`

Machine-readable fields include:

- `toxic_flow_score_normalized`
- `toxic_flow_score_percent`
- `severity_tier`
- `operator_signoff_status` (`PASS` / `CONDITIONAL` / `HOLD`)
- `components` (weights, contributions, raw signals)
- `guardrail_recommendations.actions[]`
- `guardrail_recommendations.env_overrides`
- `warnings`

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a toxic-flow-advisor
```

With explicit artifact overrides:

```bash
python tools/run_eval_cycle.py --run-id sample-run toxic-flow-advisor \
  --snapshots-dir logs/eval-cycle/sample-run \
  --report-file tools/examples/decision-report.sample.json \
  --execution-drag-file tools/examples/execution-drag-attribution.sample.json \
  --out-file logs/eval-cycle/sample-run/toxic-flow-advisor.sample.json
```

## Full-cycle integration

`full-cycle` runs `toxic-flow-advisor` after microstructure scoring (optional step, non-fatal unless `--strict`).

Optional output override:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --baseline-json artifacts/backtest-baseline.json \
  --toxic-flow-out-file logs/eval-cycle/2026-04-19-paper-a/toxic-flow-advisor.json
```
