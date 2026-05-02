#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
from pathlib import Path
from typing import Any


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


def _read_json_object(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"Expected JSON object in {path}")
    return payload


def _optional_json_object(path: Path) -> dict[str, Any] | None:
    if not path.exists():
        return None
    return _read_json_object(path)


def _pick_scalar(payload: dict[str, Any], keys: tuple[str, ...]) -> float | None:
    for key in keys:
        direct = _coerce_float(payload.get(key))
        if direct is not None:
            return direct
        nested = payload.get("metrics")
        if isinstance(nested, dict):
            nested_value = _coerce_float(nested.get(key))
            if nested_value is not None:
                return nested_value
    return None


def _extract_top_gate_counts(report: dict[str, Any]) -> dict[str, float]:
    counts: dict[str, float] = {}
    rows = report.get("gate_pressure_top5_run_window")
    if not isinstance(rows, list):
        return counts
    for row in rows:
        if not isinstance(row, dict):
            continue
        gate = row.get("gate")
        delta = _coerce_float(row.get("rejections_delta"))
        if isinstance(gate, str) and delta is not None and delta > 0:
            counts[gate.strip().lower()] = delta
    return counts


def _extract_miscoverage_pct(conformal: dict[str, Any] | None) -> float | None:
    if conformal is None:
        return None
    direct = _pick_scalar(
        conformal,
        (
            "miscoverage_pct",
            "empirical_miscoverage_pct",
            "miscoverage_rate_pct",
            "error_rate_pct",
            "alpha_realized_pct",
        ),
    )
    if direct is not None:
        return max(0.0, direct)
    coverage = _pick_scalar(conformal, ("coverage_pct", "empirical_coverage_pct"))
    if coverage is not None:
        return max(0.0, 100.0 - coverage)

    metrics = conformal.get("metrics")
    if not isinstance(metrics, dict):
        return None
    coverages: list[float] = []
    for metric in metrics.values():
        if not isinstance(metric, dict):
            continue
        bands = metric.get("bands")
        if not isinstance(bands, list):
            continue
        for band in bands:
            if not isinstance(band, dict):
                continue
            coverage_eval = _coerce_float(band.get("coverage_eval"))
            if coverage_eval is not None:
                coverages.append(coverage_eval * 100.0 if coverage_eval <= 1.0 else coverage_eval)
    if not coverages:
        return None
    return max(0.0, 100.0 - min(coverages))


def _clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


def _round2(value: float) -> float:
    return round(value, 2)


