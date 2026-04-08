"""Configuration loaded from environment variables."""

from __future__ import annotations

import os
from dataclasses import dataclass, field


@dataclass
class AlphaConfig:
    # Blink engine RPC
    blink_rpc_url: str = "http://127.0.0.1:7878"

    # OpenAI
    openai_api_key: str = ""
    openai_model: str = "gpt-4-turbo"

    # Discovery
    gamma_api_url: str = "https://gamma-api.polymarket.com"
    discovery_interval_secs: int = 300
    min_edge_bps: int = 500  # 5% minimum edge to generate signal
    max_llm_calls_per_cycle: int = 20

    # RAG
    chroma_persist_dir: str = "./data/chroma"
    embedding_model: str = "text-embedding-3-small"

    # Risk (sidecar-side pre-filter, engine has final say)
    confidence_floor: float = 0.65
    max_recommended_size_usdc: float = 5.0

    # Optional connectors
    newsapi_key: str = ""
    tavily_key: str = ""

    @classmethod
    def from_env(cls) -> AlphaConfig:
        return cls(
            blink_rpc_url=os.getenv("BLINK_RPC_URL", cls.blink_rpc_url),
            openai_api_key=os.getenv("OPENAI_API_KEY", ""),
            openai_model=os.getenv("OPENAI_MODEL", cls.openai_model),
            gamma_api_url=os.getenv("GAMMA_API_URL", cls.gamma_api_url),
            discovery_interval_secs=int(os.getenv("ALPHA_DISCOVERY_INTERVAL_SECS", str(cls.discovery_interval_secs))),
            min_edge_bps=int(os.getenv("ALPHA_MIN_EDGE_BPS", str(cls.min_edge_bps))),
            max_llm_calls_per_cycle=int(os.getenv("ALPHA_MAX_LLM_CALLS_PER_CYCLE", str(cls.max_llm_calls_per_cycle))),
            chroma_persist_dir=os.getenv("ALPHA_CHROMA_DIR", cls.chroma_persist_dir),
            confidence_floor=float(os.getenv("ALPHA_CONFIDENCE_FLOOR", str(cls.confidence_floor))),
            max_recommended_size_usdc=float(os.getenv("ALPHA_MAX_SINGLE_ORDER_USDC", str(cls.max_recommended_size_usdc))),
            newsapi_key=os.getenv("NEWSAPI_KEY", ""),
            tavily_key=os.getenv("TAVILY_API_KEY", ""),
        )
