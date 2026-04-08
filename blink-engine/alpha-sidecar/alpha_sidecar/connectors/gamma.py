"""Polymarket Gamma API connector.

Fetches active markets and filters them by liquidity and urgency.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Any

import httpx

logger = logging.getLogger(__name__)

GAMMA_MARKETS_URL = "https://gamma-api.polymarket.com/markets"


@dataclass
class GammaMarket:
    token_id: str          # CLOB token ID (YES outcome)
    condition_id: str
    question: str
    description: str
    slug: str
    yes_price: float       # Current YES mid price (0.0–1.0)
    no_price: float
    volume_usdc: float
    end_date_iso: str      # ISO 8601
    active: bool
    closed: bool
    extra: dict = field(default_factory=dict)  # raw fields for LLM context


async def fetch_active_markets(
    gamma_url: str,
    min_volume: float = 500.0,
    limit: int = 50,
) -> list[GammaMarket]:
    """Fetch liquid active markets from the Gamma API."""
    params = {
        "active": "true",
        "closed": "false",
        "limit": limit,
        "order": "volumeNum",
        "ascending": "false",
    }
    async with httpx.AsyncClient(timeout=10.0) as client:
        try:
            resp = await client.get(gamma_url, params=params)
            resp.raise_for_status()
            raw: list[dict[str, Any]] = resp.json()
        except httpx.HTTPError as e:
            logger.error("Gamma API HTTP error: %s", e)
            return []
        except Exception as e:
            logger.error("Gamma API unexpected error: %s", e)
            return []

    markets: list[GammaMarket] = []
    for m in raw:
        try:
            vol = float(m.get("volumeNum") or m.get("volume") or 0)
            if vol < min_volume:
                continue

            # Extract YES/NO prices from tokens array or top-level
            yes_price, no_price = _extract_prices(m)
            if yes_price is None or yes_price <= 0 or yes_price >= 1:
                continue

            token_id = _get_yes_token_id(m)
            if not token_id:
                continue

            end_date = (
                m.get("endDateIso")
                or m.get("end_date_iso")
                or m.get("endDate")
                or ""
            )

            markets.append(GammaMarket(
                token_id=token_id,
                condition_id=m.get("conditionId") or m.get("condition_id") or "",
                question=m.get("question") or m.get("title") or "",
                description=m.get("description") or "",
                slug=m.get("market_slug") or m.get("slug") or "",
                yes_price=yes_price,
                no_price=no_price,
                volume_usdc=vol,
                end_date_iso=end_date,
                active=bool(m.get("active")),
                closed=bool(m.get("closed")),
                extra={k: v for k, v in m.items() if k in (
                    "category", "tags", "liquidity", "spread"
                )},
            ))
        except Exception as e:
            logger.debug("Skipping market parse error: %s", e)
            continue

    logger.info("Gamma: fetched %d qualifying markets (vol > $%.0f)", len(markets), min_volume)
    return markets


def _extract_prices(m: dict) -> tuple[float | None, float | None]:
    """Try multiple field locations for YES/NO prices."""
    # Attempt 1: outcomePrices array ["0.65", "0.35"]
    prices = m.get("outcomePrices") or m.get("outcome_prices")
    if prices and len(prices) >= 2:
        try:
            return float(prices[0]), float(prices[1])
        except (ValueError, TypeError):
            pass

    # Attempt 2: tokens[].price
    tokens = m.get("tokens") or []
    if len(tokens) >= 2:
        try:
            yes_p = float(tokens[0].get("price") or tokens[0].get("mid_price") or 0)
            no_p = float(tokens[1].get("price") or tokens[1].get("mid_price") or 0)
            if 0 < yes_p < 1:
                return yes_p, no_p
        except (ValueError, TypeError):
            pass

    # Attempt 3: bestAsk/bestBid approximation
    try:
        ba = float(m.get("bestAsk") or 0)
        bb = float(m.get("bestBid") or 0)
        if 0 < ba < 1 and bb > 0:
            mid = (ba + bb) / 2
            return mid, 1.0 - mid
    except (ValueError, TypeError):
        pass

    return None, None


def _get_yes_token_id(m: dict) -> str | None:
    """Extract the YES outcome token ID."""
    tokens = m.get("tokens") or []
    for tok in tokens:
        outcome = str(tok.get("outcome") or "").upper()
        if outcome in ("YES", "1", "TRUE"):
            return str(tok.get("token_id") or tok.get("tokenId") or "")
    # Fallback: first token
    if tokens:
        return str(tokens[0].get("token_id") or tokens[0].get("tokenId") or "")
    # Last resort: top-level clobTokenIds
    ids = m.get("clobTokenIds") or m.get("clob_token_ids") or []
    if ids:
        return str(ids[0])
    return None
