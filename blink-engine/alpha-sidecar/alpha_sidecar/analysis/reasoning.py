"""Two-call reasoning chain for enhanced prediction quality.

Call 1 — Deep Analysis: structured Bayesian reasoning with category-specific guidance.
Call 2 — Devil's Advocate: adversarial critique that challenges biases and missed evidence.
Final probability: 70% x Call1 + 30% x Call2 (equal weight if disagreement > 15pp).
"""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field

from openai import AsyncOpenAI

from ..config import AlphaConfig
from ..connectors.clob import OrderbookSnapshot
from ..connectors.gamma import GammaMarket
from .prompts import detect_category, get_deep_analysis_prompt, get_devils_advocate_prompt

logger = logging.getLogger(__name__)

CALL1_WEIGHT = 0.70
CALL2_WEIGHT = 0.30
DISAGREEMENT_THRESHOLD = 0.15  # >15pp → equal weight


@dataclass
class ReasoningChain:
    """Full structured reasoning from the 2-call pipeline."""

    # Call 1 — Deep Analysis
    base_rate: str = ""
    evidence_for: list[str] = field(default_factory=list)
    evidence_against: list[str] = field(default_factory=list)
    bayesian_reasoning: str = ""
    market_efficiency: str = ""
    initial_probability: float = 0.0
    initial_confidence: float = 0.0
    initial_action: str = "PASS"

    # Call 2 — Devil's Advocate
    critique: str = ""
    missed_evidence: list[str] = field(default_factory=list)
    cognitive_biases: list[str] = field(default_factory=list)
    revised_probability: float = 0.0
    revised_confidence: float = 0.0

    # Final (combined)
    final_probability: float = 0.0
    final_confidence: float = 0.0
    final_action: str = "PASS"
    combination_method: str = ""

    # Metadata
    category: str = "default"
    prompt_version: str = "v2.0-reasoning-chain"
    total_tokens: int = 0

    def to_dict(self) -> dict:
        """Serialize for JSON storage / API transport."""
        return {
            "base_rate": self.base_rate,
            "evidence_for": self.evidence_for,
            "evidence_against": self.evidence_against,
            "bayesian_reasoning": self.bayesian_reasoning,
            "market_efficiency": self.market_efficiency,
            "initial_probability": round(self.initial_probability, 4),
            "initial_confidence": round(self.initial_confidence, 4),
            "initial_action": self.initial_action,
            "critique": self.critique,
            "missed_evidence": self.missed_evidence,
            "cognitive_biases": self.cognitive_biases,
            "revised_probability": round(self.revised_probability, 4),
            "revised_confidence": round(self.revised_confidence, 4),
            "final_probability": round(self.final_probability, 4),
            "final_confidence": round(self.final_confidence, 4),
            "final_action": self.final_action,
            "combination_method": self.combination_method,
            "category": self.category,
            "prompt_version": self.prompt_version,
            "total_tokens": self.total_tokens,
        }

    @property
    def summary_reasoning(self) -> str:
        """One-liner combining base rate + bayesian for backward-compatible reasoning field."""
        parts: list[str] = []
        if self.base_rate:
            parts.append(f"Base rate: {self.base_rate[:120]}")
        if self.bayesian_reasoning:
            parts.append(self.bayesian_reasoning[:200])
        if self.critique:
            parts.append(f"Critique: {self.critique[:120]}")
        return " | ".join(parts) or "No reasoning captured"


# ─── Main pipeline ────────────────────────────────────────────────────────────


