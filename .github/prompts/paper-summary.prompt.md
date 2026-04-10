---
mode: agent
description: Pull a live paper trading summary via the agent RPC and format it for decision-making.
---

Query the running Blink engine for a paper trading summary:

```bash
curl -s http://127.0.0.1:7878/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":"1","method":"paper_summary","params":{}}'
```

Also fetch status and alpha status:
```bash
curl -s http://127.0.0.1:7878/rpc \
  -d '{"jsonrpc":"2.0","id":"2","method":"blink_status","params":{}}'

curl -s http://127.0.0.1:7878/rpc \
  -d '{"jsonrpc":"2.0","id":"3","method":"alpha_status","params":{}}'
```

Format the combined output as:
- **NAV / Cash / Invested** — current allocation split
- **Open positions** — token, side, entry price, current price, unrealized P&L, fee category
- **Today's closed trades** — win/loss count, realized P&L, average hold time
- **Risk manager state** — circuit breaker status, daily loss used vs limit, VaR
- **Alpha sidecar** — active/inactive, positions, daily loss
- **Signal queue depth** — pending signals in BinaryHeap
- **Engine health** — WS connection state, last RN1 signal timestamp, poller status

Flag any open positions that are near stop-loss or trailing-stop trigger levels.
