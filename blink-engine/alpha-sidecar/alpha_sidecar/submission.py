"""JSON-RPC 2.0 client for submitting AlphaSignals to the Blink engine."""

from __future__ import annotations

import logging
import uuid
from dataclasses import asdict, dataclass

import httpx

from .analysis.llm import LLMSignal
from .config import AlphaConfig
from .connectors.clob import compute_expected_value

logger = logging.getLogger(__name__)

# Quarter-Kelly multiplier — conservative bet sizing given model uncertainty.
KELLY_FRACTION: float = 0.25


@dataclass
class SubmitResult:
    success: bool
    analysis_id: str
    error: str | None = None


def _compute_size(llm: LLMSignal, cfg: AlphaConfig) -> float:
    """Compute order size using Kelly fraction when CLOB data is available.

    Falls back to cfg.max_recommended_size_usdc / 2 when CLOB is unavailable.
    Kelly is capped at max_recommended_size_usdc and floored at $1.
    """
    if llm.clob is not None:
        ev = compute_expected_value(llm.yes_probability, llm.market.yes_price)
        kelly_raw = (
            ev.kelly_fraction_yes
            if llm.recommended_action == "BUY"
            else ev.kelly_fraction_no
        )
        # Quarter-Kelly × bankroll estimate ($100 virtual start)
        bankroll_estimate = 100.0
        kelly_size = kelly_raw * bankroll_estimate * KELLY_FRACTION
        size = max(1.0, min(cfg.max_recommended_size_usdc, kelly_size))
        logger.debug(
            "Kelly sizing: kelly_raw=%.3f → size=$%.2f (cap=$%.2f)",
            kelly_raw,
            size,
            cfg.max_recommended_size_usdc,
        )
        return round(size, 2)

    # Fallback: half the configured max (conservative default)
    return round(min(cfg.max_recommended_size_usdc, cfg.max_recommended_size_usdc * 0.5), 2)


async def submit_signal(llm: LLMSignal, cfg: AlphaConfig) -> SubmitResult:
    """Submit an LLM signal to the Blink engine via JSON-RPC 2.0.

    The engine's `submit_alpha_signal` method validates the signal against
    AlphaRiskConfig before it ever reaches the order pipeline.
    """
    side = "BUY" if llm.recommended_action == "BUY" else "SELL"
    if llm.recommended_action == "BUY":
        price = llm.market.yes_price * 1.005  # cross the spread slightly
    else:
        price = llm.market.yes_price * 0.995

    price = round(max(0.01, min(0.99, price)), 4)
    size = _compute_size(llm, cfg)

    # Compute EV for logging / engine metadata
    ev = compute_expected_value(llm.yes_probability, llm.market.yes_price)
    ev_bps = round(
        (ev.ev_yes if llm.recommended_action == "BUY" else ev.ev_no) * 10_000, 1
    )

    payload = {
        "jsonrpc": "2.0",
        "id": str(uuid.uuid4()),
        "method": "submit_alpha_signal",
        "params": {
            "token_id": llm.market.token_id,
            "condition_id": llm.market.condition_id,
            "side": side,
            "confidence": round(llm.confidence, 4),
            "recommended_price": price,
            "recommended_size_usdc": size,
            "reasoning": llm.reasoning[:500],
            "source": {
                "type": "AiAutonomous",
                "model": cfg.openai_model,
                "prompt_id": llm.analysis_id,
            },
            "analysis_id": llm.analysis_id,
        },
    }

    async with httpx.AsyncClient(timeout=5.0) as client:
        try:
            resp = await client.post(
                cfg.blink_rpc_url + "/rpc",
                json=payload,
                headers={"Content-Type": "application/json"},
            )
            resp.raise_for_status()
            data = resp.json()
        except httpx.HTTPError as e:
            logger.warning("RPC HTTP error submitting signal %s: %s", llm.analysis_id, e)
            return SubmitResult(success=False, analysis_id=llm.analysis_id, error=str(e))
        except Exception as e:
            logger.warning("RPC unexpected error: %s", e)
            return SubmitResult(success=False, analysis_id=llm.analysis_id, error=str(e))

    if "error" in data:
        err_msg = data["error"].get("message", "unknown RPC error")
        logger.info("Engine rejected signal %s: %s", llm.analysis_id, err_msg)
        return SubmitResult(success=False, analysis_id=llm.analysis_id, error=err_msg)

    spread_info = (
        f" spread={llm.clob.spread_pct * 10_000:.0f}bps" if llm.clob else ""
    )
    logger.info(
        "✓ Signal submitted: %s %s @ %.4f (conf=%.2f edge=%+.0fbps ev=%+.0fbps size=$%.2f%s)",
        side,
        llm.market.question[:50],
        price,
        llm.confidence,
        (llm.yes_probability - llm.market.yes_price) * 10_000,
        ev_bps,
        size,
        spread_info,
    )
    return SubmitResult(success=True, analysis_id=llm.analysis_id)
