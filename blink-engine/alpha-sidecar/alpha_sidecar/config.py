"""Configuration loaded from environment variables."""

from __future__ import annotations

import os
from dataclasses import dataclass, field


@dataclass
class AlphaConfig:
    # Blink engine RPC
    blink_rpc_url: str = "http://127.0.0.1:7878"

    # LLM — defaults to Grok (xAI), OpenAI-compatible API
    llm_api_key: str = ""
    llm_base_url: str = "https://api.x.ai/v1"
    openai_model: str = "grok-3"

    # Discovery
    gamma_api_url: str = "https://gamma-api.polymarket.com"
    clob_api_url: str = "https://clob.polymarket.com"
    discovery_interval_secs: int = 300
    min_edge_bps: int = 500  # 5% minimum edge to generate signal
    max_llm_calls_per_cycle: int = 20

    # Scanner: volume sweet-spot for alpha opportunities
    scanner_min_volume_usdc: float = 5_000.0
    scanner_max_volume_usdc: float = 500_000.0

    # RAG
    chroma_persist_dir: str = "./data/chroma"
    embedding_model: str = "text-embedding-3-small"

    # Risk (sidecar-side pre-filter, engine has final say)
    confidence_floor: float = 0.65
    max_recommended_size_usdc: float = 25.0

    # Optional connectors
    newsapi_key: str = ""
    tavily_key: str = ""

    @classmethod
    def from_env(cls) -> AlphaConfig:
        # Accept XAI_API_KEY (Grok) or OPENAI_API_KEY as fallback
        api_key = (
            os.getenv("XAI_API_KEY")
            or os.getenv("OPENAI_API_KEY")
            or ""
        )
        return cls(
            blink_rpc_url=os.getenv("BLINK_RPC_URL", cls.blink_rpc_url),
            llm_api_key=api_key,
            llm_base_url=os.getenv("LLM_BASE_URL", cls.llm_base_url),
            openai_model=os.getenv("ALPHA_MODEL", os.getenv("OPENAI_MODEL", cls.openai_model)),
            gamma_api_url=os.getenv("GAMMA_API_URL", cls.gamma_api_url),
            clob_api_url=os.getenv("CLOB_API_URL", cls.clob_api_url),
            discovery_interval_secs=int(os.getenv("ALPHA_DISCOVERY_INTERVAL_SECS", str(cls.discovery_interval_secs))),
            min_edge_bps=int(os.getenv("ALPHA_MIN_EDGE_BPS", str(cls.min_edge_bps))),
            max_llm_calls_per_cycle=int(os.getenv("ALPHA_MAX_LLM_CALLS_PER_CYCLE", str(cls.max_llm_calls_per_cycle))),
            scanner_min_volume_usdc=float(os.getenv("ALPHA_SCANNER_MIN_VOL", str(cls.scanner_min_volume_usdc))),
            scanner_max_volume_usdc=float(os.getenv("ALPHA_SCANNER_MAX_VOL", str(cls.scanner_max_volume_usdc))),
            chroma_persist_dir=os.getenv("ALPHA_CHROMA_DIR", cls.chroma_persist_dir),
            confidence_floor=float(os.getenv("ALPHA_CONFIDENCE_FLOOR", str(cls.confidence_floor))),
            max_recommended_size_usdc=float(os.getenv("ALPHA_MAX_SINGLE_ORDER_USDC", str(cls.max_recommended_size_usdc))),
            newsapi_key=os.getenv("NEWSAPI_KEY", ""),
            tavily_key=os.getenv("TAVILY_API_KEY", ""),
        )
