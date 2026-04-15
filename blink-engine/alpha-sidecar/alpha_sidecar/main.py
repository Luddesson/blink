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
from datetime import datetime, timezone
from pathlib import Path

from dotenv import load_dotenv
from openai import AsyncOpenAI

from .analysis.calibration import calibrate_confidence
from .analysis.llm import analyse_market, compute_edge
from .config import AlphaConfig
from .connectors.clob import get_orderbook, get_price_change_1h
from .connectors.gamma import fetch_active_markets
from .connectors.scanner import score_markets
from .submission import submit_signal

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


async def run_cycle(cfg: AlphaConfig, openai_client: AsyncOpenAI) -> None:
    """One full discovery → enrich → analyse → submit cycle."""
    logger.info("=== Alpha cycle start ===")

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

    logger.info(
        "Analysing up to %d markets (scanner output=%d, limit=%d)",
        min(len(markets), cfg.max_llm_calls_per_cycle),
        len(markets),
        cfg.max_llm_calls_per_cycle,
    )

    submitted = 0
    skipped_edge = 0
    skipped_llm = 0
    clob_enriched = 0

    for market in markets[: cfg.max_llm_calls_per_cycle]:
        if _shutdown:
            break

        # Step 3: Enrich with CLOB data (orderbook + spread + price history)
        clob, price_change_1h = await _enrich_with_clob(market, cfg)
        if clob is not None:
            clob_enriched += 1
            logger.debug(
                "CLOB: %s spread=%.0fbps bid=$%.0f ask=$%.0f",
                market.question[:40],
                clob.spread_pct * 10_000,
                clob.bid_depth_usdc,
                clob.ask_depth_usdc,
            )

        # Step 4: LLM analysis with full context
        llm_signal = await analyse_market(
            market, cfg, openai_client, clob=clob, price_change_1h=price_change_1h
        )
        if llm_signal is None:
            skipped_llm += 1
            continue

        # Step 5: Calibrate confidence based on spread
        spread_pct = clob.spread_pct if clob else None
        calibrated_confidence = calibrate_confidence(llm_signal.confidence, spread_pct)
        if calibrated_confidence != llm_signal.confidence:
            logger.debug(
                "Confidence calibrated: %.2f → %.2f (spread=%.0fbps)",
                llm_signal.confidence,
                calibrated_confidence,
                (spread_pct or 0) * 10_000,
            )
            llm_signal.confidence = calibrated_confidence

        # Step 6: Edge check (after calibration)
        edge_bps = compute_edge(llm_signal)
        if edge_bps < cfg.min_edge_bps:
            skipped_edge += 1
            logger.debug(
                "Edge too small for %s: %.0fbps < %dbps",
                market.question[:50], edge_bps, cfg.min_edge_bps,
            )
            continue

        # Step 7: Submit with Kelly sizing
        result = await submit_signal(llm_signal, cfg)
        if result.success:
            submitted += 1

        # Small delay between LLM calls to avoid rate limiting
        await asyncio.sleep(0.5)

    logger.info(
        "=== Cycle done: %d submitted, %d skipped (low edge), "
        "%d skipped (LLM PASS), %d/%d CLOB-enriched ===",
        submitted, skipped_edge, skipped_llm,
        clob_enriched, min(len(markets), cfg.max_llm_calls_per_cycle),
    )


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
        "| min_edge=%dbps | vol=$%.0f–$%.0f | rpc=%s",
        cfg.openai_model,
        cfg.clob_api_url,
        cfg.discovery_interval_secs,
        cfg.min_edge_bps,
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
        # Sleep in 1-second chunks so shutdown is responsive
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
