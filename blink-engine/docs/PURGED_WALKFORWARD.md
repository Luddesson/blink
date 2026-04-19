# Purged walk-forward validation

`tools/purged_walkforward.py` adds deterministic, anti-leakage fold generation for time-ordered strategy evaluation datasets.

## What it does

- Sorts observations chronologically (`timestamp` + optional tie-break `id` column).
- Builds deterministic walk-forward folds (`expanding`, `rolling`, or `both-sides` train policy).
- Applies anti-leakage guards:
  - `purge_size`: rows removed around each test interval
  - `embargo_size`: extra rows removed after each test interval
- Emits machine-readable fold metadata and aggregate fold metrics.

## Input schema (generic)

Input can be `csv`, `json` (array of objects), or `jsonl`.

Required:
- `timestamp` (or `--timestamp-col`) in ISO-8601 UTC format

Optional:
- `token_id`/`event_id` or any key as deterministic tiebreak (`--id-col`)
- numeric feature columns for fold summaries (`--metric-col`)
- target/prediction columns (`--target-col`, `--prediction-col`) for per-fold regression and binary classification metrics

Example input: `tools/examples/purged-walkforward-input.sample.csv`

## Output schema

Primary fields:
- `schema_version`
- `input` (path, format, row counts)
- `config` (all split parameters)
- `dataset_window_utc`
- `split_fingerprint_sha256` (deterministic split signature)
- `folds[]`:
  - `counts` train/test
  - `ranges` train/test index ranges + time ranges
  - `anti_leakage` test interval + excluded train interval + purge/embargo settings
  - `metrics` fold metrics
- `aggregate.metrics_by_fold` (mean/min/max across numeric fold metrics)

Example output: `tools/examples/purged-walkforward-output.sample.json`

## Usage

```bash
python tools/purged_walkforward.py \
  --input tools/examples/purged-walkforward-input.sample.csv \
  --output logs/eval-cycle/purged-walkforward.json \
  --timestamp-col timestamp \
  --id-col token_id \
  --n-splits 3 \
  --test-size 2 \
  --min-train-size 4 \
  --purge-size 1 \
  --embargo-size 1 \
  --train-policy both-sides \
  --metric-col edge_bps \
  --metric-col signal_notional_usdc \
  --target-col target \
  --prediction-col prediction
```

## Integration with `run_eval_cycle.py`

Direct command:

```bash
python tools/run_eval_cycle.py walkforward \
  --input tools/examples/purged-walkforward-input.sample.csv \
  --output logs/eval-cycle/purged-walkforward.json \
  --n-splits 3 --test-size 2 --min-train-size 4 \
  --purge-size 1 --embargo-size 1 --train-policy both-sides
```

Optional full-cycle integration:

```bash
python tools/run_eval_cycle.py --run-id sample-run full-cycle \
  --walkforward-input tools/examples/purged-walkforward-input.sample.csv \
  --walkforward-input-format csv \
  --walkforward-n-splits 3 \
  --walkforward-test-size 2 \
  --walkforward-min-train-size 4 \
  --walkforward-purge-size 1 \
  --walkforward-embargo-size 1 \
  --walkforward-train-policy both-sides
```
