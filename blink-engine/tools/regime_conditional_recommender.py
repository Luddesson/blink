#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import re
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


def _pick_nested_scalar(payload: dict[str, Any], keys: tuple[str, ...]) -> float | None:
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


def _clamp01(value: float) -> float:
    return max(0.0, min(1.0, value))


def _round2(value: float) -> float:
    return round(value, 2)


def _confidence_level(score: float) -> str:
    if score >= 0.75:
        return "HIGH"
    if score >= 0.55:
        return "MEDIUM"
    return "LOW"


def _extract_rejection_counts(
    *,
    rejections_payload: dict[str, Any] | None,
    snapshots_dir: Path,
    report_payload: dict[str, Any],
) -> tuple[dict[str, float], str]:
    if rejections_payload is not None:
        counts = rejections_payload.get("counts_by_reason")
        if isinstance(counts, dict):
            normalized = {
                str(key).strip().lower(): float(value)
                for key, value in counts.items()
                if isinstance(key, str) and _coerce_float(value) is not None and (_coerce_float(value) or 0.0) > 0
            }
            if normalized:
                return normalized, "rejections-file:counts_by_reason"

    snapshots = sorted(snapshots_dir.glob("snapshot-*.json"))
    if snapshots:
        latest = snapshots[-1]
        payload = _optional_json_object(latest)
        if payload is not None:
            data = payload.get("data")
            if isinstance(data, dict):
                rejections = data.get("/api/rejections")
                if isinstance(rejections, dict):
                    counts = rejections.get("counts_by_reason")
                    if isinstance(counts, dict):
                        normalized = {
                            str(key).strip().lower(): float(value)
                            for key, value in counts.items()
                            if isinstance(key, str)
                            and _coerce_float(value) is not None
                            and (_coerce_float(value) or 0.0) > 0
                        }
                        if normalized:
                            return normalized, f"snapshot:{latest.name}:/api/rejections.counts_by_reason"

    rows = report_payload.get("gate_pressure_top5_run_window")
    if isinstance(rows, list):
        fallback: dict[str, float] = {}
        for row in rows:
            if not isinstance(row, dict):
                continue
            key = row.get("gate")
            value = _coerce_float(row.get("rejections_delta"))
            if isinstance(key, str) and value is not None and value > 0:
                fallback[key.strip().lower()] = value
        if fallback:
            return fallback, "report:gate_pressure_top5_run_window"

    return {}, "unavailable"


def _segment_rows(regime_payload: dict[str, Any]) -> list[dict[str, Any]]:
    direct = regime_payload.get("segments")
    if isinstance(direct, list):
        return [row for row in direct if isinstance(row, dict)]
    nested = regime_payload.get("summary")
    if isinstance(nested, dict) and isinstance(nested.get("segments"), list):
        return [row for row in nested["segments"] if isinstance(row, dict)]
    return []


