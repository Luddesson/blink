#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate returns CSV from Blink eval snapshots, optionally render QuantStats HTML."
    )
    parser.add_argument(
        "--snapshots",
        default="logs\\eval-cycle",
        help="Folder containing snapshot-*.json files from tools/run_eval_cycle.py",
    )
    parser.add_argument(
        "--out",
        default="logs\\eval-cycle\\quantstats",
        help="Output folder for returns.csv and optional quantstats-report.html",
    )
    return parser.parse_args()


def _parse_snapshot_timestamp(
    path: Path,
    snapshot_payload: dict[str, Any],
) -> dt.datetime | None:
    captured = snapshot_payload.get("captured_at_utc")
    if isinstance(captured, str) and captured.strip():
        try:
            normalized = captured.replace("Z", "+00:00")
            parsed = dt.datetime.fromisoformat(normalized)
            return parsed if parsed.tzinfo else parsed.replace(tzinfo=dt.timezone.utc)
        except ValueError:
            pass

    name = path.name.removeprefix("snapshot-").removesuffix(".json")
    tail = name.rsplit("-", 1)[-1]
    try:
        return dt.datetime.strptime(tail, "%Y%m%dT%H%M%SZ").replace(tzinfo=dt.timezone.utc)
    except ValueError:
        return None


def _extract_nav(snapshot_payload: dict[str, Any]) -> float | None:
    portfolio = snapshot_payload.get("data", {}).get("/api/portfolio", {})
    nav = portfolio.get("total_nav")
    if isinstance(nav, (int, float)):
        return float(nav)
    return None


def _load_nav_points(snapshots_dir: Path) -> list[tuple[dt.datetime, float]]:
    rows: list[tuple[dt.datetime, float]] = []
    for path in sorted(snapshots_dir.rglob("snapshot-*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(payload, dict):
            continue
        ts = _parse_snapshot_timestamp(path, payload)
        if ts is None:
            continue
        nav = _extract_nav(payload)
        if nav is None:
            continue
        rows.append((ts, nav))
    return rows


def _write_returns_csv(rows: list[tuple[dt.datetime, float]], out_file: Path) -> int:
    if len(rows) < 2:
        return 0

    out_file.parent.mkdir(parents=True, exist_ok=True)
    count = 0
    with out_file.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["timestamp_utc", "return"])
        prev_nav = rows[0][1]
        for ts, nav in rows[1:]:
            if prev_nav <= 0:
                prev_nav = nav
                continue
            ret = (nav - prev_nav) / prev_nav
            writer.writerow([ts.isoformat(), f"{ret:.10f}"])
            prev_nav = nav
            count += 1
    return count


def _maybe_render_quantstats(returns_csv: Path, out_html: Path) -> bool:
    try:
        import pandas as pd  # type: ignore
        import quantstats as qs  # type: ignore
    except ImportError:
        return False

    frame = pd.read_csv(returns_csv)
    if frame.empty:
        return False
    series = pd.Series(frame["return"].astype(float).values, index=pd.to_datetime(frame["timestamp_utc"]))
    qs.reports.html(series, output=str(out_html), title="Blink Eval QuantStats")
    return True


def main() -> int:
    args = parse_args()
    snapshots_dir = Path(args.snapshots)
    out_dir = Path(args.out)
    nav_rows = _load_nav_points(snapshots_dir)
    returns_csv = out_dir / "returns.csv"
    returns_written = _write_returns_csv(nav_rows, returns_csv)
    if returns_written == 0:
        print("No usable NAV deltas found. Confirm snapshot files include /api/portfolio.total_nav.")
        return 0

    print(f"Wrote {returns_csv} with {returns_written} return rows.")
    html_path = out_dir / "quantstats-report.html"
    if _maybe_render_quantstats(returns_csv, html_path):
        print(f"Wrote {html_path}")
    else:
        print("QuantStats not installed. Install with: pip install quantstats pandas")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
