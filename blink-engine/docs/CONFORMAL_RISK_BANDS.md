# Conformal risk bands (offline)

`tools/conformal_risk_bands.py` computes calibrated prediction intervals from eval snapshots produced by `tools/run_eval_cycle.py`.

## Inputs

- Snapshot tree (recursive): `snapshot-*.json`
- Slippage metric source: `/api/portfolio.avg_slippage_bps`
- Return metric source: adjacent NAV deltas from `/api/portfolio.total_nav`

## Method

For each metric independently:
1. Sort samples chronologically.
2. Split into train / calibration / eval (defaults `60% / 20% / 20%`).
3. Use train median as point predictor.
4. Compute calibration nonconformity scores `|y - y_hat|`.
5. For each alpha, compute split-conformal `qhat` and interval `[y_hat - qhat, y_hat + qhat]`.
6. Report eval coverage and interval width.

## Output artifact

Writes one machine-readable JSON artifact (default: `logs/eval-cycle/conformal-risk-bands.json`) with:

- config (`alphas`, split fractions, method)
- per-metric bands (`lower`, `upper`, `width`, `qhat_abs_residual`)
- empirical coverage on calibration and eval splits
- sample counts/window metadata and warnings

## Example

```bash
python tools/conformal_risk_bands.py \
  --snapshots logs/eval-cycle \
  --out-file logs/eval-cycle/conformal-risk-bands.json \
  --alphas 0.20,0.10,0.05
```