def _recommend_for_regime(
    *,
    regime: str,
    samples: int,
    total_samples: int,
    avg_return: float,
    avg_drawdown_pct: float,
    avg_vol_robust_zscore: float,
    avg_trend_tscore: float,
    drift_payload: dict[str, Any],
    rejection_counts: dict[str, float],
    fee_drag_pct: float,
) -> tuple[list[dict[str, Any]], list[str], float]:
    support = (samples / total_samples) if total_samples > 0 else 0.0
    drift_delta = drift_payload.get("delta", {}) if isinstance(drift_payload.get("delta"), dict) else {}
    fill_drop_pp = max(0.0, -(_coerce_float(drift_delta.get("fill_rate_pct_points")) or 0.0))
    slippage_excess_bps = max(0.0, _coerce_float(drift_delta.get("avg_slippage_bps")) or 0.0)
    reject_l1 = _coerce_float(
        ((drift_delta.get("reject_mix") or {}).get("l1_distance")) if isinstance(drift_delta.get("reject_mix"), dict) else 0.0
    ) or 0.0

    rate_limit_hits = sum(value for key, value in rejection_counts.items() if "rate" in key)
    fee_gate_hits = sum(value for key, value in rejection_counts.items() if "fee" in key)

    regime_stress = _clamp01(
        (
            max(-avg_return, 0.0) * 24.0
            + max(-avg_drawdown_pct, 0.0) / 3.0
            + max(avg_vol_robust_zscore, 0.0) / 2.0
            + (slippage_excess_bps / 80.0)
            + (fill_drop_pp / 8.0)
        )
        / 3.6
    )
    execution_pressure = _clamp01((slippage_excess_bps / 90.0) + (reject_l1 / 0.35) + (rate_limit_hits / 8.0))
    fee_pressure = _clamp01((max(fee_drag_pct - 18.0, 0.0) / 22.0) + (fee_gate_hits / 10.0))

    recommendations: list[dict[str, Any]] = []
    rationale: list[str] = []

    def add_recommendation(
        *,
        parameter: str,
        current_assumption: float | int | str,
        recommended_value: float | int | str,
        direction: str,
        impact: float,
        reason: str,
        evidence: dict[str, float | int | None],
    ) -> None:
        score = _round2((impact * 0.55) + (support * 0.25) + ((1.0 - regime_stress) * 0.20))
        recommendations.append(
            {
                "parameter": parameter,
                "current_assumption": current_assumption,
                "recommended_value": recommended_value,
                "direction": direction,
                "score": score,
                "rationale": reason,
                "evidence": evidence,
            }
        )

    if regime in {"high_volatility", "drawdown_stress"}:
        intensity = _clamp01(regime_stress + execution_pressure * 0.35)
        add_recommendation(
            parameter="PAPER_SIZE_MULTIPLIER",
            current_assumption=0.20,
            recommended_value=_round2(0.20 * (1.0 - 0.30 * intensity)),
            direction="decrease",
            impact=_round2(0.55 + 0.35 * intensity),
            reason="Reduce exposure in stressed volatility regimes to preserve convexity and avoid slippage-driven adverse fills.",
            evidence={
                "regime_stress": _round2(regime_stress),
                "execution_pressure": _round2(execution_pressure),
                "avg_vol_robust_zscore": _round2(avg_vol_robust_zscore),
            },
        )
        add_recommendation(
            parameter="VAR_THRESHOLD_PCT",
            current_assumption=0.05,
            recommended_value=_round2(max(0.02, 0.05 - 0.02 * intensity)),
            direction="decrease",
            impact=_round2(0.48 + 0.30 * intensity),
            reason="Tighten VaR ceiling when drawdown stress and rejection drift co-occur.",
            evidence={
                "avg_drawdown_pct": _round2(avg_drawdown_pct),
                "reject_mix_l1": _round2(reject_l1),
                "fill_drop_pp": _round2(fill_drop_pp),
            },
        )
        rationale.append("Stress regime detected: prioritize downside containment over participation.")

    if regime in {"low_volatility", "neutral"} and execution_pressure < 0.65:
        loosen = _clamp01((1.0 - execution_pressure) * 0.9 + max(avg_return, 0.0) * 6.0)
        add_recommendation(
            parameter="PRICE_DRIFT_ABORT_BPS",
            current_assumption=150,
            recommended_value=int(round(150 + 40 * loosen)),
            direction="increase",
            impact=_round2(0.30 + 0.30 * loosen),
            reason="Loosen drift abort in stable regimes to improve fill conversion without materially degrading price quality.",
            evidence={
                "execution_pressure": _round2(execution_pressure),
                "avg_vol_robust_zscore": _round2(avg_vol_robust_zscore),
                "fill_drop_pp": _round2(fill_drop_pp),
            },
        )
        rationale.append("Stable execution profile supports selective fill-rate recovery.")

    if regime in {"trend_up", "trend_down"}:
        trend_strength = _clamp01(abs(avg_trend_tscore) / 2.0)
        if avg_trend_tscore > 0:
            add_recommendation(
                parameter="AUTOCLAIM_TIERS",
                current_assumption="40:0.30,70:0.30,100:1.0",
                recommended_value="50:0.25,90:0.35,140:1.0",
                direction="increase_targets",
                impact=_round2(0.28 + 0.34 * trend_strength),
                reason="Uptrend regime supports wider upside capture before full exit.",
                evidence={
                    "avg_trend_tscore": _round2(avg_trend_tscore),
                    "trend_strength": _round2(trend_strength),
                    "avg_return": round(avg_return, 6),
                },
            )
        else:
            add_recommendation(
                parameter="AUTOCLAIM_TIERS",
                current_assumption="40:0.30,70:0.30,100:1.0",
                recommended_value="30:0.40,55:0.35,90:1.0",
                direction="decrease_targets",
                impact=_round2(0.32 + 0.30 * trend_strength),
                reason="Downtrend regime favors faster de-risking to defend realized edge.",
                evidence={
                    "avg_trend_tscore": _round2(avg_trend_tscore),
                    "trend_strength": _round2(trend_strength),
                    "avg_return": round(avg_return, 6),
                },
            )
        rationale.append("Directional regime detected: align profit-taking cadence to trend persistence.")

    if rate_limit_hits > 0 and support >= 0.15:
        add_recommendation(
            parameter="MAX_ORDERS_PER_SECOND",
            current_assumption=3,
            recommended_value=int(round(3 + min(2.0, rate_limit_hits))),
            direction="increase",
            impact=_round2(0.20 + 0.18 * _clamp01(rate_limit_hits / 6.0)),
            reason="Recover missed opportunities where rate-limit pressure is measurable.",
            evidence={
                "rate_limit_hits": int(round(rate_limit_hits)),
                "support": _round2(support),
                "execution_pressure": _round2(execution_pressure),
            },
        )
        rationale.append("Gate-pressure indicates throttling in this regime support window.")

    if fee_pressure > 0.2:
        add_recommendation(
            parameter="PAPER_MIN_TRADE_USDC",
            current_assumption=5,
            recommended_value=_round2(5 + 2.5 * fee_pressure),
            direction="increase",
            impact=_round2(0.18 + 0.20 * fee_pressure),
            reason="Raise minimum trade size when fee drag dominates marginal edge.",
            evidence={
                "fee_drag_pct": _round2(fee_drag_pct),
                "fee_gate_hits": int(round(fee_gate_hits)),
                "fee_pressure": _round2(fee_pressure),
            },
        )
        rationale.append("Fee drag pressure argues against micro-notional entries.")

    if not recommendations:
        rationale.append("No strong regime-specific adjustment triggered; keep baseline parameters and continue monitoring.")

    recommendations.sort(key=lambda row: (-float(row.get("score", 0.0)), str(row.get("parameter", ""))))
    for idx, row in enumerate(recommendations, start=1):
        row["rank"] = idx

    signal_strength = _clamp01((0.55 * regime_stress) + (0.45 * execution_pressure))
    return recommendations, sorted(set(rationale)), signal_strength


