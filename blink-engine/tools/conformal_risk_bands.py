#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import statistics
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class SnapshotPoint:
    run_id: str
    timestamp: dt.datetime
    nav: float | None
    slippage_bps: float | None


@dataclass(frozen=True)
class MetricSample:
    run_id: str
    timestamp: dt.datetime
    value: float


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compute offline conformal risk bands from eval snapshots."
    )
    parser.add_argument(
        "--snapshots",
        default="logs\\eval-cycle",
        help="Snapshot root containing snapshot-*.json files (recursive).",
    )
    parser.add_argument(
        "--out-file",
        default="logs\\eval-cycle\\conformal-risk-bands.json",
        help="Output JSON artifact path.",
    )
    parser.add_argument(
        "--alphas",
        default="0.20,0.10,0.05",
        help="Comma-separated miscoverage levels alpha in (0,1). Example: 0.2,0.1,0.05",
    )
    parser.add_argument(
        "--train-frac",
        type=float,
        default=0.6,
        help="Chronological train split fraction (default: 0.6).",
    )
    parser.add_argument(
        "--calib-frac",
        type=float,
        default=0.2,
        help="Chronological calibration split fraction (default: 0.2).",
    )
    parser.add_argument(
        "--min-samples",
        type=int,
        default=12,
        help="Minimum samples required per metric (default: 12).",
    )
    return parser.parse_args()


def _parse_iso_timestamp(raw: str) -> dt.datetime | None:
    try:
        normalized = raw.replace("Z", "+00:00")
        parsed = dt.datetime.fromisoformat(normalized)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=dt.timezone.utc)
        return parsed
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


def _load_snapshot_points(snapshots_root: Path) -> tuple[list[SnapshotPoint], int]:
    points: list[SnapshotPoint] = []
    scanned = 0
    for path in sorted(snapshots_root.rglob("snapshot-*.json")):
        scanned += 1
        payload = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(payload, dict):
            continue

        ts_raw = payload.get("captured_at_utc")
        run_id_raw = payload.get("run_id")
        if not isinstance(ts_raw, str):
            continue
        timestamp = _parse_iso_timestamp(ts_raw)
        if timestamp is None:
            continue
        run_id = str(run_id_raw).strip() if isinstance(run_id_raw, str) else ""
        if not run_id:
            run_id = path.parent.name

        data = payload.get("data", {})
        if not isinstance(data, dict):
            continue
        portfolio = data.get("/api/portfolio", {})
        if not isinstance(portfolio, dict):
            portfolio = {}

        nav = _coerce_float(portfolio.get("total_nav"))
        slippage = _coerce_float(portfolio.get("avg_slippage_bps"))
        points.append(
            SnapshotPoint(
                run_id=run_id,
                timestamp=timestamp,
                nav=nav,
                slippage_bps=slippage,
            )
        )

    points.sort(key=lambda p: (p.run_id, p.timestamp))
    return points, scanned


def _collect_slippage_samples(points: list[SnapshotPoint]) -> list[MetricSample]:
    samples: list[MetricSample] = []
    for point in points:
        if point.slippage_bps is None:
            continue
        samples.append(MetricSample(run_id=point.run_id, timestamp=point.timestamp, value=point.slippage_bps))
    return samples


def _collect_return_samples(points: list[SnapshotPoint]) -> list[MetricSample]:
    grouped: dict[str, list[SnapshotPoint]] = {}
    for point in points:
        grouped.setdefault(point.run_id, []).append(point)

    samples: list[MetricSample] = []
    for run_id in sorted(grouped.keys()):
        run_points = sorted(grouped[run_id], key=lambda p: p.timestamp)
        for idx in range(1, len(run_points)):
            prev = run_points[idx - 1]
            cur = run_points[idx]
            if prev.nav is None or cur.nav is None or prev.nav <= 0:
                continue
            ret = (cur.nav - prev.nav) / prev.nav
            samples.append(MetricSample(run_id=run_id, timestamp=cur.timestamp, value=ret))
    return samples


def _parse_alphas(raw: str) -> list[float]:
    alphas: list[float] = []
    for token in raw.split(","):
        stripped = token.strip()
        if not stripped:
            continue
        value = float(stripped)
        if not (0.0 < value < 1.0):
            raise ValueError(f"alpha must be in (0,1), got {value}")
        alphas.append(value)
    unique_sorted = sorted(set(alphas), reverse=True)
    if not unique_sorted:
        raise ValueError("No valid alpha values provided.")
    return unique_sorted


def _split_sizes(n: int, train_frac: float, calib_frac: float) -> tuple[int, int, int]:
    if n < 3:
        raise ValueError("Need at least 3 samples for train/calib/eval split.")
    train_n = max(1, int(round(n * train_frac)))
    calib_n = max(1, int(round(n * calib_frac)))
    if train_n + calib_n >= n:
        overflow = train_n + calib_n - (n - 1)
        reduce_train = min(overflow, max(0, train_n - 1))
        train_n -= reduce_train
        overflow -= reduce_train
        if overflow > 0:
            reduce_calib = min(overflow, max(0, calib_n - 1))
            calib_n -= reduce_calib
            overflow -= reduce_calib
        if overflow > 0:
            raise ValueError("Unable to allocate non-empty eval split.")
    eval_n = n - train_n - calib_n
    if train_n < 1 or calib_n < 1 or eval_n < 1:
        raise ValueError("Invalid split sizes.")
    return train_n, calib_n, eval_n


