# Contextual bandit allocation simulation

`tools/contextual_bandit_allocation.py` runs an **offline replay simulation** for strategy allocation across:

- `mirror`
- `conservative`
- `aggressive`

It is analysis-only and uses Python stdlib only (no heavy dependencies).

## Data source

By default, the script scans `logs/eval-cycle/**/report.json` and pairs each report with sibling `fingerprint.json` to read:

- context quality features from `report.json` (`window.funnel_delta`, `health.ws_connected_ratio`, gate pressure)
- logged strategy mode from `fingerprint.json.strategy_mode_hint`

Only runs tagged with one of the three strategy modes are included.

If no compatible artifacts are found, the script auto-falls back to synthetic samples so CI/local validation still works.  
Use `--no-fallback-synthetic` to make this a hard failure instead.

## Algorithm

Choose one policy variant:

- `--algorithm linucb`: score = `xᵀθ + α * sqrt(xᵀA⁻¹x)`
- `--algorithm linucb-safe` (**default**): LinUCB + uncertainty-aware alpha scaling + allocation guardrails (`min/max` per arm, plus aggressive-arm cap)
- `--algorithm epsilon-greedy`: linear value estimate with random exploration `ε`

Replay policy:

- At each step, the bandit chooses an arm from context.
- Reward is only applied/learned when chosen arm equals the logged historical arm (`matched_replay=true`).

This keeps simulation faithful to historical/replay constraints.

## Off-policy evaluation (OPE)

In addition to matched replay metrics, the simulator now emits deterministic OPE estimates:

- **IPS** reward estimate (with clipping via `--ips-weight-cap`)
- **SNIPS** reward estimate
- **Direct Method (DM)** reward estimate (linear reward model)
- **Doubly Robust (DR)** reward estimate
- Effective sample size + clipping rate diagnostics

Logged propensities are approximated from empirical arm frequencies with floor `--logging-propensity-floor`.

## Usage

```bash
python tools/contextual_bandit_allocation.py \
  --eval-root logs/eval-cycle \
  --algorithm linucb-safe \
  --alpha 0.6 \
  --out-dir logs/bandit-allocation-sim
```

Synthetic validation:

```bash
python tools/contextual_bandit_allocation.py \
  --synthetic-steps 120 \
  --algorithm epsilon-greedy \
  --epsilon 0.15 \
  --out-dir logs/bandit-allocation-sim-synthetic
```

Run through `run_eval_cycle`:

```bash
python tools/run_eval_cycle.py bandit-allocation \
  --eval-root logs/eval-cycle \
  --algorithm linucb-safe \
  --out-dir logs/bandit-allocation-sim
```

## Outputs

Under `--out-dir`:

- `summary.json`
  - matched replay coverage + reward stats
  - arm counts + average allocation probabilities
  - guardrail trigger stats
  - OPE metrics (`ips`, `snips`, `dm`, `dr`, ESS, clipping rate)
  - explicit warning list (`warnings`) for missing/invalid optional artifacts
- `allocation-history.json`
- `allocation-history.csv`

History rows include step index, logged/chosen arm, replay match flag, observed/applied reward, per-arm scores/probabilities, and per-step IPS/DM/DR contributions.

Example summary artifact: `tools/examples/bandit-allocation-summary.sample.json`.