def _build_recommendations(
    *,
    run_id: str,
    report: dict[str, Any],
    drift: dict[str, Any],
    conformal: dict[str, Any] | None,
    decision: dict[str, Any] | None,
) -> dict[str, Any]:
    pnl_return_pct = _pick_scalar(report, ("pnl_return_pct", "return_pct", "net_return_pct", "roi_pct")) or 0.0
    fee_drag_pct = _pick_scalar(report, ("fee_drag_pct", "fee_drag", "fee_drag_percent")) or 0.0
    drift_delta = drift.get("delta", {}) if isinstance(drift.get("delta"), dict) else {}
    fill_rate_drop_pp = max(0.0, -(_coerce_float(drift_delta.get("fill_rate_pct_points")) or 0.0))
    slippage_excess_bps = max(0.0, _coerce_float(drift_delta.get("avg_slippage_bps")) or 0.0)
    reject_l1 = max(
        0.0,
        _coerce_float((drift_delta.get("reject_mix") or {}).get("l1_distance") if isinstance(drift_delta.get("reject_mix"), dict) else 0.0)
        or 0.0,
    )
    notional_l1 = max(
        0.0,
        _coerce_float(
            (drift_delta.get("notional_distribution") or {}).get("l1_distance")
            if isinstance(drift_delta.get("notional_distribution"), dict)
            else 0.0
        )
        or 0.0,
    )
    category_l1 = max(
        0.0,
        _coerce_float((drift_delta.get("category_mix") or {}).get("l1_distance") if isinstance(drift_delta.get("category_mix"), dict) else 0.0)
        or 0.0,
    )
    miscoverage_pct = _extract_miscoverage_pct(conformal)
    gate_counts = _extract_top_gate_counts(report)
    rate_limit_hits = sum(value for key, value in gate_counts.items() if "rate" in key)
    fee_to_edge_hits = sum(value for key, value in gate_counts.items() if "fee" in key)
    risk_events_value = None
    if decision and isinstance(decision.get("dimensions"), dict):
        risk_events_value = _coerce_float((decision.get("dimensions") or {}).get("risk_events", {}).get("value"))

    proposals: list[dict[str, Any]] = []

    sizing_pressure = _clamp(((max(fee_drag_pct - 20.0, 0.0) / 20.0) + (slippage_excess_bps / 80.0) + (fee_to_edge_hits / 10.0)) / 2.2, 0.0, 1.0)
    if sizing_pressure > 0.1:
        proposals.append(
            {
                "parameter": "PAPER_SIZE_MULTIPLIER",
                "current_assumption": 0.20,
                "recommended_value": _round2(0.20 * (1.0 - 0.25 * sizing_pressure)),
                "direction": "decrease",
                "expected_uplift_pct_points": _round2(0.2 + 1.1 * sizing_pressure),
                "risk_impact_score": _round2(25 + 45 * sizing_pressure),
                "heuristic": {
                    "formula": "sizing_pressure=((max(fee_drag-20,0)/20)+(slippage_excess/80)+(fee_to_edge_hits/10))/2.2",
                    "inputs": {
                        "fee_drag_pct": fee_drag_pct,
                        "slippage_excess_bps": slippage_excess_bps,
                        "fee_to_edge_hits": fee_to_edge_hits,
                        "sizing_pressure": _round2(sizing_pressure),
                    },
                    "rationale": "Reduce sizing when edge is being consumed by fees/slippage to improve net expectancy.",
                },
            }
        )

    if rate_limit_hits > 0:
        rate_pressure = _clamp(rate_limit_hits / 5.0, 0.0, 1.0)
        proposals.append(
            {
                "parameter": "MAX_ORDERS_PER_SECOND",
                "current_assumption": 3,
                "recommended_value": int(round(3 + min(2.0, rate_limit_hits))),
                "direction": "increase",
                "expected_uplift_pct_points": _round2(0.1 + 0.4 * rate_pressure),
                "risk_impact_score": _round2(-10 - 20 * rate_pressure),
                "heuristic": {
                    "formula": "rate_pressure=clamp(rate_limit_hits/5,0,1)",
                    "inputs": {
                        "rate_limit_hits": rate_limit_hits,
                        "rate_pressure": _round2(rate_pressure),
                    },
                    "rationale": "Recover dropped opportunities from rate-limit pressure, while acknowledging higher operational risk.",
                },
            }
        )

    drift_pressure = _clamp((fill_rate_drop_pp / 6.0) + (slippage_excess_bps / 120.0), 0.0, 1.0)
    if drift_pressure > 0.1:
        tighten = slippage_excess_bps > 20.0 or (miscoverage_pct is not None and miscoverage_pct > 8.0)
        proposals.append(
            {
                "parameter": "PRICE_DRIFT_ABORT_BPS",
                "current_assumption": 150,
                "recommended_value": 125 if tighten else 175,
                "direction": "decrease" if tighten else "increase",
                "expected_uplift_pct_points": _round2(0.12 + 0.55 * drift_pressure),
                "risk_impact_score": _round2((22 + 28 * drift_pressure) if tighten else (-8 - 18 * drift_pressure)),
                "heuristic": {
                    "formula": "drift_pressure=clamp(fill_rate_drop_pp/6 + slippage_excess_bps/120,0,1)",
                    "inputs": {
                        "fill_rate_drop_pp": fill_rate_drop_pp,
                        "slippage_excess_bps": slippage_excess_bps,
                        "miscoverage_pct": miscoverage_pct,
                        "drift_pressure": _round2(drift_pressure),
                        "mode": "tighten" if tighten else "loosen",
                    },
                    "rationale": "Tighten drift abort when execution quality is unstable; loosen only when lost fills dominate and slippage stays controlled.",
                },
            }
        )

    risk_pressure = _clamp(
        ((max((risk_events_value or 0.0) - 2.0, 0.0) / 4.0) + ((miscoverage_pct or 0.0) / 12.0) + (reject_l1 / 0.35)) / 2.4,
        0.0,
        1.0,
    )
    if risk_pressure > 0.1:
        proposals.append(
            {
                "parameter": "VAR_THRESHOLD_PCT",
                "current_assumption": 0.05,
                "recommended_value": _round2(0.05 - 0.015 * risk_pressure),
                "direction": "decrease",
                "expected_uplift_pct_points": _round2(0.08 + 0.35 * risk_pressure),
                "risk_impact_score": _round2(40 + 40 * risk_pressure),
                "heuristic": {
                    "formula": "risk_pressure=((max(risk_events-2,0)/4)+(miscoverage/12)+(reject_l1/0.35))/2.4",
                    "inputs": {
                        "risk_events_value": risk_events_value,
                        "miscoverage_pct": miscoverage_pct,
                        "reject_mix_l1": reject_l1,
                        "risk_pressure": _round2(risk_pressure),
                    },
                    "rationale": "Lower VaR tolerance when reliability and rejection drift worsen to cap downside tails.",
                },
            }
        )

    if fee_drag_pct > 25.0:
        fee_pressure = _clamp((fee_drag_pct - 25.0) / 20.0, 0.0, 1.0)
        proposals.append(
            {
                "parameter": "PAPER_MIN_TRADE_USDC",
                "current_assumption": 5,
                "recommended_value": _round2(5 + 2 * fee_pressure),
                "direction": "increase",
                "expected_uplift_pct_points": _round2(0.05 + 0.3 * fee_pressure),
                "risk_impact_score": _round2(8 + 18 * fee_pressure),
                "heuristic": {
                    "formula": "fee_pressure=clamp((fee_drag_pct-25)/20,0,1)",
                    "inputs": {"fee_drag_pct": fee_drag_pct, "fee_pressure": _round2(fee_pressure)},
                    "rationale": "Increase minimum notional to avoid fee-dominated micro-trades.",
                },
            }
        )

    if pnl_return_pct < 1.0 and slippage_excess_bps > 10.0:
        alpha = _clamp((1.0 - pnl_return_pct) / 2.0 + slippage_excess_bps / 120.0, 0.0, 1.0)
        proposals.append(
            {
                "parameter": "AUTOCLAIM_TIERS",
                "current_assumption": "40:0.30,70:0.30,100:1.0",
                "recommended_value": "35:0.35,60:0.35,100:1.0",
                "direction": "decrease_targets",
                "expected_uplift_pct_points": _round2(0.07 + 0.32 * alpha),
                "risk_impact_score": _round2(15 + 20 * alpha),
                "heuristic": {
                    "formula": "alpha=clamp((1-pnl_return_pct)/2 + slippage_excess_bps/120,0,1)",
                    "inputs": {
                        "pnl_return_pct": pnl_return_pct,
                        "slippage_excess_bps": slippage_excess_bps,
                        "alpha": _round2(alpha),
                    },
                    "rationale": "Take profits earlier when realized edge is weak and execution frictions are elevated.",
                },
            }
        )

    for proposal in proposals:
        uplift = _coerce_float(proposal.get("expected_uplift_pct_points")) or 0.0
        risk_score = _coerce_float(proposal.get("risk_impact_score")) or 0.0
        proposal["rank_score"] = _round2((uplift * 100.0 * 0.7) + (risk_score * 0.3))

    ranked = sorted(proposals, key=lambda row: float(row.get("rank_score", 0.0)), reverse=True)
    for idx, proposal in enumerate(ranked, start=1):
        proposal["rank"] = idx

    return {
        "generated_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "run_id": run_id,
        "input_metrics": {
            "pnl_return_pct": pnl_return_pct,
            "fee_drag_pct": fee_drag_pct,
            "fill_rate_drop_pp": fill_rate_drop_pp,
            "slippage_excess_bps": slippage_excess_bps,
            "reject_mix_l1": reject_l1,
            "notional_l1": notional_l1,
            "category_l1": category_l1,
            "miscoverage_pct": miscoverage_pct,
            "risk_events_value": risk_events_value,
            "rate_limit_hits": rate_limit_hits,
            "fee_to_edge_hits": fee_to_edge_hits,
        },
        "ranking_method": "rank_score = 0.7*(expected_uplift_pct_points*100) + 0.3*risk_impact_score",
        "proposal_count": len(ranked),
        "proposals": ranked,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate ranked next-iteration parameter recommendations from 24h eval artifacts."
    )
    parser.add_argument("--run-id", default="", help="Run identifier used to resolve default artifact paths.")
    parser.add_argument("--output-dir", default="logs\\eval-cycle", help="Root directory for run artifacts.")
    parser.add_argument("--report-file", default="", help="Path to report artifact JSON.")
    parser.add_argument("--drift-file", default="", help="Path to drift artifact JSON.")
    parser.add_argument("--conformal-file", default="", help="Optional conformal artifact JSON.")
    parser.add_argument("--decision-file", default="", help="Optional decision artifact JSON.")
    parser.add_argument("--out-json", default="", help="Output JSON path.")
    parser.add_argument("--out-summary", default="", help="Output summary text path.")
    return parser.parse_args()


