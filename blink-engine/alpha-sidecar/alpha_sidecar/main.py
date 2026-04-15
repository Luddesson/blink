"""Alpha Sidecar — AI-driven signal generator for the Blink Engine.

Main loop:
  1. Fetch active Polymarket markets from the Gamma API
  2. Filter + sort by inefficiency score (scanner.py)
  3. Enrich each candidate with live CLOB data (orderbook, spread, price Δ)
  4. Send each candidate to Grok/GPT for probability estimation
  5. Calibrate confidence based on spread (calibration.py)
  6. Apply Kelly fraction sizing (submission.py)
  7. Submit high-confidence signals to the Blink engine via JSON-RPC

Usage:
    alpha-sidecar                 # uses env vars for config
    python -m alpha_sidecar.main  # same
"""

from __future__ import annotations

import asyncio
import logging
import signal
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

import httpx
from dotenv import load_dotenv
from openai import AsyncOpenAI

from .analysis.calibration import calibrate_confidence
from .analysis.llm import LLMSignal, analyse_market, compute_edge
from .config import AlphaConfig
from .connectors.clob import OrderbookSnapshot, get_orderbook, get_price_change_1h
from .connectors.gamma import GammaMarket, fetch_active_markets
from .connectors.scanner import score_markets
from .submission import submit_signal

def _compute_size_for_report(llm: LLMSignal, cfg: AlphaConfig) -> float | None:
    """Compute approximate order size for reporting (same as submission.py logic)."""
    try:
        from .submission import _compute_size
        return _compute_size(llm, cfg)
    except Exception:
        return None

# Load .env — search current dir, then blink-engine/, then repo root
def _load_env() -> None:
    candidates = [
        Path.cwd() / ".env",
        Path(__file__).resolve().parents[2] / ".env",   # blink-engine/.env
        Path(__file__).resolve().parents[3] / ".env",   # repo root/.env
    ]
    for p in candidates:
        if p.exists():
            load_dotenv(p, override=False)

_load_env()

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    datefmt="%H:%M:%S",
)
logger = logging.getLogger("alpha_sidecar")

_shutdown = False


def _handle_shutdown(sig: int, _frame: object) -> None:
    global _shutdown
    logger.info("Signal %d received — shutting down gracefully", sig)
    _shutdown = True


async def _enrich_with_clob(market, cfg: AlphaConfig):
    """Fetch CLOB orderbook + price history for a market. Non-blocking on failure."""
    try:
        clob = await get_orderbook(market.token_id, clob_url=cfg.clob_api_url)
        price_change = await get_price_change_1h(market.token_id, clob_url=cfg.clob_api_url)
        return clob, price_change
    except Exception as e:
        logger.debug("CLOB enrichment failed for %s: %s", market.token_id[:16], e)
        return None, None


async def _report_cycle_to_engine(cfg: AlphaConfig, report: dict) -> None:
    """Report cycle stats to the Blink engine via JSON-RPC."""
    payload = {
        "jsonrpc": "2.0",
        "id": "cycle-report",
        "method": "report_alpha_cycle",
        "params": report,
    }
    async with httpx.AsyncClient(timeout=5.0) as client:
        try:
            resp = await client.post(
                cfg.blink_rpc_url + "/rpc",
                json=payload,
                headers={"Content-Type": "application/json"},
            )
            resp.raise_for_status()
            logger.debug("Cycle report sent to engine")
        except Exception as e:
            logger.debug("Failed to report cycle to engine: %s", e)


