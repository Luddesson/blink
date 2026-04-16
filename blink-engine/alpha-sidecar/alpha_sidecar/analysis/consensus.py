"""Multi-model consensus for high-edge signals.

Implements self-consistency sampling: run the same analysis N times and
check agreement. Disagreement reduces confidence; strong consensus boosts it.

Tier escalation:
  - Tier 1 (default): gpt-4o-mini — fast, cheap, used for all signals
  - Tier 2 (edge ≥ 300bps): 3x self-consistency at temp=0.5
  - Tier 3 (edge ≥ 500bps): escalate to gpt-4o for the final consensus call
"""

from __future__ import annotations

import asyncio
import json
import logging
import statistics

from openai import AsyncOpenAI

from ..config import AlphaConfig
from ..connectors.clob import OrderbookSnapshot
from ..connectors.gamma import GammaMarket
from .prompts import detect_category, get_deep_analysis_prompt

logger = logging.getLogger(__name__)

# Thresholds for escalation
CONSENSUS_EDGE_BPS = 300  # ≥3% edge triggers 3x sampling
ESCALATION_EDGE_BPS = 500  # ≥5% edge escalates model tier
CONSENSUS_SAMPLES = 3
CONSENSUS_TEMP = 0.5

# Escalation model — used for Tier 3
TIER3_MODEL = "gpt-4o"


async def _single_sample(
    client: AsyncOpenAI,
    model: str,
    prompt: str,
    temperature: float,
) -> dict | None:
    """Run one LLM call and parse the JSON response."""
    try:
        resp = await client.chat.completions.create(
            model=model,
            messages=[{"role": "user", "content": prompt}],
            temperature=temperature,
            max_tokens=800,
            response_format={"type": "json_object"},
        )
        raw = resp.choices[0].message.content or ""
        data = json.loads(raw)
        tokens = resp.usage.total_tokens if resp.usage else 0
        data["_tokens"] = tokens
        return data
    except Exception as e:
        logger.warning("Consensus sample failed: %s", e)
        return None


async def run_consensus(
    market: GammaMarket,
    cfg: AlphaConfig,
    client: AsyncOpenAI,
    initial_edge_bps: float,
    clob: OrderbookSnapshot | None = None,
    price_change_1h: float | None = None,
    news_context: str | None = None,
) -> dict | None:
    """Run multi-model consensus if edge warrants it.

    Returns a dict with consensus results, or None if consensus not triggered
    or all samples failed.

    Result dict:
        probabilities: list[float]  — individual sample probabilities
        mean_probability: float
        std_probability: float
        consensus_confidence: float — 1.0 if all agree, reduced by disagreement
        model_used: str
        total_tokens: int
        escalated: bool
    """
    if initial_edge_bps < CONSENSUS_EDGE_BPS:
        return None

    # Determine model tier
    escalated = initial_edge_bps >= ESCALATION_EDGE_BPS
    model = TIER3_MODEL if escalated else cfg.openai_model

    category = detect_category(market.question, market.description or "")

    price_change_str: str | None = None
    if price_change_1h is not None:
        direction = "+" if price_change_1h >= 0 else ""
        price_change_str = f"{direction}{price_change_1h:.2%}"

    prompt = get_deep_analysis_prompt(
        question=market.question,
        description=market.description or "",
        yes_price=market.yes_price,
        no_price=market.no_price,
        volume=market.volume_usdc,
        end_date=market.end_date_iso or "unknown",
        category=category,
        clob_best_bid=clob.best_bid if clob else None,
        clob_best_ask=clob.best_ask if clob else None,
        clob_spread_bps=clob.spread_pct * 10_000 if clob else None,
        clob_bid_depth=clob.bid_depth_usdc if clob else None,
        clob_ask_depth=clob.ask_depth_usdc if clob else None,
        price_change_1h=price_change_str,
        news_context=news_context,
    )

    # Run N samples concurrently
    tasks = [
        _single_sample(client, model, prompt, CONSENSUS_TEMP)
        for _ in range(CONSENSUS_SAMPLES)
    ]
    results = await asyncio.gather(*tasks)
    valid = [r for r in results if r is not None]

    if len(valid) < 2:
        logger.warning("Consensus: only %d/%d samples succeeded", len(valid), CONSENSUS_SAMPLES)
        return None

    probabilities = []
    total_tokens = 0
    for r in valid:
        prob = r.get("probability")
        total_tokens += r.get("_tokens", 0)
        if prob is not None:
            try:
                p = float(prob)
                if 0.0 <= p <= 1.0:
                    probabilities.append(p)
            except (TypeError, ValueError):
                pass

    if len(probabilities) < 2:
        return None

    mean_prob = statistics.mean(probabilities)
    std_prob = statistics.stdev(probabilities) if len(probabilities) > 1 else 0.0

    # Consensus confidence: high agreement → boost, disagreement → penalty
    # std of 0 → confidence 1.0; std of 0.15+ → confidence 0.5
    consensus_confidence = max(0.3, 1.0 - (std_prob / 0.15))

    logger.info(
        "Consensus: %s | %d samples | mean=%.3f std=%.3f conf=%.2f | model=%s | %d tok",
        market.question[:40],
        len(probabilities),
        mean_prob,
        std_prob,
        consensus_confidence,
        model,
        total_tokens,
    )

    return {
        "probabilities": [round(p, 4) for p in probabilities],
        "mean_probability": round(mean_prob, 4),
        "std_probability": round(std_prob, 4),
        "consensus_confidence": round(consensus_confidence, 3),
        "model_used": model,
        "total_tokens": total_tokens,
        "escalated": escalated,
        "n_samples": len(probabilities),
    }
