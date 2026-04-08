"""LLM-based market analysis.

Sends a Polymarket market to GPT and asks it to estimate the true
probability that YES resolves. Returns an AlphaSignal if the
LLM's estimate diverges from the market price by more than the edge threshold.
"""

from __future__ import annotations

import json
import logging
import uuid
from dataclasses import dataclass

from openai import AsyncOpenAI

from ..config import AlphaConfig
from ..connectors.gamma import GammaMarket

logger = logging.getLogger(__name__)

ANALYSIS_PROMPT_TEMPLATE = """\
You are a prediction market analyst. Analyse the following Polymarket market
and estimate the true probability that the YES outcome resolves.

Market: {question}
Description: {description}
Current YES price: {yes_price:.2%}   (market-implied probability)
Current NO price:  {no_price:.2%}
24h Volume: ${volume:,.0f}
Closes: {end_date}
Category: {category}

Instructions:
1. Reason about the true probability of YES resolving based on your knowledge.
2. Consider any relevant facts, recent news, or base rates.
3. Be concise but specific in your reasoning.
4. Output ONLY valid JSON with this exact schema:
   {{
     "yes_probability": <float 0.0-1.0>,
     "confidence": <float 0.0-1.0>,
     "reasoning": "<1-3 sentences>",
     "recommended_action": "BUY" | "SELL" | "PASS"
   }}

If you lack sufficient information to form a confident view, set
"recommended_action" to "PASS" and "confidence" below 0.5.
"""


@dataclass
class LLMSignal:
    """Raw output from LLM analysis before risk filtering."""
    market: GammaMarket
    yes_probability: float
    confidence: float
    reasoning: str
    recommended_action: str   # "BUY", "SELL", "PASS"
    analysis_id: str


async def analyse_market(
    market: GammaMarket,
    cfg: AlphaConfig,
    client: AsyncOpenAI,
) -> LLMSignal | None:
    """Call the LLM and parse its probability estimate.

    Returns None if the LLM returns an invalid response or recommends PASS.
    """
    prompt = ANALYSIS_PROMPT_TEMPLATE.format(
        question=market.question,
        description=(market.description or "No description provided.")[:500],
        yes_price=market.yes_price,
        no_price=market.no_price,
        volume=market.volume_usdc,
        end_date=market.end_date_iso or "unknown",
        category=market.extra.get("category") or "unknown",
    )

    try:
        response = await client.chat.completions.create(
            model=cfg.openai_model,
            messages=[{"role": "user", "content": prompt}],
            temperature=0.2,
            max_tokens=300,
            response_format={"type": "json_object"},
        )
    except Exception as e:
        logger.warning("OpenAI API error for market %s: %s", market.token_id[:16], e)
        return None

    raw = response.choices[0].message.content or ""
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        logger.warning("LLM returned non-JSON for %s: %r", market.token_id[:16], raw[:200])
        return None

    yes_prob = float(data.get("yes_probability", -1))
    confidence = float(data.get("confidence", 0))
    action = str(data.get("recommended_action", "PASS")).upper()
    reasoning = str(data.get("reasoning", ""))

    if not (0.0 <= yes_prob <= 1.0):
        logger.debug("LLM gave invalid probability %.3f for %s", yes_prob, market.token_id[:16])
        return None

    if action == "PASS" or confidence < cfg.confidence_floor:
        logger.debug(
            "LLM PASS for %s (action=%s confidence=%.2f)",
            market.question[:60], action, confidence,
        )
        return None

    return LLMSignal(
        market=market,
        yes_probability=yes_prob,
        confidence=confidence,
        reasoning=reasoning,
        recommended_action=action,
        analysis_id=str(uuid.uuid4())[:8],
    )


def compute_edge(llm: LLMSignal) -> float:
    """Edge in basis points: |LLM probability - market price| * 10000."""
    if llm.recommended_action == "BUY":
        return (llm.yes_probability - llm.market.yes_price) * 10_000
    else:  # SELL
        return (llm.market.yes_price - llm.yes_probability) * 10_000
