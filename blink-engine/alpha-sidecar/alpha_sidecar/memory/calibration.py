"""Calibration metrics for Alpha AI predictions.

Computes Brier scores, reliability diagrams, and expected calibration
error (ECE) from resolved predictions stored in the prediction store.
Uses adaptive bin counts to handle small sample sizes gracefully.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field

from .prediction_store import PredictionStore

logger = logging.getLogger(__name__)

MIN_PER_BIN = 5


@dataclass
class CalibrationReport:
    """Full calibration summary for the Alpha AI."""
    total_predictions: int
    total_resolved: int
    brier_overall: float | None
    accuracy: float | None
    win_rate: float | None
    ece: float | None
    brier_by_category: dict[str, float] = field(default_factory=dict)
    brier_by_model: dict[str, float] = field(default_factory=dict)
    accuracy_by_category: dict[str, float] = field(default_factory=dict)
    reliability_diagram: list[dict] = field(default_factory=list)
    brier_trend: list[dict] = field(default_factory=list)
    sufficient_data: bool = False

    def to_dict(self) -> dict:
        """Serialize to JSON-safe dict for API responses."""
        return {
            "total_predictions": self.total_predictions,
            "total_resolved": self.total_resolved,
            "brier_overall": self.brier_overall,
            "accuracy": self.accuracy,
            "win_rate": self.win_rate,
            "ece": self.ece,
            "brier_by_category": self.brier_by_category,
            "brier_by_model": self.brier_by_model,
            "accuracy_by_category": self.accuracy_by_category,
            "reliability_diagram": self.reliability_diagram,
            "brier_trend": self.brier_trend,
            "sufficient_data": self.sufficient_data,
        }


class CalibrationTracker:
    """Computes calibration metrics from resolved predictions."""

    def __init__(self, store: PredictionStore) -> None:
        self._store = store

    async def compute_report(self) -> CalibrationReport:
        """Generate a full calibration report from resolved predictions."""
        all_resolved = await self._store.get_resolved(limit=1000)
        stats = await self._store.get_stats()

        total = stats.get("total", 0) or 0
        n_resolved = len(all_resolved)
        sufficient = n_resolved >= 30

        scored = [
            p for p in all_resolved
            if p.get("predicted_prob") is not None and p.get("brier_score") is not None
        ]

        brier_overall = _mean([p["brier_score"] for p in scored]) if scored else None

        with_direction = [p for p in all_resolved if p.get("was_correct") is not None]
        accuracy = (
            sum(1 for p in with_direction if p["was_correct"]) / len(with_direction)
            if with_direction else None
        )

        submitted = [p for p in with_direction if p.get("filter_status") == "submitted"]
        win_rate = (
            sum(1 for p in submitted if p["was_correct"]) / len(submitted)
            if submitted else None
        )

        brier_by_cat = _group_brier(scored, "category")
        brier_by_model = _group_brier(scored, "model_used")

        acc_by_cat: dict[str, float] = {}
        for cat, group in _group_by(with_direction, "category").items():
            correct = sum(1 for p in group if p["was_correct"])
            acc_by_cat[cat] = round(correct / len(group), 4) if group else 0.0

        reliability = _reliability_diagram(scored) if scored else []
        ece = _compute_ece(reliability) if reliability else None
        brier_trend = _brier_trend(scored, window=20)

        return CalibrationReport(
            total_predictions=total,
            total_resolved=n_resolved,
            brier_overall=round(brier_overall, 6) if brier_overall is not None else None,
            accuracy=round(accuracy, 4) if accuracy is not None else None,
            win_rate=round(win_rate, 4) if win_rate is not None else None,
            ece=round(ece, 6) if ece is not None else None,
            brier_by_category={k: round(v, 6) for k, v in brier_by_cat.items()},
            brier_by_model={k: round(v, 6) for k, v in brier_by_model.items()},
            accuracy_by_category=acc_by_cat,
            reliability_diagram=reliability,
            brier_trend=brier_trend,
            sufficient_data=sufficient,
        )


def _mean(vals: list[float]) -> float:
    return sum(vals) / len(vals) if vals else 0.0


def _group_by(items: list[dict], key: str) -> dict[str, list[dict]]:
    groups: dict[str, list[dict]] = {}
    for item in items:
        k = item.get(key) or "unknown"
        groups.setdefault(k, []).append(item)
    return groups


def _group_brier(scored: list[dict], key: str) -> dict[str, float]:
    result: dict[str, float] = {}
    for group_key, group in _group_by(scored, key).items():
        briers = [p["brier_score"] for p in group if p.get("brier_score") is not None]
        if briers:
            result[group_key] = _mean(briers)
    return result


def _reliability_diagram(scored: list[dict]) -> list[dict]:
    """Adaptive-bin reliability diagram."""
    n = len(scored)
    if n < MIN_PER_BIN:
        return []

    n_bins = min(10, max(2, n // MIN_PER_BIN))
    bin_width = 1.0 / n_bins

    bins: list[dict] = []
    for i in range(n_bins):
        lo = i * bin_width
        hi = (i + 1) * bin_width

        in_bin = [
            p for p in scored
            if lo <= (p.get("predicted_prob") or 0) < hi
        ]

        if not in_bin:
            continue

        avg_pred = _mean([p["predicted_prob"] for p in in_bin])
        actuals = [p["actual_outcome"] for p in in_bin if p.get("actual_outcome") is not None]
        avg_actual = _mean(actuals) if actuals else 0.0

        bins.append({
            "bin_center": round((lo + hi) / 2.0, 3),
            "avg_predicted": round(avg_pred, 4),
            "avg_actual": round(avg_actual, 4),
            "count": len(in_bin),
        })

    return bins


def _compute_ece(reliability: list[dict]) -> float:
    """Expected Calibration Error."""
    total_count = sum(b["count"] for b in reliability)
    if total_count == 0:
        return 0.0

    ece = 0.0
    for b in reliability:
        weight = b["count"] / total_count
        ece += weight * abs(b["avg_predicted"] - b["avg_actual"])
    return ece


def _brier_trend(scored: list[dict], window: int = 20) -> list[dict]:
    """Rolling Brier score over windows of predictions."""
    if len(scored) < window:
        if scored:
            return [{
                "window_start": 0,
                "brier": round(_mean([p["brier_score"] for p in scored]), 6),
                "count": len(scored),
            }]
        return []

    sorted_preds = sorted(scored, key=lambda p: p.get("resolved_at") or "")

    trend = []
    for i in range(0, len(sorted_preds) - window + 1, max(1, window // 2)):
        chunk = sorted_preds[i : i + window]
        avg_brier = _mean([p["brier_score"] for p in chunk])
        trend.append({
            "window_start": i,
            "brier": round(avg_brier, 6),
            "count": len(chunk),
        })

    return trend