def _finite_sample_residual_quantile(residuals: list[float], alpha: float) -> float:
    sorted_residuals = sorted(abs(x) for x in residuals)
    n = len(sorted_residuals)
    rank = math.ceil((n + 1) * (1.0 - alpha))
    index = min(max(rank - 1, 0), n - 1)
    return sorted_residuals[index]


def _coverage(values: list[float], lower: float, upper: float) -> float:
    if not values:
        return 0.0
    hits = sum(1 for value in values if lower <= value <= upper)
    return hits / len(values)


def _build_metric_bands(
    *,
    metric_name: str,
    unit: str,
    samples: list[MetricSample],
    alphas: list[float],
    train_frac: float,
    calib_frac: float,
) -> dict[str, Any]:
    ordered = sorted(samples, key=lambda s: s.timestamp)
    values = [sample.value for sample in ordered]
    train_n, calib_n, eval_n = _split_sizes(len(values), train_frac, calib_frac)
    train_values = values[:train_n]
    calib_values = values[train_n : train_n + calib_n]
    eval_values = values[train_n + calib_n :]

    point_estimate = statistics.median(train_values)
    calib_residuals = [abs(value - point_estimate) for value in calib_values]
    eval_abs_error = statistics.fmean(abs(value - point_estimate) for value in eval_values)

    bands: list[dict[str, Any]] = []
    for alpha in alphas:
        qhat = _finite_sample_residual_quantile(calib_residuals, alpha)
        lower = point_estimate - qhat
        upper = point_estimate + qhat
        bands.append(
            {
                "alpha": round(alpha, 6),
                "confidence": round(1.0 - alpha, 6),
                "lower": lower,
                "upper": upper,
                "width": upper - lower,
                "qhat_abs_residual": qhat,
                "coverage_calibration": round(_coverage(calib_values, lower, upper), 6),
                "coverage_eval": round(_coverage(eval_values, lower, upper), 6),
            }
        )

    return {
        "metric": metric_name,
        "unit": unit,
        "sample_count": len(values),
        "run_count": len({sample.run_id for sample in ordered}),
        "splits": {"train": train_n, "calibration": calib_n, "eval": eval_n},
        "point_estimate_train_median": point_estimate,
        "calibration_abs_residual_mean": statistics.fmean(calib_residuals),
        "eval_abs_error_mean": eval_abs_error,
        "sample_window": {
            "start_utc": ordered[0].timestamp.isoformat(),
            "end_utc": ordered[-1].timestamp.isoformat(),
        },
        "bands": bands,
    }


def build_artifact(args: argparse.Namespace) -> dict[str, Any]:
    snapshots_root = Path(args.snapshots)
    out_file = Path(args.out_file)
    alphas = _parse_alphas(args.alphas)
    train_frac = float(args.train_frac)
    calib_frac = float(args.calib_frac)
    if not (0.0 < train_frac < 1.0 and 0.0 < calib_frac < 1.0):
        raise ValueError("train-frac and calib-frac must be in (0,1).")
    if train_frac + calib_frac >= 1.0:
        raise ValueError("train-frac + calib-frac must be < 1.0")

    points, scanned = _load_snapshot_points(snapshots_root)
    slippage_samples = _collect_slippage_samples(points)
    return_samples = _collect_return_samples(points)

    metrics: dict[str, Any] = {}
    warnings: list[str] = []
    if len(slippage_samples) >= args.min_samples:
        metrics["slippage_bps"] = _build_metric_bands(
            metric_name="slippage_bps",
            unit="bps",
            samples=slippage_samples,
            alphas=alphas,
            train_frac=train_frac,
            calib_frac=calib_frac,
        )
    else:
        warnings.append(
            f"Skipped slippage_bps: found {len(slippage_samples)} samples (< min-samples={args.min_samples})."
        )

    if len(return_samples) >= args.min_samples:
        metrics["return"] = _build_metric_bands(
            metric_name="return",
            unit="fraction",
            samples=return_samples,
            alphas=alphas,
            train_frac=train_frac,
            calib_frac=calib_frac,
        )
    else:
        warnings.append(
            f"Skipped return: found {len(return_samples)} samples (< min-samples={args.min_samples})."
        )

    artifact = {
        "generated_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "source": {
            "snapshots_root": str(snapshots_root.resolve()),
            "snapshot_files_scanned": scanned,
            "snapshot_points_usable": len(points),
            "run_ids": sorted({point.run_id for point in points}),
        },
        "config": {
            "alphas": alphas,
            "train_frac": train_frac,
            "calib_frac": calib_frac,
            "min_samples": int(args.min_samples),
            "method": "split_conformal_absolute_residual",
            "point_predictor": "train_median",
        },
        "metrics": metrics,
        "warnings": warnings,
    }

    out_file.parent.mkdir(parents=True, exist_ok=True)
    out_file.write_text(json.dumps(artifact, indent=2), encoding="utf-8")
    print(f"Wrote {out_file}")
    if warnings:
        print("Warnings:")
        for warning in warnings:
            print(f"- {warning}")
    if not metrics:
        raise RuntimeError("No metrics produced. Provide more snapshots or lower --min-samples.")
    return artifact


def main() -> int:
    args = parse_args()
    build_artifact(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
