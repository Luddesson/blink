---
mode: agent
description: Analyze the latest Blink paper trading session — P&L, win rate, fee drag, rejection breakdown, and actionable improvements.
---

Read `blink-engine/logs/LATEST_POSTRUN_REVIEW.txt` and `blink-engine/logs/LATEST_SESSION_LOG.txt`.

Produce a structured analysis covering:

1. **Performance summary** — NAV change, realized P&L, win rate, average hold time, largest win/loss
2. **Fee drag** — total fees paid vs gross P&L; which fee category (sports/politics/crypto/geo) dominated
3. **Signal funnel** — how many RN1 signals received → passed all 9 pre-filters → entered priority queue → executed
4. **Top rejection reasons** — from RejectionAnalytics, which filters fired most and why
5. **Exit quality** — which ExitAction types triggered (TakeProfit/StopLoss/TrailingStop/Stagnant/Resolved/MaxHoldExpired); any premature exits?
6. **Risk manager trips** — did circuit breaker, VaR, rate limit, or any other check block orders?
7. **InPlayFailsafe aborts** — how many drift aborts and at what bps threshold?
8. **A/B experiment results** — if ExperimentSwitches were active, compare variant metrics
9. **Concrete improvements** — specific env var tunings or code changes with expected impact

Keep the tone direct. Quantify everything. Flag any anomalies compared to expected behavior.
