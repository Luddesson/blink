---
applyTo:
  - blink-engine/alpha-sidecar/**
description: Python conventions for the Blink alpha sidecar (AI signal generator).
---

## General
- Python 3.12+. Use `from __future__ import annotations` for forward refs.
- Full type annotations on all public functions and classes. Run `mypy` before committing.
- Use `ruff` for linting and formatting (`ruff check . && ruff format .`).

## Engine communication
- The sidecar connects to the engine at `BLINK_RPC_URL` (default `http://127.0.0.1:7878`).
- All communication is JSON-RPC 2.0 via the `agent_rpc` module — use the `submit_alpha_signal` method.
- The engine must be running before the sidecar starts. Always handle `ConnectionRefusedError` gracefully.
- Required `AlphaSignal` fields: `token_id`, `side` (`"YES"`/`"NO"`), `confidence` (0.0–1.0), `edge_bps` (int), `source` (`"AiAutonomous"`).

## Signal quality gates (sidecar-side — engine has its own independent layer)
- Only submit signals with `confidence >= ALPHA_CONFIDENCE_FLOOR` (default 0.65).
- Only submit signals with `edge_bps >= ALPHA_MIN_EDGE_BPS` (default 500 = 5%).
- Justify every signal with a `reasoning` field — used for post-session review.

## LLM calls
- Default: Grok-3 via xAI API (`XAI_API_KEY`). Switchable via `LLM_BASE_URL` + `OPENAI_API_KEY`.
- Always set a `timeout` on LLM calls (default 30s). Never block the signal loop indefinitely.
- Log token usage per call with `structlog` — track cost drift over sessions.

## Configuration
- All config from environment variables with safe defaults. Use `python-dotenv` to load `.env`.
- Never hardcode API keys, endpoints, or token IDs in source.

## Error handling
- Use `structlog` for structured logging — never bare `print()`.
- Catch and log LLM errors without crashing the main loop. The sidecar should self-heal.
