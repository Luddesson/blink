"""Alpha Sidecar — AI-driven signal generator for the Blink Engine.

Main loop:
  1. Fetch active Polymarket markets from the Gamma API
  2. Filter by volume, end date, and minimum edge threshold
  3. Send each candidate to GPT for probability estimation
  4. Submit high-confidence signals to the Blink engine via JSON-RPC

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

from dotenv import load_dotenv
from openai import AsyncOpenAI

from .analysis.llm import analyse_market, compute_edge
from .config import AlphaConfig
from .connectors.gamma import fetch_active_markets
from .submission import submit_signal

load_dotenv()

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


async def run_cycle(cfg: AlphaConfig, openai_client: AsyncOpenAI) -> None:
    """One full discovery → analyse → submit cycle."""
    logger.info("=== Alpha cycle start ===")

    markets = await fetch_active_markets(
        gamma_url=cfg.gamma_api_url,
        min_volume=500.0,
        limit=100,
    )

    if not markets:
        logger.warning("No markets returned from Gamma API")
        return

    now_iso = datetime.now(timezone.utc).isoformat()
    logger.info("Analysing up to %d markets (limit=%d)", len(markets), cfg.max_llm_calls_per_cycle)

    submitted = 0
    skipped_edge = 0
    skipped_llm = 0

    for market in markets[: cfg.max_llm_calls_per_cycle]:
        if _shutdown:
            break

        llm_signal = await analyse_market(market, cfg, openai_client)
        if llm_signal is None:
            skipped_llm += 1
            continue

        edge_bps = compute_edge(llm_signal)
        if edge_bps < cfg.min_edge_bps:
            skipped_edge += 1
            logger.debug(
                "Edge too small for %s: %.0fbps < %dbps",
                market.question[:50], edge_bps, cfg.min_edge_bps,
            )
            continue

        result = await submit_signal(llm_signal, cfg)
        if result.success:
            submitted += 1

        # Small delay between LLM calls to avoid rate limiting
        await asyncio.sleep(0.5)

    logger.info(
        "=== Cycle done: %d submitted, %d skipped (low edge), %d skipped (LLM PASS) ===",
        submitted, skipped_edge, skipped_llm,
    )


async def main_loop(cfg: AlphaConfig) -> None:
    if not cfg.llm_api_key:
        logger.error("XAI_API_KEY (or OPENAI_API_KEY) is not set — alpha sidecar cannot start")
        sys.exit(1)

    openai_client = AsyncOpenAI(api_key=cfg.llm_api_key, base_url=cfg.llm_base_url)
    logger.info(
        "Alpha sidecar starting | model=%s | base_url=%s | interval=%ds | min_edge=%dbps | rpc=%s",
        cfg.openai_model,
        cfg.llm_base_url,
        cfg.discovery_interval_secs,
        cfg.min_edge_bps,
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
