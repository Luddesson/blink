# Execution drag attribution

`tools/run_eval_cycle.py execution-drag-attribution` decomposes eval-session execution drag into:

- spread proxy drag
- residual slippage drag
- paid fees drag
- configured entry-delay drag
- rejection opportunity-cost drag

## Inputs

- `snapshot-*.json` (required; from `start`/`snapshot`)
- `funnel-rollup.json` (optional fallback auto-computed if missing)
- `fingerprint.json` (optional; used to read `ENTRY_DELAY_SECS` / `PAPER_ADVERSE_FILL_BPS` from env file)

## Output

Default output: `logs/eval-cycle/<run-id>/execution-drag-attribution.json`

Top-level schema:

```json
{
  "schema_version": 1,
  "generated_at_utc": "string",
  "run_id": "string",
  "window": { "snapshot_count": 0, "window_start_utc": "string|null", "window_end_utc": "string|null" },
  "funnel": { "signals": 0, "accepted": 0, "fills": 0, "rejections": 0, "aborts": 0 },
  "notional": {
    "executed_notional_usdc_est": 0.0,
    "rejected_notional_usdc_est": 0.0
  },
  "components": {
    "spread": { "drag_usdc": 0.0, "drag_bps_of_executed_notional": 0.0, "share_of_total_drag_pct": 0.0, "method": "..." },
    "slippage": { "drag_usdc": 0.0, "drag_bps_of_executed_notional": 0.0, "share_of_total_drag_pct": 0.0, "method": "..." },
    "fees": { "drag_usdc": 0.0, "drag_bps_of_executed_notional": 0.0, "share_of_total_drag_pct": 0.0, "method": "..." },
    "delay": { "drag_usdc": 0.0, "drag_bps_of_executed_notional": 0.0, "share_of_total_drag_pct": 0.0, "method": "..." },
    "rejections": { "drag_usdc": 0.0, "drag_bps_of_executed_notional": 0.0, "share_of_total_drag_pct": 0.0, "method": "..." }
  },
  "aggregate": {
    "total_drag_usdc": 0.0,
    "total_drag_bps_of_executed_notional": 0.0,
    "total_drag_pct_of_start_nav": 0.0
  },
  "assumptions": {
    "spread_bps_proxy": 10.0,
    "entry_delay_secs": 0.0,
    "delay_bps_per_second": 2.0,
    "rejection_edge_bps": 50.0
  }
}
```

## Usage

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a execution-drag-attribution
```

With explicit assumptions:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a execution-drag-attribution \
  --spread-bps 12 \
  --entry-delay-secs 2 \
  --delay-bps-per-second 2.5 \
  --rejection-edge-bps 60
```

Full-cycle optional hook:

```bash
python tools/run_eval_cycle.py --run-id 2026-04-19-paper-a full-cycle \
  --run-execution-drag-attribution \
  --execution-drag-out-file logs/eval-cycle/2026-04-19-paper-a/execution-drag-attribution.json
```

## Notes

- Spread and delay components are model-based proxies (explicit in `assumptions`).
- Rejection drag is opportunity-cost based (`rejection_edge_bps`).
- Output is machine-readable and safe for downstream dashboards/registry pipelines.
