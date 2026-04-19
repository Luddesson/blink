#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import math
import statistics
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class NavPoint:
    timestamp: dt.datetime
    nav: float


REGIME_ROUTE_MAP: dict[str, str] = {
    "warmup": "observe_only",
    "low_volatility": "maker_spread_capture",
    "high_volatility": "defensive_risk_off",
    "trend_up": "momentum_yes_bias",
    "trend_down": "mean_reversion_or_reduce_risk",
    "drawdown_stress": "capital_preservation",
    "neutral": "baseline_shadow_copy",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Offline regime detection for Blink eval snapshots."
    )
    parser.add_argument(
        "--snapshots",
        default="logs\\eval-cycle",
        help="Directory containing snapshot-*.json files (recursive).",
    )
    parser.add_argument(
        "--out-dir",
        default="logs\\eval-cycle\\regimes",
        help="Output directory for regime artifacts.",
    )
    parser.add_argument(
        "--window",
        type=int,
        default=12,
        help="Rolling window size in samples for volatility/trend metrics.",
    )
    parser.add_argument(
        "--high-vol-z",
        type=float,
        default=0.8,
        help="Robust z-score threshold above rolling vol baseline for high volatility.",
    )
    parser.add_argument(
        "--low-vol-z",
        type=float,
        default=-0.8,
        help="Robust z-score threshold below rolling vol baseline for low volatility.",
    )
    parser.add_argument(
        "--trend-t-threshold",
        type=float,
        default=0.75,
        help="Absolute rolling t-score threshold to classify trend_up / trend_down.",
    )
    parser.add_argument(
        "--change-point-score",
        type=float,
        default=2.25,
        help="Score threshold for change-point boundary hints between adjacent windows.",
    )
    parser.add_argument(
        "--drawdown-stress-pct",
        type=float,
        default=4.0,
        help="Drawdown percentage threshold for drawdown_stress regime.",
    )
    parser.add_argument(
        "--min-segment-samples",
        type=int,
        default=3,
        help="Minimum segment sample count before change-point can split a segment.",
    )
    return parser.parse_args()


def _parse_ts(raw: str) -> dt.datetime | None:
    try:
        normalized = raw.replace("Z", "+00:00")
        parsed = dt.datetime.fromisoformat(normalized)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=dt.timezone.utc)
        return parsed
    except ValueError:
        return None


def _extract_nav(path: Path) -> NavPoint | None:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        return None
    ts_raw = payload.get("captured_at_utc")
    ts = _parse_ts(ts_raw) if isinstance(ts_raw, str) else None
    if ts is None:
        name = path.name.removeprefix("snapshot-").removesuffix(".json")
        tail = name.rsplit("-", 1)[-1]
        try:
            ts = dt.datetime.strptime(tail, "%Y%m%dT%H%M%SZ").replace(
                tzinfo=dt.timezone.utc
            )
        except ValueError:
            return None
    if ts is None:
        return None
    portfolio = payload.get("data", {}).get("/api/portfolio", {})
    if not isinstance(portfolio, dict):
        return None
    nav = portfolio.get("total_nav")
    if not isinstance(nav, (int, float)) or nav <= 0:
        return None
    return NavPoint(timestamp=ts, nav=float(nav))


def load_nav_points(snapshots_dir: Path) -> list[NavPoint]:
    points: list[NavPoint] = []
    for path in sorted(snapshots_dir.rglob("snapshot-*.json")):
        p = _extract_nav(path)
        if p is not None:
            points.append(p)
    return points


def _robust_zscore(value: float, baseline: list[float]) -> float:
    if not baseline:
        return 0.0
    median = statistics.median(baseline)
    abs_dev = [abs(x - median) for x in baseline]
    mad = statistics.median(abs_dev)
    if mad > 1e-12:
        return (value - median) / (1.4826 * mad)
    std = statistics.pstdev(baseline) if len(baseline) > 1 else 0.0
    return (value - median) / std if std > 1e-12 else 0.0


def _change_point_score(
    prev_mean: float | None,
    prev_vol: float | None,
    curr_mean: float,
    curr_vol: float,
) -> float:
    if prev_mean is None or prev_vol is None:
        return 0.0
    eps = 1e-9
    mean_shift = abs(curr_mean - prev_mean) / (prev_vol + eps)
    vol_shift = abs(curr_vol - prev_vol) / (prev_vol + eps)
    return mean_shift + vol_shift


def _classify_regime(
    *,
    robust_vol_z: float,
    trend_tscore: float,
    drawdown_pct: float,
    high_vol_z: float,
    low_vol_z: float,
    trend_t_threshold: float,
    drawdown_stress_pct: float,
) -> str:
    if drawdown_pct <= -drawdown_stress_pct and robust_vol_z >= 0.5:
        return "drawdown_stress"
    if robust_vol_z >= high_vol_z:
        return "high_volatility"
    if robust_vol_z <= low_vol_z:
        return "low_volatility"
    if trend_tscore >= trend_t_threshold:
        return "trend_up"
    if trend_tscore <= -trend_t_threshold:
        return "trend_down"
    return "neutral"


