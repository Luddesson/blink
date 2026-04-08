"""JSON-RPC 2.0 client for submitting AlphaSignals to the Blink engine."""

from __future__ import annotations

import logging
import uuid
from dataclasses import asdict, dataclass

import httpx

from .analysis.llm import LLMSignal
from .config import AlphaConfig

logger = logging.getLogger(__name__)


@dataclass
class SubmitResult:
    success: bool
    analysis_id: str
    error: str | None = None


async def submit_signal(llm: LLMSignal, cfg: AlphaConfig) -> SubmitResult:
    """Submit an LLM signal to the Blink engine via JSON-RPC 2.0.

    The engine's `submit_alpha_signal` method validates the signal against
    AlphaRiskConfig before it ever reaches the order pipeline.
    """
    side = "Buy" if llm.recommended_action == "BUY" else "Sell"
    if llm.recommended_action == "BUY":
        price = llm.market.yes_price * 1.005  # cross the spread slightly
    else:
        price = llm.market.yes_price * 0.995

    price = round(max(0.01, min(0.99, price)), 4)
    size = min(cfg.max_recommended_size_usdc, 5.0)

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

    logger.info(
        "✓ Signal submitted: %s %s @ %.4f (conf=%.2f edge=%+.0fbps)",
        side, llm.market.question[:50], price, llm.confidence,
        (llm.yes_probability - llm.market.yes_price) * 10_000,
    )
    return SubmitResult(success=True, analysis_id=llm.analysis_id)
