"""Category-aware market scanner for the alpha sidecar.

**Latency class: COLD PATH.** Supplements Gamma API discovery by sorting
and filtering markets for the highest inefficiency opportunity.

Key insight from artvandelay/polymarket-agents:
- Very-high-volume markets (> $500k) are efficient — skip them.
- Very-low-volume markets (< $5k) are illiquid — skip them.
- Mid-range volume ($5k–$500k) is the sweet spot for alpha.
- Wide spreads signal genuine disagreement — prioritise those.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field

from ..connectors.gamma import GammaMarket

logger = logging.getLogger(__name__)

# Blocked categories (inherently unpredictable, poor signal quality)
_BLOCKED_CATEGORIES: frozenset[str] = frozenset(
    {
        "esports",
        "gaming",
        "entertainment",
        "reality-tv",
        "celebrity",
    }
)

# Volume sweet-spot for alpha (too big = efficient, too small = illiquid)
MIN_ALPHA_VOLUME_USDC: float = 5_000.0
MAX_ALPHA_VOLUME_USDC: float = 500_000.0

# Blocked title keywords (copied from engine's market category block list)
_BLOCKED_KEYWORDS: tuple[str, ...] = (
    "esports",
    "lol:",
    "cs2:",
    "cs:go",
    "dota",
    "valorant",
    "league of legends",
    "counter-strike",
    "overwatch",
    "bo3)",
    "bo5)",
    "lec ",
    "lck ",
    "lpl ",
    "vct ",
)


@dataclass
class ScoredMarket:
    """A market with an attached inefficiency score for prioritisation."""

    market: GammaMarket
    # Higher = more likely to contain alpha
    inefficiency_score: float = 0.0
    spread_pct: float | None = None
    notes: list[str] = field(default_factory=list)


def score_markets(
    markets: list[GammaMarket],
    min_volume: float = MIN_ALPHA_VOLUME_USDC,
    max_volume: float = MAX_ALPHA_VOLUME_USDC,
) -> list[GammaMarket]:
    """Filter and sort markets by estimated inefficiency.

    Returns markets sorted highest-score first, applying:
    1. Volume band filter (remove mega and micro markets)
    2. Category / keyword block
    3. Price extremity filter (prices near 0 or 1 have little alpha left)
    4. Score by: mid-price distance from 0.5 (more uncertain → more alpha)
    """
    scored: list[ScoredMarket] = []

    for m in markets:
        # ── Volume filter ──────────────────────────────────────────────────
        if m.volume_usdc < min_volume or m.volume_usdc > max_volume:
            continue

        # ── Category / keyword block ───────────────────────────────────────
        category = str(m.extra.get("category") or "").lower()
        if category in _BLOCKED_CATEGORIES:
            continue

        question_lower = m.question.lower()
        if any(kw in question_lower for kw in _BLOCKED_KEYWORDS):
            continue

        # ── Price extremity filter — no alpha when market is resolved ──────
        if m.yes_price < 0.05 or m.yes_price > 0.95:
            continue

        # ── Score ──────────────────────────────────────────────────────────
        # Price uncertainty: market at 0.5 is most uncertain → most alpha
        price_uncertainty = 1.0 - abs(m.yes_price - 0.5) * 2  # 0.0–1.0

        # Volume sweet-spot boost: mid-range volume scores highest
        vol_mid = (min_volume + max_volume) / 2
        vol_distance = abs(m.volume_usdc - vol_mid) / vol_mid
        vol_score = max(0.0, 1.0 - vol_distance)

        inefficiency_score = price_uncertainty * 0.6 + vol_score * 0.4

        scored.append(
            ScoredMarket(
                market=m,
                inefficiency_score=inefficiency_score,
            )
        )

    scored.sort(key=lambda s: s.inefficiency_score, reverse=True)

    result = [s.market for s in scored]
    logger.info(
        "Scanner: %d → %d markets after filtering (vol $%.0f–$%.0f)",
        len(markets),
        len(result),
        min_volume,
        max_volume,
    )
    return result