def build_regime_conditional_recommendations(
    *,
    run_id: str,
    regime_payload: dict[str, Any],
    report_payload: dict[str, Any],
    drift_payload: dict[str, Any],
    rejections_payload: dict[str, Any] | None,
    snapshots_dir: Path,
) -> dict[str, Any]:
    segments = _segment_rows(regime_payload)
    if not segments:
        raise ValueError("Regime artifact does not contain a non-empty segments array")

    total_samples = sum(
        int(_coerce_float(segment.get("samples")) or 0)
        for segment in segments
        if isinstance(segment, dict)
    )
    if total_samples <= 0:
        raise ValueError("Regime segments contain no valid sample counts")

    rejection_counts, rejection_source = _extract_rejection_counts(
        rejections_payload=rejections_payload,
        snapshots_dir=snapshots_dir,
        report_payload=report_payload,
    )
    fee_drag_pct = _pick_nested_scalar(report_payload, ("fee_drag_pct", "fee_drag", "fee_drag_percent")) or 0.0

    regime_rows: list[dict[str, Any]] = []
    required_artifacts_present = {
        "regime": True,
        "report": True,
        "drift": True,
        "rejections": rejections_payload is not None,
    }
    artifact_factor = 0.80 + (0.20 * ((3.0 + (1.0 if required_artifacts_present["rejections"] else 0.0)) / 4.0))

    for segment in sorted(segments, key=lambda row: (str(row.get("regime", "")), str(row.get("start_utc", "")))):
        regime = str(segment.get("regime", "unknown")).strip().lower() or "unknown"
        samples = int(_coerce_float(segment.get("samples")) or 0)
        avg_return = _coerce_float(segment.get("avg_return")) or 0.0
        avg_drawdown_pct = _coerce_float(segment.get("avg_drawdown_pct")) or 0.0
        avg_vol_robust_zscore = _coerce_float(segment.get("avg_vol_robust_zscore")) or 0.0
        avg_trend_tscore = _coerce_float(segment.get("avg_trend_tscore")) or 0.0
        strategy_route = str(segment.get("strategy_route", "")).strip()

        recommendations, rationale, signal_strength = _recommend_for_regime(
            regime=regime,
            samples=samples,
            total_samples=total_samples,
            avg_return=avg_return,
            avg_drawdown_pct=avg_drawdown_pct,
            avg_vol_robust_zscore=avg_vol_robust_zscore,
            avg_trend_tscore=avg_trend_tscore,
            drift_payload=drift_payload,
            rejection_counts=rejection_counts,
            fee_drag_pct=fee_drag_pct,
        )

        support = (samples / total_samples) if total_samples > 0 else 0.0
        confidence_score = _clamp01((0.20 + (0.40 * support) + (0.25 * artifact_factor) + (0.15 * signal_strength)))
        row = {
            "regime": regime,
            "strategy_route": strategy_route,
            "segment_window": {
                "start_utc": segment.get("start_utc"),
                "end_utc": segment.get("end_utc"),
            },
            "samples": samples,
            "support_weight": round(support, 6),
            "signal_strength": round(signal_strength, 6),
            "confidence": {
                "score_normalized": round(confidence_score, 6),
                "score_percent": round(confidence_score * 100.0, 2),
                "level": _confidence_level(confidence_score),
                "components": {
                    "support": round(support, 6),
                    "artifact_factor": round(artifact_factor, 6),
                    "signal_strength": round(signal_strength, 6),
                },
            },
            "metrics": {
                "avg_return": round(avg_return, 8),
                "avg_drawdown_pct": round(avg_drawdown_pct, 6),
                "avg_vol_robust_zscore": round(avg_vol_robust_zscore, 6),
                "avg_trend_tscore": round(avg_trend_tscore, 6),
            },
            "recommendation_count": len(recommendations),
            "recommendations": recommendations,
            "rationale": rationale,
        }
        regime_rows.append(row)

    regime_rows.sort(
        key=lambda row: (
            -float(((row.get("confidence") or {}).get("score_normalized")) or 0.0),
            str(row.get("regime", "")),
            str((row.get("segment_window") or {}).get("start_utc", "")),
        )
    )
    for idx, row in enumerate(regime_rows, start=1):
        row["rank"] = idx

    return {
        "schema_version": "1.0.0",
        "generated_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "run_id": run_id,
        "artifacts": {
            "rejection_source": rejection_source,
            "rejections_provided": rejections_payload is not None,
        },
        "determinism": {
            "regime_sort": "(-confidence.score_normalized, regime, segment_window.start_utc)",
            "recommendation_sort": "(-score, parameter)",
            "scoring_version": "regime-conditional-v1",
        },
        "regime_count": len(regime_rows),
        "regimes": regime_rows,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate deterministic regime-conditional parameter recommendations from eval artifacts."
    )
    parser.add_argument("--run-id", default="", help="Run identifier used to resolve default artifact paths.")
    parser.add_argument("--output-dir", default="logs\\eval-cycle", help="Root directory for run artifacts.")
    parser.add_argument("--regime-file", default="", help="Regime summary JSON path (default: <run>/regimes/regime-summary.json).")
    parser.add_argument("--report-file", default="", help="Report JSON path (default: <run>/report.json).")
    parser.add_argument("--drift-file", default="", help="Drift matrix JSON path (default: <run>/drift-matrix.json).")
    parser.add_argument(
        "--rejections-file",
        default="",
        help="Optional explicit rejections JSON path. Falls back to latest snapshot or report gate pressure.",
    )
    parser.add_argument(
        "--snapshots-dir",
        default="",
        help="Optional snapshots directory for rejection fallback (default: <run>).",
    )
    parser.add_argument(
        "--out-json",
        default="",
        help="Output JSON path (default: <run>/regime-conditional-recommendations.json).",
    )
    return parser.parse_args()


