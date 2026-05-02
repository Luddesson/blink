# Regime-conditional recommender

`tools/regime_conditional_recommender.py` generates deterministic, machine-readable parameter recommendations per detected regime segment.

## Inputs

- Required:
  - `regimes/regime-summary.json` (from `tools/regime_detection.py`)
  - `report.json`
  - `drift-matrix.json`
- Optional:
  - explicit rejections artifact (`--rejections-file`)
  - latest snapshot fallback (`snapshot-*.json` in run dir)

If rejections are unavailable, the tool falls back to `report.gate_pressure_top5_run_window`.

## Output

Default output: `logs/eval-cycle/<run-id>/regime-conditional-recommendations.json`

Per regime block includes:

- `recommendations[]` with deterministic rank and score
- `rationale[]`
- `confidence` (`score_normalized`, `%`, `LOW|MEDIUM|HIGH`)
- `metrics` and `signal_strength`

Deterministic ordering:

- Regimes sorted by `(-confidence.score_normalized, regime, segment_window.start_utc)`
- Recommendations sorted by `(-score, parameter)`

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a regime-conditional-recommender
```

Explicit artifacts:

```bash
python tools/run_eval_cycle.py --run-id sample-run regime-conditional-recommender \
  --regime-file logs/eval-cycle/sample-run/regimes/regime-summary.json \
  --report-file tools/examples/decision-report.sample.json \
  --drift-file tools/examples/decision-drift.sample.json \
  --out-json logs/eval-cycle/sample-run/regime-conditional-recommendations.sample.json
```

## Full-cycle hook

Enable optional synthesis in `full-cycle`:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --baseline-json artifacts/backtest-baseline.json \
  --run-regime-conditional-recommender
```
