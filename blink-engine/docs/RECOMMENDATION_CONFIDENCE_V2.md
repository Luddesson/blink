# Recommendation confidence v2

`tools/run_eval_cycle.py recommendation-confidence-v2` computes a normalized operator signoff confidence score from available eval artifacts.

## Inputs

- `decision.json`
- `report.json`
- `drift-matrix.json`
- `conformal-summary.json` (optional)
- `purged-walkforward.json` (optional)

Missing optional artifacts are handled gracefully with explicit warnings and conservative fallback scoring.

## Output

Default output: `logs/eval-cycle/<run-id>/recommendation-confidence-v2.json`

Machine-readable fields include:

- `confidence_score_normalized` (0.0–1.0)
- `confidence_score_percent`
- `confidence_level` (`LOW` / `MEDIUM` / `HIGH`)
- `components`:
  - `profitability_stability`
  - `drift_risk`
  - `coverage_reliability`
  - `sample_sufficiency`
- `artifact_availability_factor`
- `signals` (raw sub-signals used by each component)
- `warnings`

## Interpretation (operator signoff use)

- `HIGH` (>= 0.75): signoff-ready candidate, still enforce hard-stop criteria.
- `MEDIUM` (>= 0.55 and < 0.75): conditional signoff; tune and re-check unstable dimensions.
- `LOW` (< 0.55): do not promote; gather more data/tune strategy.

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a recommendation-confidence-v2
```

With explicit sample artifacts:

```bash
python tools/run_eval_cycle.py --run-id sample-run recommendation-confidence-v2 \
  --decision-file logs/eval-cycle/sample-run/decision.sample.json \
  --report-file tools/examples/decision-report.sample.json \
  --drift-file tools/examples/decision-drift.sample.json \
  --conformal-file tools/examples/decision-conformal.sample.json \
  --walkforward-file tools/examples/purged-walkforward-output.sample.json \
  --out-file logs/eval-cycle/sample-run/recommendation-confidence-v2.sample.json
```

## Full-cycle integration

`full-cycle` now attempts confidence v2 synthesis after `decision-eval` and includes score metadata in the final JSON summary when generated.

Optional output override:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --baseline-json artifacts/backtest-baseline.json \
  --confidence-out-file logs/eval-cycle/2026-04-19-paper-a/recommendation-confidence-v2.json
```