def classify_regimes(
    points: list[NavPoint],
    window: int,
    high_vol_z: float,
    low_vol_z: float,
    trend_t_threshold: float,
    change_point_score_threshold: float,
    drawdown_stress_pct: float,
    min_segment_samples: int,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    if len(points) < 3:
        return [], []

    returns: list[float] = []
    rolling_vol_history: list[float] = []
    records: list[dict[str, Any]] = []
    nav_peak = points[0].nav
    prev_window_mean: float | None = None
    prev_window_vol: float | None = None

    for idx in range(1, len(points)):
        prev = points[idx - 1]
        cur = points[idx]
        if prev.nav <= 0:
            continue
        ret = (cur.nav - prev.nav) / prev.nav
        returns.append(ret)
        nav_peak = max(nav_peak, cur.nav)
        drawdown_pct = ((cur.nav - nav_peak) / nav_peak * 100.0) if nav_peak > 0 else 0.0
        sample_count = min(window, len(returns))
        window_returns = returns[-sample_count:]
        warmup = len(returns) < window

        if sample_count >= 2:
            vol = statistics.pstdev(window_returns)
            avg = statistics.fmean(window_returns)
            trend_tscore = (
                (avg / (vol + 1e-9)) * math.sqrt(sample_count) if sample_count > 1 else 0.0
            )
            baseline = rolling_vol_history[-max(window * 6, window) :]
            robust_vol_z = _robust_zscore(vol, baseline)
            cp_score = _change_point_score(prev_window_mean, prev_window_vol, avg, vol)
            change_point = (
                cp_score >= change_point_score_threshold
                and not warmup
                and len(rolling_vol_history) >= max(window, 3)
            )
            regime = (
                "warmup"
                if warmup
                else _classify_regime(
                    robust_vol_z=robust_vol_z,
                    trend_tscore=trend_tscore,
                    drawdown_pct=drawdown_pct,
                    high_vol_z=high_vol_z,
                    low_vol_z=low_vol_z,
                    trend_t_threshold=trend_t_threshold,
                    drawdown_stress_pct=drawdown_stress_pct,
                )
            )
        else:
            regime = "warmup"
            vol = 0.0
            avg = 0.0
            robust_vol_z = 0.0
            trend_tscore = 0.0
            cp_score = 0.0
            change_point = False

        rolling_vol_history.append(vol)
        prev_window_mean = avg
        prev_window_vol = vol

        records.append(
            {
                "timestamp_utc": cur.timestamp.isoformat(),
                "nav": round(cur.nav, 6),
                "return": round(ret, 8),
                "rolling_return_mean": round(avg, 8),
                "rolling_vol": round(vol, 8),
                "vol_robust_zscore": round(robust_vol_z, 6),
                "trend_tscore": round(trend_tscore, 6),
                "change_point_score": round(cp_score, 6),
                "change_point": bool(change_point),
                "drawdown_pct": round(drawdown_pct, 4),
                "regime": regime,
                "strategy_route": REGIME_ROUTE_MAP[regime],
            }
        )

    segments: list[dict[str, Any]] = []
    if not records:
        return [], []

    start_idx = 0
    for i in range(1, len(records) + 1):
        regime_changed = i < len(records) and records[i]["regime"] != records[start_idx]["regime"]
        cp_boundary = (
            i < len(records)
            and bool(records[i]["change_point"])
            and (i - start_idx) >= max(min_segment_samples, 1)
        )
        if i == len(records) or regime_changed or cp_boundary:
            block = records[start_idx:i]
            segments.append(
                {
                    "regime": records[start_idx]["regime"],
                    "strategy_route": records[start_idx]["strategy_route"],
                    "start_utc": block[0]["timestamp_utc"],
                    "end_utc": block[-1]["timestamp_utc"],
                    "samples": len(block),
                    "avg_return": round(statistics.fmean(x["return"] for x in block), 8),
                    "avg_drawdown_pct": round(
                        statistics.fmean(x["drawdown_pct"] for x in block), 6
                    ),
                    "avg_vol_robust_zscore": round(
                        statistics.fmean(x["vol_robust_zscore"] for x in block), 6
                    ),
                    "avg_trend_tscore": round(statistics.fmean(x["trend_tscore"] for x in block), 6),
                }
            )
            start_idx = i

    return records, segments


def write_outputs(out_dir: Path, records: list[dict[str, Any]], segments: list[dict[str, Any]]) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    csv_path = out_dir / "regime-points.csv"
    points_json_path = out_dir / "regime-points.json"
    segments_json_path = out_dir / "regime-segments.json"
    json_path = out_dir / "regime-summary.json"

    with csv_path.open("w", encoding="utf-8", newline="") as handle:
        if records:
            writer = csv.DictWriter(handle, fieldnames=list(records[0].keys()))
            writer.writeheader()
            for row in records:
                writer.writerow(row)

    points_json_path.write_text(json.dumps(records, indent=2), encoding="utf-8")
    segments_json_path.write_text(json.dumps({"segments": segments}, indent=2), encoding="utf-8")

    transition_counts: dict[str, int] = {}
    for idx in range(1, len(segments)):
        pair = f"{segments[idx - 1]['regime']}->{segments[idx]['regime']}"
        transition_counts[pair] = transition_counts.get(pair, 0) + 1

    summary = {
        "generated_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "points": len(records),
        "regimes_present": sorted({str(row["regime"]) for row in records}),
        "strategy_routes_present": sorted({str(row["strategy_route"]) for row in records}),
        "transition_counts": transition_counts,
        "segments": segments,
    }
    json_path.write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(f"Wrote {csv_path}")
    print(f"Wrote {points_json_path}")
    print(f"Wrote {segments_json_path}")
    print(f"Wrote {json_path}")


def main() -> int:
    args = parse_args()
    points = load_nav_points(Path(args.snapshots))
    if len(points) < 3:
        raise RuntimeError("Not enough NAV points from snapshots to detect regimes.")
    records, segments = classify_regimes(
        points=points,
        window=max(args.window, 3),
        high_vol_z=args.high_vol_z,
        low_vol_z=args.low_vol_z,
        trend_t_threshold=args.trend_t_threshold,
        change_point_score_threshold=args.change_point_score,
        drawdown_stress_pct=max(args.drawdown_stress_pct, 0.0),
        min_segment_samples=max(args.min_segment_samples, 1),
    )
    write_outputs(Path(args.out_dir), records, segments)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