async def run_reasoning_chain(
    market: GammaMarket,
    cfg: AlphaConfig,
    client: AsyncOpenAI,
    clob: OrderbookSnapshot | None = None,
    price_change_1h: float | None = None,
    news_context: str | None = None,
) -> ReasoningChain | None:
    """Execute the 2-call reasoning chain.

    Returns None only on total API failure. Returns partial chain if Call 2 fails.
    """
    category = detect_category(market.question, market.description or "")
    chain = ReasoningChain(category=category)
    total_tokens = 0

    # ─── Call 1: Deep Analysis ─────────────────────────────────────────
    price_change_str: str | None = None
    if price_change_1h is not None:
        direction = "+" if price_change_1h >= 0 else ""
        price_change_str = f"{direction}{price_change_1h:.2%}"

    prompt1 = get_deep_analysis_prompt(
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

    try:
        resp1 = await client.chat.completions.create(
            model=cfg.openai_model,
            messages=[{"role": "user", "content": prompt1}],
            temperature=0.4,
            max_tokens=800,
            response_format={"type": "json_object"},
        )
        total_tokens += resp1.usage.total_tokens if resp1.usage else 0
    except Exception as e:
        logger.warning("Reasoning Call 1 failed for %s: %s", market.token_id[:16], e)
        return None

    raw1 = resp1.choices[0].message.content or ""
    try:
        data1 = json.loads(raw1)
    except json.JSONDecodeError:
        logger.warning("Call 1 non-JSON for %s: %r", market.token_id[:16], raw1[:200])
        return None

    chain.base_rate = str(data1.get("base_rate", ""))
    chain.evidence_for = _ensure_str_list(data1.get("evidence_for", []))
    chain.evidence_against = _ensure_str_list(data1.get("evidence_against", []))
    chain.bayesian_reasoning = str(data1.get("bayesian_reasoning", ""))
    chain.market_efficiency = str(data1.get("market_efficiency", ""))
    chain.initial_probability = _safe_float(data1.get("probability"), -1)
    chain.initial_confidence = _safe_float(data1.get("confidence"), 0)
    chain.initial_action = str(data1.get("recommended_action", "PASS")).upper()

    if not (0.0 <= chain.initial_probability <= 1.0):
        logger.debug("Call 1 invalid probability %.3f for %s", chain.initial_probability, market.token_id[:16])
        return None

    # ─── Call 2: Devil's Advocate ──────────────────────────────────────
    prompt2 = get_devils_advocate_prompt(
        question=market.question,
        analysis=data1,
        market_price=market.yes_price,
    )

    try:
        resp2 = await client.chat.completions.create(
            model=cfg.openai_model,
            messages=[{"role": "user", "content": prompt2}],
            temperature=0.6,
            max_tokens=400,
            response_format={"type": "json_object"},
        )
        total_tokens += resp2.usage.total_tokens if resp2.usage else 0
    except Exception as e:
        logger.warning("Call 2 failed for %s — using Call 1 only: %s", market.token_id[:16], e)
        chain.final_probability = chain.initial_probability
        chain.final_confidence = chain.initial_confidence
        chain.final_action = chain.initial_action
        chain.combination_method = "call1_only (call2_failed)"
        chain.total_tokens = total_tokens
        return chain

    raw2 = resp2.choices[0].message.content or ""
    try:
        data2 = json.loads(raw2)
    except json.JSONDecodeError:
        logger.warning("Call 2 non-JSON — using Call 1 only: %r", raw2[:200])
        chain.final_probability = chain.initial_probability
        chain.final_confidence = chain.initial_confidence
        chain.final_action = chain.initial_action
        chain.combination_method = "call1_only (call2_parse_error)"
        chain.total_tokens = total_tokens
        return chain

    chain.critique = str(data2.get("critique", ""))
    chain.missed_evidence = _ensure_str_list(data2.get("missed_evidence", []))
    chain.cognitive_biases = _ensure_str_list(data2.get("cognitive_biases", []))
    chain.revised_probability = _safe_float(data2.get("revised_probability"), chain.initial_probability)
    chain.revised_confidence = _safe_float(data2.get("revised_confidence"), chain.initial_confidence)

    if not (0.0 <= chain.revised_probability <= 1.0):
        chain.revised_probability = chain.initial_probability

    # ─── Combine ──────────────────────────────────────────────────────
    disagreement = abs(chain.initial_probability - chain.revised_probability)

    if disagreement > DISAGREEMENT_THRESHOLD:
        # Large disagreement: equal weight + confidence penalty
        chain.final_probability = 0.5 * chain.initial_probability + 0.5 * chain.revised_probability
        chain.final_confidence = min(chain.initial_confidence, chain.revised_confidence) * 0.8
        chain.combination_method = f"equal_weight (disagreement={disagreement:.2f})"
    else:
        chain.final_probability = CALL1_WEIGHT * chain.initial_probability + CALL2_WEIGHT * chain.revised_probability
        chain.final_confidence = CALL1_WEIGHT * chain.initial_confidence + CALL2_WEIGHT * chain.revised_confidence
        chain.combination_method = "weighted_70_30"

    chain.final_probability = max(0.01, min(0.99, chain.final_probability))
    chain.final_confidence = max(0.0, min(1.0, chain.final_confidence))

    # Determine final action from edge
    edge = chain.final_probability - market.yes_price
    if abs(edge) < 0.01:
        chain.final_action = "PASS"
    elif edge > 0:
        chain.final_action = "BUY"
    else:
        chain.final_action = "SELL"

    chain.total_tokens = total_tokens

    logger.info(
        "Chain: %s | call1=%.2f call2=%.2f -> final=%.2f (%s) | cat=%s | %d tok",
        market.question[:40],
        chain.initial_probability,
        chain.revised_probability,
        chain.final_probability,
        chain.combination_method,
        category,
        total_tokens,
    )

    return chain


# ─── Helpers ──────────────────────────────────────────────────────────────────


def _safe_float(val: object, default: float) -> float:
    """Parse a float from LLM output, returning default on failure."""
    try:
        return float(val)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return default


def _ensure_str_list(val: object) -> list[str]:
    """Ensure a list of strings from possibly malformed LLM output."""
    if isinstance(val, list):
        return [str(v) for v in val]
    return []
