# Anomaly response automation

`tools/run_eval_cycle.py anomaly-response-automation` detects evaluation-session anomalies and emits a deterministic, machine-readable response plan for control-plane automation.

## Covered anomaly conditions

- `severe_drift` (fill-rate/slippage/distribution drift threshold breaches)
- `severe_toxic_flow` (`toxic-flow-advisor` severity `HIGH`/`SEVERE`)
- `repeated_integrity_failures` (artifact-integrity failure counts above thresholds)
- `confidence_collapse` (confidence score below floor / low-confidence collapse)

Missing artifacts are handled gracefully with conservative fallbacks + warnings.

## Safety constraints

- Control-plane only: no live-trading mutation actions are emitted.
- `automation_safe_action_list` explicitly marks `live_trade_mutation=false` for every action.
- Output is deterministic for the same artifacts + thresholds (`deterministic_fingerprint` included).

## Output

Default output: `logs\eval-cycle\<run-id>\anomaly-response-plan.json`

Key fields:

- `summary.highest_severity`
- `summary.escalation_channel`
- `recommended_mitigations[]`
- `automation_safe_action_list[]`
- `escalation`
- `warnings`

## Usage

```bash
python tools\run_eval_cycle.py --run-id 2026-04-19-paper-a anomaly-response-automation
```

With explicit artifact paths and severity exit gate:

```bash
python tools\run_eval_cycle.py --run-id sample-run anomaly-response-automation ^
  --drift-file logs\eval-cycle\sample-run\drift-matrix.json ^
  --toxic-flow-file logs\eval-cycle\sample-run\toxic-flow-advisor.json ^
  --integrity-file logs\eval-cycle\sample-run\artifact-integrity.json ^
  --confidence-file logs\eval-cycle\sample-run\recommendation-confidence-v2.json ^
  --fail-on-severity high
```

## Full-cycle hook

Enable as an optional full-cycle step:

```bash
python tools\run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle ^
  --baseline-json artifacts\backtest-baseline.json ^
  --run-anomaly-response-automation ^
  --anomaly-fail-on-severity high
```

