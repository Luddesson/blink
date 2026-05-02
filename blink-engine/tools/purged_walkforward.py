#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import datetime as dt
import hashlib
import json
import math
import statistics
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class Observation:
    original_index: int
    timestamp: dt.datetime
    payload: dict[str, Any]


@dataclass(frozen=True)
class FoldPlan:
    fold_id: int
    train_indices: list[int]
    test_indices: list[int]
    test_start: int
    test_end: int
    excluded_start: int
    excluded_end: int


def now_utc_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def _parse_timestamp(value: Any) -> dt.datetime | None:
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return dt.datetime.fromtimestamp(float(value), tz=dt.timezone.utc)
    if not isinstance(value, str):
        return None
    raw = value.strip()
    if not raw:
        return None
    try:
        if raw.endswith("Z"):
            return dt.datetime.fromisoformat(raw.replace("Z", "+00:00")).astimezone(dt.timezone.utc)
        parsed = dt.datetime.fromisoformat(raw)
        if parsed.tzinfo is None:
            return parsed.replace(tzinfo=dt.timezone.utc)
        return parsed.astimezone(dt.timezone.utc)
    except ValueError:
        return None


def _coerce_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        try:
            return float(value.strip())
        except ValueError:
            return None
    return None


def _read_rows(input_path: Path, input_format: str) -> list[dict[str, Any]]:
    if input_format == "csv":
        with input_path.open("r", encoding="utf-8", newline="") as handle:
            reader = csv.DictReader(handle)
            return [dict(row) for row in reader]
    if input_format == "json":
        payload = json.loads(input_path.read_text(encoding="utf-8"))
        if not isinstance(payload, list):
            raise ValueError("JSON input must be an array of objects.")
        rows = [row for row in payload if isinstance(row, dict)]
        return [dict(row) for row in rows]
    if input_format == "jsonl":
        rows: list[dict[str, Any]] = []
        for line in input_path.read_text(encoding="utf-8").splitlines():
            stripped = line.strip()
            if not stripped:
                continue
            parsed = json.loads(stripped)
            if isinstance(parsed, dict):
                rows.append(dict(parsed))
        return rows
    raise ValueError(f"Unsupported input format: {input_format}")


def _infer_format(input_path: Path, explicit: str) -> str:
    if explicit != "auto":
        return explicit
    suffix = input_path.suffix.lower()
    if suffix == ".csv":
        return "csv"
    if suffix in (".json",):
        return "json"
    if suffix in (".jsonl", ".ndjson"):
        return "jsonl"
    raise ValueError("Unable to infer format. Pass --input-format {csv,json,jsonl}.")


def _build_observations(rows: list[dict[str, Any]], timestamp_col: str, id_col: str) -> list[Observation]:
    observations: list[Observation] = []
    for idx, row in enumerate(rows):
        ts = _parse_timestamp(row.get(timestamp_col))
        if ts is None:
            continue
        observations.append(Observation(original_index=idx, timestamp=ts, payload=row))

    observations.sort(
        key=lambda obs: (
            obs.timestamp,
            str(obs.payload.get(id_col, "")) if id_col else "",
            obs.original_index,
        )
    )
    return observations


def _contiguous_ranges(indices: list[int]) -> list[list[int]]:
    if not indices:
        return []
    ranges: list[list[int]] = []
    start = indices[0]
    prev = indices[0]
    for value in indices[1:]:
        if value == prev + 1:
            prev = value
            continue
        ranges.append([start, prev + 1])
        start = value
        prev = value
    ranges.append([start, prev + 1])
    return ranges


