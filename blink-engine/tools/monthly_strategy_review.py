#!/usr/bin/env python3
from __future__ import annotations

import argparse
import calendar
import datetime as dt
import json
import pathlib
import statistics
from typing import Any


ARTIFACT_FILES: tuple[str, ...] = (
    "report.json",
    "decision.json",
    "drift-matrix.json",
    "recommendation-confidence-v2.json",
)


def _parse_iso_utc(value: Any) -> dt.datetime | None:
    if not isinstance(value, str) or not value.strip():
        return None
    try:
        normalized = value.replace("Z", "+00:00")
        parsed = dt.datetime.fromisoformat(normalized)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=dt.timezone.utc)
        return parsed.astimezone(dt.timezone.utc)
    except ValueError:
        return None


def _to_iso_utc(value: dt.datetime) -> str:
    return value.astimezone(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def _month_bounds(month: str) -> tuple[dt.datetime, dt.datetime]:
    parsed = dt.datetime.strptime(month, "%Y-%m").replace(tzinfo=dt.timezone.utc)
    if parsed.month == 12:
        next_month = parsed.replace(year=parsed.year + 1, month=1, day=1)
    else:
        next_month = parsed.replace(month=parsed.month + 1, day=1)
    start = parsed.replace(day=1, hour=0, minute=0, second=0, microsecond=0)
    return start, next_month


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


def _read_json_object(path: pathlib.Path) -> dict[str, Any] | None:
    if not path.exists():
        return None
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        return None
    return payload


def _pick_nested_scalar(payload: dict[str, Any], keys: tuple[str, ...]) -> float | None:
    for key in keys:
        value = payload.get(key)
        number = _coerce_float(value)
        if number is not None:
            return number
    return None


def _stats(values: list[float]) -> dict[str, Any]:
    if not values:
        return {"count": 0, "mean": None, "median": None, "min": None, "max": None}
    return {
        "count": len(values),
        "mean": round(statistics.fmean(values), 6),
        "median": round(statistics.median(values), 6),
        "min": round(min(values), 6),
        "max": round(max(values), 6),
    }


def _resolve_run_timestamp(report: dict[str, Any] | None, fingerprint: dict[str, Any] | None) -> dt.datetime | None:
    if report is not None:
        window = report.get("window")
        if isinstance(window, dict):
            for key in ("window_end_utc", "window_start_utc"):
                parsed = _parse_iso_utc(window.get(key))
                if parsed is not None:
                    return parsed
        for key in ("generated_at_utc",):
            parsed = _parse_iso_utc(report.get(key))
            if parsed is not None:
                return parsed
    if fingerprint is not None:
        return _parse_iso_utc(fingerprint.get("captured_at_utc"))
    return None


def _build_markdown(summary: dict[str, Any]) -> str:
    month = str(summary["month"])
    coverage = summary["coverage"]
    performance = summary["performance"]
    risk = summary["risk"]
    drift = summary["drift"]
    confidence = summary["confidence"]
    recommendation = summary["recommendation"]
    missing = summary["missing_artifacts"]
    lines = [
        f"# Monthly Strategy Review Packet — {month}",
        "",
        "## Coverage",
        f"- Runs considered: **{summary['runs_considered']}**",
        f"- Days with runs: **{coverage['days_with_runs']} / {coverage['days_in_month']}**",
        f"- Missing days: **{coverage['missing_days_count']}**",
    ]
    if coverage["missing_days"]:
        lines.append(f"- Missing dates: `{', '.join(coverage['missing_days'])}`")

    lines.extend(
        [
            "",
            "## Performance",
            f"- PnL return % stats: count={performance['pnl_return_pct']['count']}, mean={performance['pnl_return_pct']['mean']}, median={performance['pnl_return_pct']['median']}, min={performance['pnl_return_pct']['min']}, max={performance['pnl_return_pct']['max']}",
            f"- PSR stats: count={performance['psr_probability']['count']}, mean={performance['psr_probability']['mean']}",
            f"- DSR stats: count={performance['dsr_probability']['count']}, mean={performance['dsr_probability']['mean']}",
            "",
            "## Risk",
            f"- Risk events score stats: count={risk['risk_events_score']['count']}, mean={risk['risk_events_score']['mean']}",
            f"- WS connected ratio stats: count={risk['ws_connected_ratio']['count']}, mean={risk['ws_connected_ratio']['mean']}",
            "",
            "## Drift",
            f"- Drift severity stats: count={drift['severity_score']['count']}, mean={drift['severity_score']['mean']}",
            f"- Fill-rate delta (pp) stats: count={drift['fill_rate_pct_points']['count']}, mean={drift['fill_rate_pct_points']['mean']}",
            f"- Slippage delta (bps) stats: count={drift['avg_slippage_bps']['count']}, mean={drift['avg_slippage_bps']['mean']}",
            "",
            "## Confidence",
            f"- Confidence score % stats: count={confidence['score_percent']['count']}, mean={confidence['score_percent']['mean']}",
            f"- Confidence levels: {json.dumps(confidence['levels'], sort_keys=True)}",
            "",
            "## Recommendation Changes",
            f"- Final recommendation counts: {json.dumps(recommendation['counts'], sort_keys=True)}",
            f"- Recommendation transitions: **{recommendation['changes_count']}**",
        ]
    )

    transitions = recommendation.get("changes", [])
    if isinstance(transitions, list) and transitions:
        lines.append("")
        lines.append("| at_utc | run_id | from | to |")
        lines.append("|---|---|---|---|")
        for row in transitions:
            lines.append(
                f"| {row.get('at_utc')} | {row.get('run_id')} | {row.get('from')} | {row.get('to')} |"
            )

    lines.extend(
        [
            "",
            "## Missing Artifacts",
            f"- Totals: {json.dumps(missing, sort_keys=True)}",
            "",
            "## Notes",
            "- This packet is deterministic for the same input artifacts.",
            "- Missing daily runs are treated as coverage gaps, not hard errors.",
        ]
    )
    return "\n".join(lines) + "\n"


def run_monthly_strategy_review(args: argparse.Namespace) -> int:
    month = str(args.month).strip() or _default_month()
    start, end = _month_bounds(month)
    eval_root = pathlib.Path(args.eval_root).resolve()
    out_dir = pathlib.Path(args.out_dir).resolve() / month
    out_dir.mkdir(parents=True, exist_ok=True)

    run_rows: list[dict[str, Any]] = []
    run_dirs = sorted(path for path in eval_root.iterdir() if path.is_dir()) if eval_root.exists() else []
    for run_dir in run_dirs:
        report = _read_json_object(run_dir / "report.json")
        decision = _read_json_object(run_dir / "decision.json")
        drift = _read_json_object(run_dir / "drift-matrix.json")
        confidence = _read_json_object(run_dir / "recommendation-confidence-v2.json")
        fingerprint = _read_json_object(run_dir / "fingerprint.json")

        run_ts = _resolve_run_timestamp(report, fingerprint)
        if run_ts is None or run_ts < start or run_ts >= end:
            continue

        decision_dimensions = decision.get("dimensions", {}) if isinstance(decision, dict) else {}
        if not isinstance(decision_dimensions, dict):
            decision_dimensions = {}
        risk_dimension = decision_dimensions.get("risk_events", {})
        drift_dimension = decision_dimensions.get("drift_severity", {})
        if not isinstance(risk_dimension, dict):
            risk_dimension = {}
        if not isinstance(drift_dimension, dict):
            drift_dimension = {}

        drift_delta = drift.get("delta", {}) if isinstance(drift, dict) else {}
        if not isinstance(drift_delta, dict):
            drift_delta = {}

        report_risk_adjusted = report.get("risk_adjusted", {}) if isinstance(report, dict) else {}
        if not isinstance(report_risk_adjusted, dict):
            report_risk_adjusted = {}
        report_health = report.get("health", {}) if isinstance(report, dict) else {}
        if not isinstance(report_health, dict):
            report_health = {}

        run_rows.append(
            {
                "run_id": str((report or {}).get("run_id") or run_dir.name),
                "run_dir": str(run_dir),
                "timestamp_utc": _to_iso_utc(run_ts),
                "date_utc": run_ts.date().isoformat(),
                "artifacts_present": {
                    "report": report is not None,
                    "decision": decision is not None,
                    "drift_matrix": drift is not None,
                    "recommendation_confidence_v2": confidence is not None,
                },
                "performance": {
                    "pnl_return_pct": _pick_nested_scalar(
                        report or {},
                        ("pnl_return_pct", "return_pct", "net_return_pct", "roi_pct"),
                    ),
                    "psr_probability_sharpe_gt_0": _coerce_float(report_risk_adjusted.get("psr_probability_sharpe_gt_0")),
                    "dsr_probability": _coerce_float(report_risk_adjusted.get("dsr_probability")),
                },
                "risk": {
                    "risk_events_score": _coerce_float(risk_dimension.get("value")),
                    "ws_connected_ratio": _coerce_float(report_health.get("ws_connected_ratio")),
                },
                "drift": {
                    "severity_score": _coerce_float(drift_dimension.get("value")),
                    "fill_rate_pct_points": _coerce_float(drift_delta.get("fill_rate_pct_points")),
                    "avg_slippage_bps": _coerce_float(drift_delta.get("avg_slippage_bps")),
                },
                "confidence": {
                    "score_percent": _coerce_float((confidence or {}).get("confidence_score_percent")),
                    "level": (confidence or {}).get("confidence_level"),
                },
                "recommendation": {
                    "decision": (decision or {}).get("decision"),
                },
            }
        )

    run_rows.sort(key=lambda row: (str(row.get("timestamp_utc")), str(row.get("run_id"))))

    days_in_month = calendar.monthrange(start.year, start.month)[1]
    expected_days = [dt.date(start.year, start.month, day).isoformat() for day in range(1, days_in_month + 1)]
    observed_days = sorted({str(row["date_utc"]) for row in run_rows})
    missing_days = [day for day in expected_days if day not in observed_days]

    missing_artifacts = {key: 0 for key in ("report", "decision", "drift_matrix", "recommendation_confidence_v2")}
    for row in run_rows:
        present = row.get("artifacts_present", {})
        if not isinstance(present, dict):
            continue
        for artifact_name in missing_artifacts:
            if present.get(artifact_name) is False:
                missing_artifacts[artifact_name] += 1

    performance_pnl = [row["performance"]["pnl_return_pct"] for row in run_rows if row["performance"]["pnl_return_pct"] is not None]
    performance_psr = [
        row["performance"]["psr_probability_sharpe_gt_0"]
        for row in run_rows
        if row["performance"]["psr_probability_sharpe_gt_0"] is not None
    ]
    performance_dsr = [row["performance"]["dsr_probability"] for row in run_rows if row["performance"]["dsr_probability"] is not None]

    risk_events_score = [row["risk"]["risk_events_score"] for row in run_rows if row["risk"]["risk_events_score"] is not None]
    ws_connected_ratio = [row["risk"]["ws_connected_ratio"] for row in run_rows if row["risk"]["ws_connected_ratio"] is not None]

    drift_severity = [row["drift"]["severity_score"] for row in run_rows if row["drift"]["severity_score"] is not None]
    drift_fill_rate = [row["drift"]["fill_rate_pct_points"] for row in run_rows if row["drift"]["fill_rate_pct_points"] is not None]
    drift_slippage = [row["drift"]["avg_slippage_bps"] for row in run_rows if row["drift"]["avg_slippage_bps"] is not None]

    confidence_score = [row["confidence"]["score_percent"] for row in run_rows if row["confidence"]["score_percent"] is not None]
    confidence_levels: dict[str, int] = {}
    for row in run_rows:
        level = row["confidence"]["level"]
        if isinstance(level, str) and level.strip():
            confidence_levels[level.strip()] = confidence_levels.get(level.strip(), 0) + 1

    recommendation_counts: dict[str, int] = {}
    transitions: list[dict[str, Any]] = []
    previous_decision: str | None = None
    for row in run_rows:
        decision_value = row["recommendation"]["decision"]
        if not isinstance(decision_value, str) or not decision_value.strip():
            continue
        normalized = decision_value.strip().upper()
        recommendation_counts[normalized] = recommendation_counts.get(normalized, 0) + 1
        if previous_decision is not None and normalized != previous_decision:
            transitions.append(
                {
                    "at_utc": row["timestamp_utc"],
                    "run_id": row["run_id"],
                    "from": previous_decision,
                    "to": normalized,
                }
            )
        previous_decision = normalized

    default_compiled_at = _to_iso_utc(start)
    compiled_at = run_rows[-1]["timestamp_utc"] if run_rows else default_compiled_at

    summary = {
        "schema_version": 1,
        "compiled_at_utc": compiled_at,
        "month": month,
        "window": {
            "start_utc": _to_iso_utc(start),
            "end_utc_exclusive": _to_iso_utc(end),
        },
        "runs_considered": len(run_rows),
        "coverage": {
            "days_in_month": days_in_month,
            "days_with_runs": len(observed_days),
            "missing_days_count": len(missing_days),
            "missing_days": missing_days,
        },
        "missing_artifacts": missing_artifacts,
        "performance": {
            "pnl_return_pct": _stats(performance_pnl),
            "psr_probability": _stats(performance_psr),
            "dsr_probability": _stats(performance_dsr),
        },
        "risk": {
            "risk_events_score": _stats(risk_events_score),
            "ws_connected_ratio": _stats(ws_connected_ratio),
        },
        "drift": {
            "severity_score": _stats(drift_severity),
            "fill_rate_pct_points": _stats(drift_fill_rate),
            "avg_slippage_bps": _stats(drift_slippage),
        },
        "confidence": {
            "score_percent": _stats(confidence_score),
            "levels": {key: confidence_levels[key] for key in sorted(confidence_levels)},
        },
        "recommendation": {
            "counts": {key: recommendation_counts[key] for key in sorted(recommendation_counts)},
            "changes_count": len(transitions),
            "changes": transitions,
        },
        "runs": run_rows,
    }

    out_json_path = pathlib.Path(args.out_json).resolve() if str(args.out_json).strip() else (out_dir / "summary.json")
    out_md_path = pathlib.Path(args.out_md).resolve() if str(args.out_md).strip() else (out_dir / "review-packet.md")
    out_json_path.parent.mkdir(parents=True, exist_ok=True)
    out_md_path.parent.mkdir(parents=True, exist_ok=True)
    out_json_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    out_md_path.write_text(_build_markdown(summary), encoding="utf-8")

    print(f"Wrote {out_json_path}")
    print(f"Wrote {out_md_path}")
    return 0


def _default_month() -> str:
    now = dt.datetime.now(dt.timezone.utc)
    if now.month == 1:
        previous = now.replace(year=now.year - 1, month=12, day=1)
    else:
        previous = now.replace(month=now.month - 1, day=1)
    return previous.strftime("%Y-%m")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Compile monthly eval artifacts into deterministic machine + markdown review packet outputs."
    )
    parser.add_argument("--month", default=_default_month(), help="Month window in YYYY-MM format (default: previous month)")
    parser.add_argument(
        "--eval-root",
        default="logs\\eval-cycle",
        help="Root directory containing eval run subfolders",
    )
    parser.add_argument(
        "--out-dir",
        default="logs\\eval-cycle\\monthly-review",
        help="Base output directory for monthly packet artifacts",
    )
    parser.add_argument("--out-json", default="", help="Optional explicit summary JSON path")
    parser.add_argument("--out-md", default="", help="Optional explicit markdown packet path")
    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return run_monthly_strategy_review(args)


if __name__ == "__main__":
    raise SystemExit(main())
