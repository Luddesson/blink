"""RAG news integration for Alpha AI.

Fetches recent news via Tavily (or NewsAPI fallback) to inject real-time
context into LLM prompts. Significantly improves prediction quality for
current-events markets (politics, geopolitics, breaking news).

Usage:
    context = await fetch_news_context("Will Trump win the 2028 election?", cfg)
    # context is a string ready to inject into the LLM prompt
"""

from __future__ import annotations

import logging
from datetime import datetime, timezone

import httpx

logger = logging.getLogger(__name__)

TAVILY_SEARCH_URL = "https://api.tavily.com/search"


async def fetch_news_context(
    question: str,
    tavily_key: str = "",
    max_results: int = 5,
    max_chars: int = 2000,
) -> str | None:
    """Fetch relevant news for a market question.

    Returns a formatted string of news snippets, or None if no key / API fails.
    Uses Tavily API which is optimized for AI agents.
    """
    if not tavily_key:
        return None

    try:
        async with httpx.AsyncClient(timeout=15.0) as client:
            resp = await client.post(
                TAVILY_SEARCH_URL,
                json={
                    "api_key": tavily_key,
                    "query": question,
                    "search_depth": "basic",
                    "max_results": max_results,
                    "include_answer": False,
                    "include_raw_content": False,
                },
            )
            resp.raise_for_status()
            data = resp.json()

        results = data.get("results", [])
        if not results:
            return None

        snippets: list[str] = []
        total_chars = 0

        for item in results:
            title = item.get("title", "")
            content = item.get("content", "")
            url = item.get("url", "")
            published = item.get("published_date", "")

            snippet = f"- [{title}]({url})"
            if published:
                snippet += f" ({published})"
            if content:
                truncated = content[:400].strip()
                snippet += f"\n  {truncated}"

            if total_chars + len(snippet) > max_chars:
                break

            snippets.append(snippet)
            total_chars += len(snippet)

        if not snippets:
            return None

        now_str = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
        header = f"**Recent news context** (fetched {now_str}):\n"
        return header + "\n".join(snippets)

    except Exception as e:
        logger.warning("News fetch failed for %r: %s", question[:60], e)
        return None
