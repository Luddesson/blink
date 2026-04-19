# Triple-Barrier / Meta-Labeling Pipeline (Offline)

`tools/triple_barrier_labeling.py` evaluates whether raw signals were execution-worthy without touching the live/paper hot path.

## Inputs

- `--signals`: signal events (`.csv`, `.json`, `.jsonl`)
- `--prices`: price events (`.csv`, `.json`, `.jsonl`)  
  If omitted, the script reuses `--signals` as a combined source.

Supported signal keys (first match wins):
- timestamp: `timestamp_ms`, `timestamp`, `ts`, `captured_at_utc`, `time`
- token: `token_id`, `asset_id`, `market_id`
- side: `side`, `signal_side`, `direction` (`YES/BUY/LONG` or `NO/SELL/SHORT`)
- entry price: `entry_price_scaled`, `entry_price`, `price_scaled`, `price`

Supported price keys:
- timestamp: same timestamp aliases
- token: same token aliases
- price: `price_scaled`, `yes_price_scaled`, `mid_price_scaled`, `price`, `yes_price`

Price values are interpreted as:
- scaled if `> 1.0` (Blink style, e.g. `650`)
- decimal if `<= 1.0` (auto-scaled, e.g. `0.65` → `650`)

## Labeling

For each signal and each requested horizon:
1. Track forward prices until the vertical horizon.
2. First hit of take-profit barrier → `triple_label=1`, `meta_label=1`.
3. First hit of stop-loss barrier → `triple_label=-1`, `meta_label=0`.
4. If no horizontal barrier hit, use the terminal return at horizon (`vertical_barrier`).
5. If insufficient forward bars, mark `hit_type=insufficient_data`.

## Outputs

- `labels.csv`: one row per `signal × horizon`
- `summary.json`: per-horizon counts, meta positive-rate, average return (bps)

## Example

```bash
python tools/triple_barrier_labeling.py \
  --signals tools/examples/meta_labeling_signals.csv \
  --prices tools/examples/meta_labeling_prices.csv \
  --horizons-minutes 5,15,30 \
  --take-profit-bps 150 \
  --stop-loss-bps 100 \
  --out-dir logs/meta-labeling
```