def _resolve_path(path_arg: str, *, run_dir: Path, default_name: str) -> Path:
    if path_arg:
        return Path(path_arg).resolve()
    return (run_dir / default_name).resolve()


def _safe_run_id_filename(run_id: str) -> str:
    safe = re.sub(r"[^A-Za-z0-9._-]", "_", run_id).strip("._")
    return safe or hashlib.sha256(run_id.encode("utf-8")).hexdigest()[:16]


def run_recommender(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for regime-conditional-recommender")
    run_dir = (Path(args.output_dir).resolve() / _safe_run_id_filename(args.run_id)).resolve()

    regime_path = _resolve_path(args.regime_file, run_dir=run_dir, default_name="regimes\\regime-summary.json")
    report_path = _resolve_path(args.report_file, run_dir=run_dir, default_name="report.json")
    drift_path = _resolve_path(args.drift_file, run_dir=run_dir, default_name="drift-matrix.json")
    rejections_path = Path(args.rejections_file).resolve() if args.rejections_file else None
    snapshots_dir = Path(args.snapshots_dir).resolve() if args.snapshots_dir else run_dir
    out_path = _resolve_path(
        args.out_json,
        run_dir=run_dir,
        default_name="regime-conditional-recommendations.json",
    )

    regime_payload = _read_json_object(regime_path)
    report_payload = _read_json_object(report_path)
    drift_payload = _read_json_object(drift_path)
    rejections_payload = _optional_json_object(rejections_path) if rejections_path else None

    payload = build_regime_conditional_recommendations(
        run_id=args.run_id,
        regime_payload=regime_payload,
        report_payload=report_payload,
        drift_payload=drift_payload,
        rejections_payload=rejections_payload,
        snapshots_dir=snapshots_dir,
    )
    payload["artifact_paths"] = {
        "regime": str(regime_path),
        "report": str(report_path),
        "drift": str(drift_path),
        "rejections": str(rejections_path) if rejections_path else None,
        "snapshots_dir": str(snapshots_dir),
    }

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {out_path}")
    print(f"Generated {payload.get('regime_count', 0)} regime recommendation block(s)")
    return 0


def main() -> int:
    return run_recommender(parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
