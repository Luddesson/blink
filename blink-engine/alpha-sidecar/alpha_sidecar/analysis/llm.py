"""LLM-based market analysis.

Sends a Polymarket market to GPT/Grok and asks it to estimate the true
probability that YES resolves. Returns an AlphaSignal if the LLM's estimate
diverges from the market price by more than the edge threshold.

When CLOB data is available (orderbook, spread, price history), it is included
in the prompt to dramatically improve signal quality — the LLM can see whether
the market is liquid/efficient or wide/inefficient.
"""

from __future__ import annotations

import json
import logging
import uuid
from dataclasses import dataclass

from openai import AsyncOpenAI

from ..config import AlphaConfig
from ..connectors.clob import OrderbookSnapshot
from ..connectors.gamma import GammaMarket

logger = logging.getLogger(__name__)

# ─── Prompt template ─────────────────────────────────────────────────────────

_BASE_PROMPT = """\
You are a prediction market analyst. Analyse the following Polymarket market
and estimate the true probability that the YES outcome resolves.

Market: {question}
Description: {description}
Current YES price: {yes_price:.2%}   (market-implied probability)
Current NO price:  {no_price:.2%}
24h Volume: ${volume:,.0f}
Closes: {end_date}
Category: {category}
"""

_CLOB_SECTION = """\

Live Orderbook (CLOB data):
  Best Bid:     {best_bid:.4f}  (highest buyer)
  Best Ask:     {best_ask:.4f}  (lowest seller)
  Spread:       {spread_bps:.0f}bps  ({spread_pct:.2%} of mid)
  Bid Depth:    ${bid_depth_usdc:,.0f} USDC (top 5 levels)
  Ask Depth:    ${ask_depth_usdc:,.0f} USDC (top 5 levels)
  1h Price Δ:   {price_change}
"""

_INSTRUCTIONS = """\

Instructions:
1. Reason about the true probability of YES resolving based on your knowledge.
2. Consider any relevant facts, recent news, or base rates.
3. If orderbook data is present, use the spread and depth to judge market efficiency.
   A wide spread (> 200bps) often signals genuine disagreement — your edge may be real.
   A tight spread (< 50bps) suggests the market is efficient — be conservative.
4. Be concise but specific in your reasoning.
5. Output ONLY valid JSON with this exact schema:
   {{
     "yes_probability": <float 0.0-1.0>,
     "confidence": <float 0.0-1.0>,
     "reasoning": "<1-3 sentences>",
     "recommended_action": "BUY" | "SELL" | "PASS"
   }}

IMPORTANT: Only output "PASS" if you genuinely cannot form ANY directional view.
If you have even a slight lean toward the probability being different from the market
price, output "BUY" (if you think YES is underpriced) or "SELL" (if overpriced)
with an appropriate confidence level. Low confidence (0.4-0.6) is fine — the sizing
algorithm will scale position size accordingly.
"""


def _build_prompt(
    market: GammaMarket,
    clob: OrderbookSnapshot | None,
    price_change_1h: float | None,
) -> str:
    """Build the full analysis prompt, optionally enriched with CLOB data."""
    base = _BASE_PROMPT.format(
        question=market.question,
        description=(market.description or "No description provided.")[:500],
        yes_price=market.yes_price,
        no_price=market.no_price,
        volume=market.volume_usdc,
        end_date=market.end_date_iso or "unknown",
        category=market.extra.get("category") or "unknown",
    )

    if clob is not None:
        if price_change_1h is not None:
            direction = "+" if price_change_1h >= 0 else ""
            change_str = f"{direction}{price_change_1h:.2%}"
        else:
            change_str = "n/a"

        clob_section = _CLOB_SECTION.format(
            best_bid=clob.best_bid,
            best_ask=clob.best_ask,
            spread_bps=clob.spread_pct * 10_000,
            spread_pct=clob.spread_pct,
            bid_depth_usdc=clob.bid_depth_usdc,
            ask_depth_usdc=clob.ask_depth_usdc,
            price_change=change_str,
        )
        return base + clob_section + _INSTRUCTIONS

    return base + _INSTRUCTIONS


# ─── Dataclass ───────────────────────────────────────────────────────────────


@dataclass
class LLMSignal:
    """Raw output from LLM analysis before risk filtering."""

    market: GammaMarket
    yes_probability: float
    confidence: float
    reasoning: str
    recommended_action: str   # "BUY", "SELL", "PASS"
    analysis_id: str
    # CLOB enrichment (set if available — used for Kelly sizing in submission.py)
    clob: OrderbookSnapshot | None = None
    price_change_1h: float | None = None


# ─── Analysis function ────────────────────────────────────────────────────────


async def analyse_market(
    market: GammaMarket,
    cfg: AlphaConfig,
    client: AsyncOpenAI,
    clob: OrderbookSnapshot | None = None,
    price_change_1h: float | None = None,
) -> LLMSignal | None:
    """Call the LLM and parse its probability estimate.

    Pass `clob` and `price_change_1h` for enriched analysis.
    Returns None if the LLM returns an invalid response or recommends PASS.
    """
    prompt = _build_prompt(market, clob, price_change_1h)

    try:
        response = await client.chat.completions.create(
            model=cfg.openai_model,
            messages=[{"role": "user", "content": prompt}],
            temperature=0.2,
            max_tokens=350,
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
        clob=clob,
        price_change_1h=price_change_1h,
    )


def compute_edge(llm: LLMSignal) -> float:
    """Edge in basis points: |LLM probability - market price| * 10000."""
    if llm.recommended_action == "BUY":
        return (llm.yes_probability - llm.market.yes_price) * 10_000
    else:  # SELL
        return (llm.market.yes_price - llm.yes_probability) * 10_000