async def run_cycle(cfg: AlphaConfig, openai_client: AsyncOpenAI) -> None:
    """One full discovery → enrich → analyse → submit cycle."""
    logger.info("=== Alpha cycle start ===")
    cycle_start = time.monotonic()

    # Step 1: Discover markets from Gamma API (broad set)
    raw_markets = await fetch_active_markets(
        gamma_url=cfg.gamma_api_url,
        min_volume=cfg.scanner_min_volume_usdc,
        limit=100,
    )

    if not raw_markets:
        logger.warning("No markets returned from Gamma API")
        return

    # Step 2: Filter and sort by inefficiency score
    markets = score_markets(
        raw_markets,
        min_volume=cfg.scanner_min_volume_usdc,
        max_volume=cfg.scanner_max_volume_usdc,
    )

    if not markets:
        logger.warning("Scanner filtered all markets — check volume thresholds")
        return

    n_to_analyze = min(len(markets), cfg.max_llm_calls_per_cycle)
    logger.info(
        "Analysing up to %d markets (scanner output=%d, limit=%d)",
        n_to_analyze, len(markets), cfg.max_llm_calls_per_cycle,
    )

    submitted = 0
    skipped_edge = 0
    skipped_llm = 0
    clob_enriched = 0
    top_markets: list[dict] = []

    def _build_market_entry(
        market: GammaMarket,
        llm_signal: LLMSignal | None,
        clob: OrderbookSnapshot | None,
        price_change_1h: float | None,
        edge_bps: float | None,
        action: str,
        *,
        size_usdc: float | None = None,
    ) -> dict:
        """Build enriched market entry for cycle report."""
        entry: dict = {
            "question": market.question[:100],
            "yes_price": round(market.yes_price, 4),
            "llm_probability": round(llm_signal.yes_probability, 4) if llm_signal else None,
            "confidence": round(llm_signal.confidence, 3) if llm_signal else None,
            "edge_bps": round(edge_bps, 1) if edge_bps is not None else None,
            "action": action,
            "reasoning": (llm_signal.reasoning[:300] if llm_signal else None),
            "spread_pct": round(clob.spread_pct, 4) if clob else None,
            "bid_depth_usdc": round(clob.bid_depth_usdc, 2) if clob else None,
            "ask_depth_usdc": round(clob.ask_depth_usdc, 2) if clob else None,
            "price_change_1h": round(price_change_1h, 4) if price_change_1h is not None else None,
            "side": llm_signal.recommended_action if llm_signal else None,
            "token_id": market.token_id,
            "recommended_size_usdc": round(size_usdc, 2) if size_usdc is not None else None,
        }
        return entry

    for market in markets[:n_to_analyze]:
        if _shutdown:
            break

        # Step 3: Enrich with CLOB data (orderbook + spread + price history)
        clob, price_change_1h = await _enrich_with_clob(market, cfg)
        if clob is not None:
            clob_enriched += 1

        # Step 4: LLM analysis with full context
        llm_signal = await analyse_market(
            market, cfg, openai_client, clob=clob, price_change_1h=price_change_1h
        )

        if llm_signal is None:
            skipped_llm += 1
            top_markets.append(_build_market_entry(
                market, None, clob, price_change_1h, None, "PASS",
            ))
            continue

        # Step 5: Calibrate confidence based on spread
        spread_pct = clob.spread_pct if clob else None
        calibrated_confidence = calibrate_confidence(llm_signal.confidence, spread_pct)
        if calibrated_confidence != llm_signal.confidence:
            llm_signal.confidence = calibrated_confidence

        # Step 6: Edge check (after calibration)
        edge_bps = compute_edge(llm_signal)

        if edge_bps < cfg.min_edge_bps:
            skipped_edge += 1
            top_markets.append(_build_market_entry(
                market, llm_signal, clob, price_change_1h, edge_bps, "LOW_EDGE",
            ))
            continue

        # Step 7: Submit with Kelly sizing
        result = await submit_signal(llm_signal, cfg)
        action = "SUBMITTED" if result.success else "REJECTED"
        if result.success:
            submitted += 1

        top_markets.append(_build_market_entry(
            market, llm_signal, clob, price_change_1h, edge_bps, action,
            size_usdc=_compute_size_for_report(llm_signal, cfg),
        ))

        await asyncio.sleep(0.5)

    cycle_duration = time.monotonic() - cycle_start

    logger.info(
        "=== Cycle done: %d submitted, %d skipped (low edge), "
        "%d skipped (LLM PASS), %d/%d CLOB-enriched (%.1fs) ===",
        submitted, skipped_edge, skipped_llm,
        clob_enriched, n_to_analyze, cycle_duration,
    )

    # Report cycle stats to engine
    await _report_cycle_to_engine(cfg, {
        "markets_scanned": len(raw_markets),
        "markets_analyzed": n_to_analyze,
        "signals_generated": submitted + skipped_edge,
        "signals_submitted": submitted,
        "cycle_duration_secs": round(cycle_duration, 2),
        "top_markets": top_markets,
    })


async def main_loop(cfg: AlphaConfig) -> None:
    if not cfg.llm_api_key:
        logger.error(
            "XAI_API_KEY is not set. Add it to blink-engine/.env:\n"
            "  XAI_API_KEY=xai-...\n"
            "Searched: %s",
            ", ".join(str(p) for p in [
                Path.cwd() / ".env",
                Path(__file__).resolve().parents[2] / ".env",
                Path(__file__).resolve().parents[3] / ".env",
            ])
        )
        sys.exit(1)

    openai_client = AsyncOpenAI(api_key=cfg.llm_api_key, base_url=cfg.llm_base_url)
    logger.info(
        "Alpha sidecar starting | model=%s | clob=%s | interval=%ds "
        "| min_edge=%dbps | conf_floor=%.2f | vol=$%.0f–$%.0f | rpc=%s",
        cfg.openai_model,
        cfg.clob_api_url,
        cfg.discovery_interval_secs,
        cfg.min_edge_bps,
        cfg.confidence_floor,
        cfg.scanner_min_volume_usdc,
        cfg.scanner_max_volume_usdc,
        cfg.blink_rpc_url,
    )

    while not _shutdown:
        try:
            await run_cycle(cfg, openai_client)
        except Exception:
            logger.exception("Unhandled error in cycle — continuing after backoff")
            await asyncio.sleep(30)
            continue

        logger.info("Next cycle in %ds", cfg.discovery_interval_secs)
        for _ in range(cfg.discovery_interval_secs):
            if _shutdown:
                break
            await asyncio.sleep(1)

    logger.info("Alpha sidecar stopped.")


def main() -> None:
    """Entry point for `alpha-sidecar` CLI command."""
    signal.signal(signal.SIGINT, _handle_shutdown)
    signal.signal(signal.SIGTERM, _handle_shutdown)

    cfg = AlphaConfig.from_env()
    asyncio.run(main_loop(cfg))


if __name__ == "__main__":
    main()
