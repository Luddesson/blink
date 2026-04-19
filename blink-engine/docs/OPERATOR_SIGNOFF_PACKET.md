# Operator signoff packet generator

`tools/run_eval_cycle.py signoff-packet` assembles a deploy-ready operator signoff JSON from existing eval outputs.

## Inputs

- `decision.json`
- `regime-conditional-recommendations.json` (optional recommendation artifact)
- `recommendation-confidence-v2.json`
- `artifact-integrity.json`
- required gate artifacts:
  - `blink-engine/tools/examples/decision-thresholds.json`
  - `deploy/ROLLBACK-PLAYBOOK.md`
  - `deploy/rollback-hetzner.ps1`

## Output

Default output path:

- `deploy/signoffs/<run-id>-<utc>.json`

Key sections:

- template-aligned signoff fields (`signers`, `pre_run`, `post_run`, `promotion_allowed`)
- `gate_statuses` with PASS/FAIL/MISSING
- `missing_artifact_diagnostics` + `artifact_diagnostics`
- source summaries from decision/confidence/integrity/recommendation artifacts

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a signoff-packet \
  --environment hetzner-prod \
  --primary-operator-name "oncall-1" \
  --primary-operator-signed-at-utc "2026-04-20T00:01:00Z" \
  --secondary-reviewer-name "risk-1" \
  --secondary-reviewer-signed-at-utc "2026-04-20T00:02:00Z" \
  --rollback-preview-verified \
  --target-mode-verified \
  --prior-decision-reviewed \
  --decision-dimensions-reviewed
```

`full-cycle` integration:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --generate-signoff-packet \
  --signoff-environment hetzner-prod \
  --signoff-decision-dimensions-reviewed
```

When generated from `full-cycle`, summary JSON includes `signoff_packet` status + output path.
