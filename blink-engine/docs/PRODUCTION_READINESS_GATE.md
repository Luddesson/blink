# Production readiness gate

`tools/run_eval_cycle.py production-readiness-gate` synthesizes deploy artifacts into a deterministic final readiness verdict:

- `GO`
- `TUNE`
- `ROLLBACK`

It evaluates:

- artifact integrity
- decision
- confidence
- operator signoff packet
- rollback packet quality + rollback recommendation signal
- anomaly response plan
- threshold policy checks (including decision policy fingerprint match)

The output is machine-readable and includes:

- `gate_statuses.hard` / `gate_statuses.soft`
- `missing_artifact_diagnostics`
- `readiness_verdict`
- `deterministic_fingerprint`

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a production-readiness-gate
```

Explicit deterministic timestamp:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a production-readiness-gate \
  --generated-at-utc 2026-04-20T00:10:00Z
```

Fail CI/CD on non-GO verdict:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a production-readiness-gate \
  --fail-on-non-go
```

## Full-cycle integration

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --run-anomaly-response-automation \
  --generate-signoff-packet \
  --generate-rollback-packet \
  --generate-production-readiness-gate
```

When enabled in `full-cycle`, summary JSON includes:

- `production_readiness_gate.verdict`
- hard/soft failure counts
- output artifact path in `outputs.production_readiness_gate`

## Smoke checks

GO fixture smoke check (deterministic):

```bash
python tools/run_eval_cycle.py --run-id sample-run production-readiness-gate \
  --repo-root . \
  --decision-file tools/examples/production-readiness-decision.sample.json \
  --integrity-file tools/examples/production-readiness-integrity.sample.json \
  --confidence-file tools/examples/production-readiness-confidence.sample.json \
  --signoff-file tools/examples/operator-signoff-packet.sample.json \
  --rollback-packet-file tools/examples/production-readiness-rollback.sample.json \
  --anomaly-response-file tools/examples/production-readiness-anomaly.sample.json \
  --thresholds-json tools/examples/decision-thresholds.json \
  --generated-at-utc 2026-04-20T00:11:00Z \
  --out-file tools/examples/production-readiness-gate.sample.json
```

Missing-artifact diagnostics smoke check:

```bash
python tools/run_eval_cycle.py --run-id sample-run production-readiness-gate \
  --decision-file tools/examples/production-readiness-decision.sample.json \
  --integrity-file tools/examples/production-readiness-integrity.sample.json \
  --confidence-file tools/examples/production-readiness-confidence.sample.json \
  --signoff-file tools/examples/operator-signoff-packet.sample.json \
  --rollback-packet-file tools/examples/production-readiness-rollback.sample.json \
  --anomaly-response-file tools/examples/DOES_NOT_EXIST.json \
  --thresholds-json tools/examples/decision-thresholds.json
```

Example output:

- `tools/examples/production-readiness-gate.sample.json`
