"""Polymarket CLOB API connector.

**Latency class: COLD PATH.** Called once per market per cycle — never from
the hot signal path. Provides orderbook snapshot, spread, price history,
expected value (EV), and Kelly fraction for position sizing.

Ported and extended from artvandelay/polymarket-agents clob.py.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass

import httpx

logger = logging.getLogger(__name__)

CLOB_API_URL = "https://clob.polymarket.com"


@dataclass
class OrderbookSnapshot:
    """Live orderbook summary for a single YES token."""

    token_id: str
    best_bid: float
    best_ask: float
    spread: float          # best_ask - best_bid  (absolute)
    spread_pct: float      # spread / mid_price   (fraction, e.g. 0.02 = 2%)
    bid_depth_usdc: float  # top-5 bids notional in USDC
    ask_depth_usdc: float  # top-5 asks notional in USDC
    mid_price: float


@dataclass
class EvResult:
    """Expected value and Kelly fraction for a market position."""

    ev_yes: float               # EV for buying YES
    ev_no: float                # EV for buying NO
    kelly_fraction_yes: float   # Kelly fraction (0.0–1.0)
    kelly_fraction_no: float
    recommendation: str         # "BUY", "SELL", or "PASS"


async def get_orderbook(
    token_id: str,
    clob_url: str = CLOB_API_URL,
) -> OrderbookSnapshot | None:
    """Fetch live orderbook and compute spread + depth.

    Returns None on timeout, HTTP error, or illiquid book (no bids/asks).
    """
    params = {"token_id": token_id}

    async with httpx.AsyncClient(timeout=8.0) as client:
        try:
            resp = await client.get(f"{clob_url}/book", params=params)
            resp.raise_for_status()
            data = resp.json()
        except httpx.HTTPError as e:
            logger.debug("CLOB orderbook HTTP error %s: %s", token_id[:16], e)
            return None
        except Exception as e:
            logger.debug("CLOB orderbook error %s: %s", token_id[:16], e)
            return None

    bids: list[dict] = data.get("bids") or []
    asks: list[dict] = data.get("asks") or []

    if not bids or not asks:
        return None

    try:
        sorted_bids = sorted(bids, key=lambda x: float(x["price"]), reverse=True)
        sorted_asks = sorted(asks, key=lambda x: float(x["price"]))

        best_bid = float(sorted_bids[0]["price"])
        best_ask = float(sorted_asks[0]["price"])

        if best_bid <= 0 or best_ask <= 0 or best_ask <= best_bid:
            return None

        spread = best_ask - best_bid
        mid_price = (best_bid + best_ask) / 2
        spread_pct = spread / mid_price if mid_price > 0 else 0.0

        # Notional depth: price × size summed over top-5 levels
        bid_depth = sum(
            float(b["price"]) * float(b["size"]) for b in sorted_bids[:5]
        )
        ask_depth = sum(
            float(a["price"]) * float(a["size"]) for a in sorted_asks[:5]
        )

        return OrderbookSnapshot(
            token_id=token_id,
            best_bid=best_bid,
            best_ask=best_ask,
            spread=spread,
            spread_pct=spread_pct,
            bid_depth_usdc=bid_depth,
            ask_depth_usdc=ask_depth,
            mid_price=mid_price,
        )

    except (ValueError, KeyError, IndexError) as e:
        logger.debug("CLOB parse error %s: %s", token_id[:16], e)
        return None


async def get_price_change_1h(
    token_id: str,
    clob_url: str = CLOB_API_URL,
) -> float | None:
    """Return the 1-hour price change as a fraction (e.g. +0.05 = +5%).

    Returns None if insufficient history or on error.
    """
    params = {
        "market": token_id,
        "interval": "1h",
        "fidelity": 60,  # 60-second candles
    }

    async with httpx.AsyncClient(timeout=8.0) as client:
        try:
            resp = await client.get(f"{clob_url}/prices-history", params=params)
            resp.raise_for_status()
            data = resp.json()
        except Exception as e:
            logger.debug("CLOB price history error %s: %s", token_id[:16], e)
            return None

    history: list[dict] = data.get("history") or []
    if len(history) < 2:
        return None

    try:
        oldest = float(history[0]["p"])
        newest = float(history[-1]["p"])
        if oldest <= 0:
            return None
        return (newest - oldest) / oldest
    except (ValueError, KeyError, IndexError):
        return None


def compute_expected_value(
    prob_estimate: float,
    market_price: float,
) -> EvResult:
    """Compute EV and Kelly fraction given an LLM probability estimate.

    Formula (for YES at price P with estimated probability p):
      ev_yes = p * (1 - P) - (1 - p) * P
      kelly_yes = ev_yes / (1 - P)   [Kelly fraction, capped at 1.0]

    Kelly fraction is quarter-Kelly in practice — apply a 0.25 multiplier
    externally (in submission.py) to account for model uncertainty.
    """
    p = max(0.0, min(1.0, prob_estimate))
    price = max(0.01, min(0.99, market_price))

    ev_yes = p * (1.0 - price) - (1.0 - p) * price
    ev_no = (1.0 - p) * price - p * (1.0 - price)  # symmetric

    kelly_yes = max(0.0, min(1.0, ev_yes / (1.0 - price))) if ev_yes > 0 else 0.0
    kelly_no = max(0.0, min(1.0, ev_no / price)) if ev_no > 0 else 0.0

    if ev_yes >= ev_no and ev_yes > 0:
        recommendation = "BUY"
    elif ev_no > ev_yes and ev_no > 0:
        recommendation = "SELL"
    else:
        recommendation = "PASS"

    return EvResult(
        ev_yes=ev_yes,
        ev_no=ev_no,
        kelly_fraction_yes=kelly_yes,
        kelly_fraction_no=kelly_no,
        recommendation=recommendation,
    )
