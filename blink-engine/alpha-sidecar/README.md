# Alpha Sidecar

AI-driven signal generator for the Blink Engine. Analyses Polymarket markets using GPT and submits trading signals to the engine via JSON-RPC.

## How it works

1. **Discover** — Fetches the 50+ most liquid active markets from the Polymarket Gamma API every N seconds
2. **Filter** — Skips markets with low volume or tiny estimated edge
3. **Analyse** — Sends each candidate to GPT with market context (title, description, current price, volume)
4. **Submit** — If GPT estimates a >5% edge with ≥0.65 confidence, submits an `AlphaSignal` to the Blink engine
5. **Engine decides** — Blink applies its own risk checks (position limits, daily loss cap) before executing

## Setup

### 1. Install dependencies

```bash
cd blink-engine/alpha-sidecar
pip install -e .
```

### 2. Configure environment

Add to your `.env` file (alongside the main Blink engine config):

```env
# Required — Grok (xAI)
XAI_API_KEY=xai-...
ALPHA_ENABLED=true          # Enable sidecar in the engine
ALPHA_TRADING_ENABLED=true  # Allow sidecar signals to trigger trades

# Optional — tune to taste
ALPHA_MODEL=grok-3                 # or grok-3-mini, grok-beta
LLM_BASE_URL=https://api.x.ai/v1  # default, no need to set unless changing provider
ALPHA_DISCOVERY_INTERVAL_SECS=300  # How often to scan (default: 5 min)
ALPHA_MIN_EDGE_BPS=500             # Minimum edge to act (default: 5%)
ALPHA_CONFIDENCE_FLOOR=0.65        # Minimum confidence (0.0–1.0)
ALPHA_MAX_SINGLE_ORDER_USDC=5.0    # Max bet per signal ($)
ALPHA_MAX_CONCURRENT_POSITIONS=3   # Max open AI positions
ALPHA_MAX_LLM_CALLS_PER_CYCLE=20   # Grok calls per discovery cycle
BLINK_RPC_URL=http://127.0.0.1:7878  # Blink engine RPC address

# To switch to OpenAI instead of Grok:
# OPENAI_API_KEY=sk-...
# ALPHA_MODEL=gpt-4-turbo
# LLM_BASE_URL=https://api.openai.com/v1
```

### 3. Start alongside Blink

In a separate terminal:

```bash
cd blink-engine/alpha-sidecar
alpha-sidecar
```

Or:

```bash
python -m alpha_sidecar.main
```

The sidecar connects to the running Blink engine on port 7878. The engine must be running first.

## Architecture

```
alpha-sidecar/
├── alpha_sidecar/
│   ├── config.py          — AlphaConfig loaded from env
│   ├── main.py            — Main loop (entry point)
│   ├── submission.py      — JSON-RPC 2.0 client → Blink engine
│   ├── connectors/
│   │   └── gamma.py       — Polymarket Gamma API client
│   ├── analysis/
│   │   └── llm.py         — LLM market analysis
│   └── rag/               — Optional RAG context store (future)
└── pyproject.toml
```

## Risk controls

The sidecar has its own pre-filter:
- Minimum confidence floor (default 0.65)
- Maximum size per signal (default $5)
- Minimum edge threshold (default 5%)

The Blink engine applies **additional** risk checks:
- `ALPHA_TRADING_ENABLED` kill switch
- `ALPHA_MAX_CONCURRENT_POSITIONS` cap
- `ALPHA_MAX_DAILY_LOSS_PCT` daily loss limit
- Standard circuit breaker

Both layers must pass before a trade executes.

## Monitoring

Signal outcomes are tracked in the engine's `alpha_status` RPC endpoint:
```bash
curl http://127.0.0.1:7878/rpc -d '{"jsonrpc":"2.0","id":"1","method":"alpha_status","params":{}}'
```

Returns: signals received, accepted, rejected (by reason), P&L attribution.
