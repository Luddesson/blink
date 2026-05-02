# Rollback packet generator

`tools/run_eval_cycle.py rollback-packet` builds a deterministic rollback decision packet from eval artifacts + deploy rollback metadata.

## What it includes

- rollback triggers
- failed rollout gates
- impacted env/config keys
- rollback steps/checklist aligned with `deploy/ROLLBACK-PLAYBOOK.md`
- evidence artifact references + SHA-256 hashes
- explicit `missing_artifact_diagnostics`
- explicit readiness pass/fail fields (`readiness.pass`, `readiness.status`)

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a rollback-packet \
  --decision-file logs/eval-cycle/2026-04-19-paper-a/decision.json \
  --integrity-file logs/eval-cycle/2026-04-19-paper-a/artifact-integrity.json \
  --confidence-file logs/eval-cycle/2026-04-19-paper-a/recommendation-confidence-v2.json \
  --toxic-flow-file logs/eval-cycle/2026-04-19-paper-a/toxic-flow-advisor.json \
  --signoff-file deploy/signoffs/2026-04-19-paper-a-auto-signoff.json \
  --out-file logs/eval-cycle/2026-04-19-paper-a/rollback-packet.json
```

Deterministic timestamp source:

- pass `--generated-at-utc` to pin timestamp, or
- omit it to use the max timestamp found in input artifacts.

Fail CI if packet is not rollback-ready:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a rollback-packet --fail-on-not-ready
```

## Full-cycle integration

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --baseline-json artifacts/backtest-baseline.json \
  --generate-rollback-packet
```

Optional strict full-cycle failure when rollback packet readiness is `FAIL`:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --baseline-json artifacts/backtest-baseline.json \
  --generate-rollback-packet \
  --rollback-packet-fail-on-not-ready
```

See sample payload: `tools/examples/rollback-packet.sample.json`.
