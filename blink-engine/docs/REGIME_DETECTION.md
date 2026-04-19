# Regime detection (offline)

`tools/regime_detection.py` segments a snapshot run into transparent risk/trend regimes without touching hot-path logic.

## Inputs

- Snapshot root from `tools/run_eval_cycle.py` (recursive): `snapshot-*.json`
- NAV source: `/api/portfolio.total_nav`

## Method

For each snapshot step:
1. Compute return from previous NAV.
2. Compute rolling volatility and rolling mean return.
3. Compute robust volatility z-score (median/MAD; std fallback) vs historical rolling-vol baseline.
4. Compute trend t-score (`mean / vol * sqrt(window)`) and drawdown.
5. Compute lightweight change-point score from adjacent rolling-window mean/vol shifts.
6. Classify regime:
   - `high_volatility` / `low_volatility` via robust volatility z-score,
   - `trend_up` / `trend_down` via trend t-score,
   - `drawdown_stress` when drawdown is deep under elevated vol,
   - `neutral` otherwise,
   - `warmup` before enough samples.

Then collapse labels into segments, with optional change-point boundaries (minimum segment length guard).

## Output

- `regime-points.csv` — per-sample regime metrics.
- `regime-points.json` — same per-sample data in JSON for downstream postrun tooling.
- `regime-segments.json` — segment list with regime + strategy routing hint.
- `regime-summary.json` — metadata + transitions + segments.

## Example

```bash
python tools/regime_detection.py \
  --snapshots logs/eval-cycle/2026-04-19-paper-a \
  --out-dir logs/eval-cycle/2026-04-19-paper-a/regimes \
  --window 12 \
  --high-vol-z 0.8 \
  --low-vol-z -0.8 \
  --trend-t-threshold 0.75 \
  --change-point-score 2.25
```
