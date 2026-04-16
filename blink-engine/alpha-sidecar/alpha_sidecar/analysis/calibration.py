"""Confidence calibration based on market spread.

**Latency class: COLD PATH.** Applied after LLM analysis, before submission.

A tight spread (< 50bps) indicates an efficient market where the LLM's
alpha edge is likely small. A wide spread (> 200bps) signals genuine
price disagreement — the LLM's view deserves more weight.

Both the raw and calibrated confidence values are logged so post-session
reviews can audit calibration quality.
"""

from __future__ import annotations

import logging

logger = logging.getLogger(__name__)

# Spread thresholds
TIGHT_SPREAD_THRESHOLD_PCT: float = 0.005   # 50bps  → efficient market
WIDE_SPREAD_THRESHOLD_PCT: float = 0.020    # 200bps → genuine inefficiency

# Adjustments
TIGHT_SPREAD_DISCOUNT: float = 0.20   # Reduce confidence by 20%
WIDE_SPREAD_BONUS: float = 0.10       # Increase confidence by 10%


def calibrate_confidence(
    raw_confidence: float,
    spread_pct: float | None,
) -> float:
    """Return a spread-adjusted confidence score clamped to [0.0, 1.0].

    If spread_pct is None (CLOB unavailable), the raw score is returned unchanged
    so the pipeline degrades gracefully when the CLOB API is down.
    """
    if spread_pct is None:
        return raw_confidence

    if spread_pct < TIGHT_SPREAD_THRESHOLD_PCT:
        # Very tight spread → efficient market → trust LLM less
        adjusted = raw_confidence * (1.0 - TIGHT_SPREAD_DISCOUNT)
        logger.debug(
            "Tight spread %.1fbps → confidence %.2f → %.2f (−%.0f%%)",
            spread_pct * 10_000,
            raw_confidence,
            adjusted,
            TIGHT_SPREAD_DISCOUNT * 100,
        )
    elif spread_pct > WIDE_SPREAD_THRESHOLD_PCT:
        # Wide spread → real inefficiency → trust LLM more
        adjusted = raw_confidence * (1.0 + WIDE_SPREAD_BONUS)
        logger.debug(
            "Wide spread %.1fbps → confidence %.2f → %.2f (+%.0f%%)",
            spread_pct * 10_000,
            raw_confidence,
            adjusted,
            WIDE_SPREAD_BONUS * 100,
        )
    else:
        adjusted = raw_confidence

    return max(0.0, min(1.0, adjusted))
