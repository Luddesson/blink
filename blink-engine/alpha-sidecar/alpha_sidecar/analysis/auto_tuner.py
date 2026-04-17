"""Self-improvement loop for Alpha AI.

Runs periodically (default: every 24h) to:
1. Analyze prediction accuracy per category
2. Auto-tune confidence_floor and min_edge_bps based on Brier scores
3. Track improvement trends over time

The tuner reads from PredictionStore and writes adjusted thresholds
to a JSON file that the main loop can reload.
"""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from pathlib import Path

from ..memory.prediction_store import PredictionStore

logger = logging.getLogger(__name__)

TUNER_STATE_PATH = "./data/tuner_state.json"

# Per-category Brier targets — if actual Brier exceeds target, tighten filters
BRIER_TARGET: dict[str, float] = {
    "politics": 0.22,
    "sports": 0.25,
    "crypto": 0.28,
    "geopolitics": 0.20,
    "default": 0.25,
}

# Adjustment step sizes
CONFIDENCE_STEP = 0.02
EDGE_STEP = 25  # bps


@dataclass
class TunerAdjustment:
    """Result of one auto-tuning cycle."""
    category: str
    brier_score: float
    brier_target: float
    n_predictions: int
    confidence_delta: float  # positive = tightened, negative = loosened
    edge_delta: int  # bps; positive = tightened
    recommendation: str


async def run_auto_tuner(
    store: PredictionStore,
    state_path: str = TUNER_STATE_PATH,
) -> list[TunerAdjustment]:
    """Analyze prediction accuracy and compute threshold adjustments.

    Returns a list of per-category adjustments. Writes tuner state to disk.
    """
    stats = await store.get_stats()
    if not stats:
        logger.info("Auto-tuner: no stats available yet")
        return []

    adjustments: list[TunerAdjustment] = []

    # Load existing tuner state
    state = _load_state(state_path)
    category_overrides = state.get("category_overrides", {})

    # Get per-category Brier scores from the store
    category_stats = await store.get_category_stats()

    for cat, cat_data in category_stats.items():
        brier = cat_data.get("brier_score")
        n_preds = cat_data.get("resolved_count", 0)

        if brier is None or n_preds < 10:
            continue

        target = BRIER_TARGET.get(cat, BRIER_TARGET["default"])
        current_overrides = category_overrides.get(cat, {})
        conf_adj = current_overrides.get("confidence_adj", 0.0)
        edge_adj = current_overrides.get("edge_adj", 0)

        if brier > target * 1.2:
            # Performing badly: tighten thresholds
            conf_adj += CONFIDENCE_STEP
            edge_adj += EDGE_STEP
            recommendation = "TIGHTEN — Brier above target"
        elif brier < target * 0.8 and n_preds >= 20:
            # Performing well: can afford to loosen
            conf_adj = max(conf_adj - CONFIDENCE_STEP, -0.10)
            edge_adj = max(edge_adj - EDGE_STEP, -100)
            recommendation = "LOOSEN — Brier below target"
        else:
            recommendation = "HOLD — within acceptable range"

        category_overrides[cat] = {
            "confidence_adj": round(conf_adj, 3),
            "edge_adj": edge_adj,
            "last_brier": round(brier, 4),
            "n_predictions": n_preds,
        }

        adjustments.append(TunerAdjustment(
            category=cat,
            brier_score=round(brier, 4),
            brier_target=target,
            n_predictions=n_preds,
            confidence_delta=round(conf_adj, 3),
            edge_delta=edge_adj,
            recommendation=recommendation,
        ))

    # Save updated state
    state["category_overrides"] = category_overrides
    state["last_run"] = _now_iso()
    state["total_adjustments"] = len(adjustments)
    _save_state(state, state_path)

    for adj in adjustments:
        logger.info(
            "Tuner [%s]: Brier=%.3f (target=%.3f) n=%d → %s (conf%+.3f, edge%+dbps)",
            adj.category, adj.brier_score, adj.brier_target, adj.n_predictions,
            adj.recommendation, adj.confidence_delta, adj.edge_delta,
        )

    return adjustments


def get_category_thresholds(
    base_confidence: float,
    base_edge_bps: int,
    category: str,
    state_path: str = TUNER_STATE_PATH,
) -> tuple[float, int]:
    """Get adjusted thresholds for a category.

    Returns (adjusted_confidence_floor, adjusted_min_edge_bps).
    """
    state = _load_state(state_path)
    overrides = state.get("category_overrides", {}).get(category, {})
    conf_adj = overrides.get("confidence_adj", 0.0)
    edge_adj = overrides.get("edge_adj", 0)

    adjusted_conf = max(0.2, min(0.9, base_confidence + conf_adj))
    adjusted_edge = max(50, base_edge_bps + edge_adj)

    return adjusted_conf, adjusted_edge


def _load_state(path: str) -> dict:
    try:
        return json.loads(Path(path).read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def _save_state(state: dict, path: str) -> None:
    try:
        p = Path(path)
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(json.dumps(state, indent=2))
    except Exception as e:
        logger.error("Failed to save tuner state: %s", e)


def _now_iso() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).isoformat()
