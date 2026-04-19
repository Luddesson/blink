# Operator Signoff Gate (Pre-Run + Post-Run)

Mandatory operator workflow before deployment/start and after each evaluation window.

## Required artifacts

- Decision thresholds schema: `blink-engine/tools/examples/decision-thresholds.json`
- Decision output per run: `blink-engine/logs/eval-cycle/<run-id>/decision.json`
- Rollback playbook: `deploy/ROLLBACK-PLAYBOOK.md`
- Rollback helper: `deploy/rollback-hetzner.ps1`

If any required artifact is missing, **HARD STOP** (no deploy / no promotion).

## Roles, signers, and record location

- **Primary signer:** On-call operator (executes run/deploy).
- **Secondary signer:** Risk owner/reviewer (independent GO/TUNE/ROLLBACK confirmation).
- Record every signoff in:
  - `deploy/signoffs/<run-id>-<utc-timestamp>.json` (preferred, machine-readable), or
  - incident/ticket system with the same fields as the template.

Use template: `deploy/templates/operator-signoff-record.template.json`.

## Automated signoff packet generation

`run_eval_cycle` can synthesize a machine-readable signoff packet with gate statuses, checklist fields, and missing-artifact diagnostics:

```bash
python tools/run_eval_cycle.py --run-id <run-id> signoff-packet \
  --decision-dimensions-reviewed \
  --out-file deploy/signoffs/<run-id>-<utc>.json
```

The packet includes:

- decision/recommendation/confidence/integrity artifact references
- pass/fail gate statuses (`artifact`, `integrity`, `decision`, `risk_critical`, `pre_run`, `post_run`, `confidence`)
- missing/invalid artifact diagnostics (`missing_artifact_diagnostics`)
- required checklist fields from the operator signoff template

`full-cycle` integration (optional):

```bash
python tools/run_eval_cycle.py --run-id <run-id> full-cycle --generate-signoff-packet
```

## Pre-run signoff gate (before deploy/start)

1. Confirm rollback readiness:
   - `deploy/ROLLBACK-PLAYBOOK.md` reviewed.
   - `.\deploy\rollback-hetzner.ps1` preview run succeeded in current environment.
2. Confirm controls in target `.env`:
   - `TRADING_ENABLED`, `LIVE_TRADING`, `PAPER_TRADING`, `ALPHA_TRADING_ENABLED` set to intended mode.
3. Confirm latest post-run decision from prior window is present and reviewed.
4. Capture signer names and UTC timestamps in signoff record.

### Pre-run HARD STOP criteria

- Missing signer (primary or secondary).
- Rollback preview not verified.
- Missing prior `decision.json` for the run being promoted.
- Prior decision is `ROLLBACK`.
- Prior decision has any `warnings` indicating missing decision artifacts.

## Post-run signoff gate (after evaluation window)

1. Generate and attach `decision.json` with:
   - `python tools/run_eval_cycle.py --run-id <run-id> decision-eval`
2. Review `decision.json` dimensions vs thresholds schema.
3. Record final operator decision (`GO`, `TUNE`, or `ROLLBACK`) and rationale.
4. If decision is `ROLLBACK`, execute rollback playbook immediately and record command/output reference.

### Post-run HARD STOP criteria (no promotion / immediate rollback)

- `decision.json` missing or unreadable.
- `decision == "ROLLBACK"` in `decision.json`.
- Any dimension status is `ROLLBACK`.
- `warnings` in `decision.json` include missing report/drift/conformal artifacts.
- Risk-critical dimension failure:
  - `dimensions.risk_events.status == "ROLLBACK"`, or
  - `dimensions.drift_severity.status == "ROLLBACK"`.

When hard-stop is triggered, operator must:

1. Run `.\deploy\rollback-hetzner.ps1 -Apply` (or manual steps in rollback playbook).
2. Record rollback execution timestamp and verification evidence.
3. Mark signoff record `promotion_allowed = false`.

