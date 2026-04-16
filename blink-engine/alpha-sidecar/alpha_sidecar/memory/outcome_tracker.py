"""Background outcome tracker for Alpha AI predictions.

Periodically checks unresolved predictions against the Gamma API to
detect market resolution. When a market resolves, it computes the
Brier score and marks the prediction as resolved.

Runs as an asyncio background task inside the sidecar event loop.
"""

from __future__ import annotations

import asyncio
import logging

import httpx

from .prediction_store import PredictionStore

logger = logging.getLogger(__name__)

GAMMA_MARKET_URL = "https://gamma-api.polymarket.com/markets"
CHECK_INTERVAL_SECS = 120


async def check_resolutions(
    store: PredictionStore,
    gamma_url: str = GAMMA_MARKET_URL,
    engine_url: str = "http://127.0.0.1:7878",
) -> int:
    """Check unresolved predictions against Gamma API. Returns count of newly resolved."""
    unresolved = await store.get_unresolved(limit=50)
    if not unresolved:
        return 0

    by_condition: dict[str, list[dict]] = {}
    for pred in unresolved:
        cid = pred["condition_id"]
        by_condition.setdefault(cid, []).append(pred)

    resolved_count = 0

    async with httpx.AsyncClient(timeout=10.0) as client:
        for condition_id, preds in by_condition.items():
            try:
                outcome = await _fetch_resolution(client, gamma_url, condition_id)
            except Exception as e:
                logger.debug("Resolution check failed for %s: %s", condition_id[:16], e)
                for p in preds:
                    await store.bump_next_check(p["analysis_id"])
                continue

            if outcome is None:
                for p in preds:
                    await store.bump_next_check(p["analysis_id"])
                continue

            actual = 1.0 if outcome == "YES" else 0.0

            for pred in preds:
                predicted = pred.get("predicted_prob")
                if predicted is None:
                    await store.mark_resolved(
                        pred["analysis_id"],
                        actual_outcome=actual,
                        brier_score=0.0,
                        was_correct=False,
                        pnl_usdc=None,
                    )
                    resolved_count += 1
                    continue

                brier = (predicted - actual) ** 2

                side = pred.get("side") or pred.get("model_action")
                if side == "BUY":
                    was_correct = actual == 1.0
                elif side == "SELL":
                    was_correct = actual == 0.0
                else:
                    was_correct = False

                pnl = await _fetch_pnl(client, engine_url, pred["analysis_id"])

                await store.mark_resolved(
                    pred["analysis_id"],
                    actual_outcome=actual,
                    brier_score=round(brier, 6),
                    was_correct=was_correct,
                    pnl_usdc=pnl,
                )
                resolved_count += 1

                logger.info(
                    "Resolved: %s | predicted=%.2f actual=%.0f brier=%.4f correct=%s pnl=%s",
                    pred["question"][:50],
                    predicted,
                    actual,
                    brier,
                    was_correct,
                    f"${pnl:.2f}" if pnl is not None else "n/a",
                )

            await asyncio.sleep(0.3)

    return resolved_count


async def _fetch_resolution(
    client: httpx.AsyncClient,
    gamma_url: str,
    condition_id: str,
) -> str | None:
    """Check if a market has resolved. Returns 'YES' or 'NO' or None."""
    resp = await client.get(gamma_url, params={"condition_id": condition_id})
    resp.raise_for_status()
    markets = resp.json()

    if not markets:
        return None

    m = markets[0] if isinstance(markets, list) else markets

    if m.get("closed") and m.get("resolved"):
        winner = m.get("winner") or m.get("outcome")
        if winner:
            return str(winner).upper()

        prices = m.get("outcomePrices") or []
        if len(prices) >= 2:
            try:
                yes_price = float(prices[0])
                if yes_price >= 0.99:
                    return "YES"
                elif yes_price <= 0.01:
                    return "NO"
            except (ValueError, TypeError):
                pass

        tokens = m.get("tokens") or []
        for tok in tokens:
            outcome_str = str(tok.get("outcome", "")).upper()
            price = float(tok.get("price", 0))
            if price >= 0.99 and outcome_str in ("YES", "1"):
                return "YES"
            if price >= 0.99 and outcome_str in ("NO", "0"):
                return "NO"

    return None


async def _fetch_pnl(
    client: httpx.AsyncClient,
    engine_url: str,
    analysis_id: str,
) -> float | None:
    """Best-effort P&L lookup from engine's API."""
    try:
        resp = await client.get(f"{engine_url}/api/alpha", timeout=3.0)
        if resp.status_code != 200:
            return None
        data = resp.json()
        for trade in data.get("ai_closed_trades", []):
            if trade.get("analysis_id") == analysis_id:
                return trade.get("pnl_usdc") or trade.get("pnl")
    except Exception:
        pass
    return None


async def run_outcome_tracker(
    store: PredictionStore,
    gamma_url: str = GAMMA_MARKET_URL,
    engine_url: str = "http://127.0.0.1:7878",
    shutdown_event: asyncio.Event | None = None,
) -> None:
    """Background loop that checks for resolved markets."""
    logger.info("Outcome tracker started (interval=%ds)", CHECK_INTERVAL_SECS)

    while True:
        if shutdown_event and shutdown_event.is_set():
            break

        try:
            n = await check_resolutions(store, gamma_url, engine_url)
            if n > 0:
                logger.info("Outcome tracker resolved %d predictions", n)
        except Exception:
            logger.exception("Outcome tracker error — will retry next interval")

        for _ in range(CHECK_INTERVAL_SECS):
            if shutdown_event and shutdown_event.is_set():
                break
            await asyncio.sleep(1)

    logger.info("Outcome tracker stopped.")