def _build_fold_plans(
    *,
    row_count: int,
    n_splits: int,
    test_size: int | None,
    min_train_size: int,
    purge_size: int,
    embargo_size: int,
    train_policy: str,
    train_window_size: int | None,
) -> list[FoldPlan]:
    if row_count <= 0:
        raise ValueError("No valid observations after parsing timestamps.")
    if n_splits < 1:
        raise ValueError("--n-splits must be >= 1.")
    if min_train_size < 1:
        raise ValueError("--min-train-size must be >= 1.")
    if purge_size < 0 or embargo_size < 0:
        raise ValueError("--purge-size and --embargo-size must be >= 0.")

    if test_size is None:
        remaining = row_count - min_train_size
        if remaining < n_splits:
            raise ValueError("Not enough rows for requested n_splits with current min_train_size.")
        test_size = max(1, remaining // n_splits)
    if test_size < 1:
        raise ValueError("--test-size must be >= 1.")

    first_test_start = row_count - (n_splits * test_size)
    if first_test_start < min_train_size:
        raise ValueError("Invalid split geometry. Increase rows or reduce n_splits/test_size/min_train_size.")

    fold_plans: list[FoldPlan] = []
    all_indices = list(range(row_count))
    for fold_id in range(n_splits):
        test_start = first_test_start + fold_id * test_size
        test_end = min(test_start + test_size, row_count)
        if test_start >= test_end:
            raise ValueError("Computed empty test fold; adjust split parameters.")
        test_indices = list(range(test_start, test_end))

        if train_policy == "expanding":
            candidate_train = list(range(0, test_start))
        elif train_policy == "rolling":
            if train_window_size is None or train_window_size < 1:
                raise ValueError("--train-window-size must be provided and >=1 when --train-policy=rolling.")
            candidate_start = max(0, test_start - train_window_size)
            candidate_train = list(range(candidate_start, test_start))
        elif train_policy == "both-sides":
            candidate_train = [idx for idx in all_indices if idx < test_start or idx >= test_end]
        else:
            raise ValueError(f"Unknown train policy: {train_policy}")

        excluded_start = max(0, test_start - purge_size)
        excluded_end = min(row_count, test_end + purge_size + embargo_size)
        train_indices = [idx for idx in candidate_train if not (excluded_start <= idx < excluded_end)]

        if len(train_indices) < min_train_size:
            raise ValueError(
                f"Fold {fold_id} has train_count={len(train_indices)} < min_train_size={min_train_size}. "
                "Reduce purge/embargo or adjust split parameters."
            )

        fold_plans.append(
            FoldPlan(
                fold_id=fold_id,
                train_indices=train_indices,
                test_indices=test_indices,
                test_start=test_start,
                test_end=test_end,
                excluded_start=excluded_start,
                excluded_end=excluded_end,
            )
        )
    return fold_plans


def _regression_metrics(targets: list[float], preds: list[float]) -> dict[str, float]:
    errors = [pred - target for pred, target in zip(preds, targets)]
    mae = statistics.fmean(abs(err) for err in errors)
    mse = statistics.fmean(err * err for err in errors)
    rmse = math.sqrt(mse)
    bias = statistics.fmean(errors)
    return {
        "mae": round(mae, 10),
        "rmse": round(rmse, 10),
        "mean_error": round(bias, 10),
    }


def _classification_metrics(targets: list[int], preds: list[int]) -> dict[str, float]:
    tp = sum(1 for t, p in zip(targets, preds) if t == 1 and p == 1)
    tn = sum(1 for t, p in zip(targets, preds) if t == 0 and p == 0)
    fp = sum(1 for t, p in zip(targets, preds) if t == 0 and p == 1)
    fn = sum(1 for t, p in zip(targets, preds) if t == 1 and p == 0)
    total = len(targets)
    accuracy = (tp + tn) / total if total else 0.0
    precision = tp / (tp + fp) if (tp + fp) > 0 else 0.0
    recall = tp / (tp + fn) if (tp + fn) > 0 else 0.0
    f1 = (2 * precision * recall / (precision + recall)) if (precision + recall) > 0 else 0.0
    return {
        "accuracy": round(accuracy, 10),
        "precision": round(precision, 10),
        "recall": round(recall, 10),
        "f1": round(f1, 10),
    }


def _fold_metric_payload(
    *,
    observations: list[Observation],
    fold: FoldPlan,
    metric_cols: list[str],
    target_col: str,
    prediction_col: str,
) -> dict[str, Any]:
    metrics: dict[str, Any] = {}
    train_rows = [observations[idx].payload for idx in fold.train_indices]
    test_rows = [observations[idx].payload for idx in fold.test_indices]

    for metric_col in metric_cols:
        train_vals = [_coerce_float(row.get(metric_col)) for row in train_rows]
        test_vals = [_coerce_float(row.get(metric_col)) for row in test_rows]
        train_clean = [value for value in train_vals if value is not None]
        test_clean = [value for value in test_vals if value is not None]
        metrics[f"{metric_col}_train_mean"] = round(statistics.fmean(train_clean), 10) if train_clean else None
        metrics[f"{metric_col}_test_mean"] = round(statistics.fmean(test_clean), 10) if test_clean else None
        if train_clean and test_clean:
            metrics[f"{metric_col}_delta_test_minus_train"] = round(
                statistics.fmean(test_clean) - statistics.fmean(train_clean),
                10,
            )
        else:
            metrics[f"{metric_col}_delta_test_minus_train"] = None

    if target_col and prediction_col:
        paired_targets: list[float] = []
        paired_preds: list[float] = []
        for row in test_rows:
            target = _coerce_float(row.get(target_col))
            pred = _coerce_float(row.get(prediction_col))
            if target is None or pred is None:
                continue
            paired_targets.append(target)
            paired_preds.append(pred)
        metrics["paired_observations"] = len(paired_targets)
        if paired_targets:
            metrics["regression"] = _regression_metrics(paired_targets, paired_preds)
            is_binary = all(value in (0.0, 1.0) for value in paired_targets) and all(
                value in (0.0, 1.0) for value in paired_preds
            )
            if is_binary:
                metrics["classification"] = _classification_metrics(
                    [int(value) for value in paired_targets],
                    [int(value) for value in paired_preds],
                )
    return metrics


def _aggregate_numeric_metrics(folds: list[dict[str, Any]]) -> dict[str, Any]:
    numeric_series: dict[str, list[float]] = {}
    for fold in folds:
        metric_payload = fold.get("metrics")
        if not isinstance(metric_payload, dict):
            continue
        stack: list[tuple[str, Any]] = [(key, value) for key, value in metric_payload.items()]
        while stack:
            key, value = stack.pop()
            if isinstance(value, dict):
                stack.extend((f"{key}.{nested_key}", nested_value) for nested_key, nested_value in value.items())
                continue
            if isinstance(value, bool):
                continue
            if isinstance(value, (int, float)):
                numeric_series.setdefault(key, []).append(float(value))

    aggregated: dict[str, Any] = {}
    for key, values in sorted(numeric_series.items()):
        aggregated[key] = {
            "mean": round(statistics.fmean(values), 10),
            "min": round(min(values), 10),
            "max": round(max(values), 10),
        }
    return aggregated


def run_walkforward(args: argparse.Namespace) -> int:
    input_path = Path(args.input).resolve()
    if not input_path.exists():
        raise FileNotFoundError(f"Input dataset not found: {input_path}")

    input_format = _infer_format(input_path, args.input_format)
    raw_rows = _read_rows(input_path, input_format)
    observations = _build_observations(raw_rows, args.timestamp_col, args.id_col)
    if not observations:
        raise RuntimeError("No observations parsed; verify timestamp column and input data.")

    fold_plans = _build_fold_plans(
        row_count=len(observations),
        n_splits=args.n_splits,
        test_size=args.test_size,
        min_train_size=args.min_train_size,
        purge_size=args.purge_size,
        embargo_size=args.embargo_size,
        train_policy=args.train_policy,
        train_window_size=args.train_window_size,
    )

    folds: list[dict[str, Any]] = []
    for fold in fold_plans:
        train_times = [observations[idx].timestamp for idx in fold.train_indices]
        test_times = [observations[idx].timestamp for idx in fold.test_indices]
        fold_payload: dict[str, Any] = {
            "fold_id": fold.fold_id,
            "counts": {
                "train": len(fold.train_indices),
                "test": len(fold.test_indices),
            },
            "ranges": {
                "train_index_ranges": _contiguous_ranges(fold.train_indices),
                "test_index_ranges": _contiguous_ranges(fold.test_indices),
                "train_time_utc": {
                    "start": min(train_times).isoformat().replace("+00:00", "Z"),
                    "end": max(train_times).isoformat().replace("+00:00", "Z"),
                },
                "test_time_utc": {
                    "start": min(test_times).isoformat().replace("+00:00", "Z"),
                    "end": max(test_times).isoformat().replace("+00:00", "Z"),
                },
            },
            "anti_leakage": {
                "test_interval": [fold.test_start, fold.test_end],
                "excluded_train_interval": [fold.excluded_start, fold.excluded_end],
                "purge_size": args.purge_size,
                "embargo_size": args.embargo_size,
                "train_policy": args.train_policy,
            },
        }
        if args.include_indices:
            fold_payload["indices"] = {
                "train": fold.train_indices,
                "test": fold.test_indices,
            }
        fold_payload["metrics"] = _fold_metric_payload(
            observations=observations,
            fold=fold,
            metric_cols=args.metric_col,
            target_col=args.target_col,
            prediction_col=args.prediction_col,
        )
        folds.append(fold_payload)

    config_payload = {
        "n_splits": args.n_splits,
        "test_size": args.test_size,
        "min_train_size": args.min_train_size,
        "purge_size": args.purge_size,
        "embargo_size": args.embargo_size,
        "train_policy": args.train_policy,
        "train_window_size": args.train_window_size,
        "timestamp_col": args.timestamp_col,
        "id_col": args.id_col,
        "target_col": args.target_col,
        "prediction_col": args.prediction_col,
        "metric_cols": args.metric_col,
    }

    fold_fingerprint = hashlib.sha256(
        json.dumps(
            {
                "config": config_payload,
                "folds": [
                    {
                        "fold_id": fold["fold_id"],
                        "train_range": fold["ranges"]["train_index_ranges"],
                        "test_range": fold["ranges"]["test_index_ranges"],
                        "excluded": fold["anti_leakage"]["excluded_train_interval"],
                    }
                    for fold in folds
                ],
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()

    output_payload: dict[str, Any] = {
        "schema_version": 1,
        "generated_at_utc": now_utc_iso(),
        "input": {
            "path": str(input_path),
            "format": input_format,
            "row_count_raw": len(raw_rows),
            "row_count_valid": len(observations),
        },
        "config": config_payload,
        "dataset_window_utc": {
            "start": observations[0].timestamp.isoformat().replace("+00:00", "Z"),
            "end": observations[-1].timestamp.isoformat().replace("+00:00", "Z"),
        },
        "split_fingerprint_sha256": fold_fingerprint,
        "folds": folds,
        "aggregate": {
            "fold_count": len(folds),
            "train_count_mean": round(statistics.fmean([fold["counts"]["train"] for fold in folds]), 6),
            "test_count_mean": round(statistics.fmean([fold["counts"]["test"] for fold in folds]), 6),
            "metrics_by_fold": _aggregate_numeric_metrics(folds),
        },
    }

    output_path = Path(args.output).resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(output_payload, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {output_path}")
    print(f"folds={len(folds)} split_fingerprint_sha256={fold_fingerprint}")
    return 0


def add_cli_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--input", required=True, help="Input dataset path (csv/json/jsonl).")
    parser.add_argument(
        "--output",
        default="logs\\eval-cycle\\purged-walkforward.json",
        help="Output JSON path.",
    )
    parser.add_argument(
        "--input-format",
        choices=("auto", "csv", "json", "jsonl"),
        default="auto",
        help="Input format (default: auto by extension).",
    )
    parser.add_argument(
        "--timestamp-col",
        default="timestamp",
        help="Timestamp column in UTC-like ISO format (default: timestamp).",
    )
    parser.add_argument(
        "--id-col",
        default="",
        help="Optional deterministic tiebreak column for equal timestamps.",
    )
    parser.add_argument("--n-splits", type=int, default=5, help="Number of folds (default: 5).")
    parser.add_argument("--test-size", type=int, default=None, help="Rows per test fold (default: auto).")
    parser.add_argument("--min-train-size", type=int, default=20, help="Minimum train rows per fold.")
    parser.add_argument("--purge-size", type=int, default=0, help="Rows purged around each test interval.")
    parser.add_argument("--embargo-size", type=int, default=0, help="Rows embargoed after each test interval.")
    parser.add_argument(
        "--train-policy",
        choices=("expanding", "rolling", "both-sides"),
        default="expanding",
        help="Train index policy (default: expanding).",
    )
    parser.add_argument(
        "--train-window-size",
        type=int,
        default=None,
        help="Required when --train-policy=rolling; rolling train window size in rows.",
    )
    parser.add_argument(
        "--metric-col",
        action="append",
        default=[],
        help="Repeatable numeric feature for fold mean metrics (train/test/delta).",
    )
    parser.add_argument("--target-col", default="", help="Optional target column for fold quality metrics.")
    parser.add_argument("--prediction-col", default="", help="Optional prediction column for fold quality metrics.")
    parser.add_argument(
        "--include-indices",
        action="store_true",
        help="Include full train/test index arrays in output (larger files).",
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Purged walk-forward splitter with deterministic anti-leakage metadata."
    )
    add_cli_arguments(parser)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    return run_walkforward(args)


if __name__ == "__main__":
    raise SystemExit(main())