def _resolve_path(path_arg: str, *, run_id: str, output_dir: str, default_name: str) -> Path:
    if path_arg:
        return Path(path_arg).resolve()
    if not run_id:
        raise ValueError(f"Missing --run-id and explicit file path for {default_name}")
    return (Path(output_dir).resolve() / run_id / default_name).resolve()


def _write_summary(summary_path: Path, payload: dict[str, Any]) -> None:
    lines = [
        f"Run {payload.get('run_id', 'unknown')}: {payload.get('proposal_count', 0)} ranked parameter recommendation(s).",
    ]
    proposals = payload.get("proposals")
    if isinstance(proposals, list) and proposals:
        for row in proposals[:3]:
            if not isinstance(row, dict):
                continue
            lines.append(
                f"- #{row.get('rank')} {row.get('parameter')}: {row.get('direction')} -> {row.get('recommended_value')} "
                f"(uplift {row.get('expected_uplift_pct_points')}pp, risk {row.get('risk_impact_score')})"
            )
    else:
        lines.append("- No triggered proposals (artifact metrics were within heuristic guardrails).")
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()
    report_path = _resolve_path(args.report_file, run_id=args.run_id, output_dir=args.output_dir, default_name="report.json")
    drift_path = _resolve_path(args.drift_file, run_id=args.run_id, output_dir=args.output_dir, default_name="drift-matrix.json")
    conformal_path = _resolve_path(
        args.conformal_file,
        run_id=args.run_id,
        output_dir=args.output_dir,
        default_name="conformal-summary.json",
    )
    decision_path = _resolve_path(args.decision_file, run_id=args.run_id, output_dir=args.output_dir, default_name="decision.json")

    report = _read_json_object(report_path)
    drift = _read_json_object(drift_path)
    conformal = _optional_json_object(conformal_path)
    decision = _optional_json_object(decision_path)

    run_id = args.run_id or str(report.get("run_id") or drift.get("run_id") or "unknown-run")
    payload = _build_recommendations(
        run_id=run_id,
        report=report,
        drift=drift,
        conformal=conformal,
        decision=decision,
    )
    payload["artifacts"] = {
        "report": str(report_path),
        "drift": str(drift_path),
        "conformal": str(conformal_path),
        "decision": str(decision_path),
        "conformal_present": conformal is not None,
        "decision_present": decision is not None,
    }

    out_json = _resolve_path(
        args.out_json,
        run_id=run_id,
        output_dir=args.output_dir,
        default_name="parameter-recommendations.json",
    )
    out_summary = _resolve_path(
        args.out_summary,
        run_id=run_id,
        output_dir=args.output_dir,
        default_name="parameter-recommendations.summary.txt",
    )

    out_json.parent.mkdir(parents=True, exist_ok=True)
    out_json.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")
    _write_summary(out_summary, payload)

    print(f"Wrote {out_json}")
    print(f"Wrote {out_summary}")
    print(f"Generated {payload.get('proposal_count', 0)} recommendation(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
