# TUI Todo — Advanced Features & Optimization

This todo list is based on a 4-minute runtime observation of the Blink Engine (v0.2). The engine is successfully tracking RN1, maintaining order books via WebSocket, and executing paper trades.

## 1. Interactive Market Management 🛰️
- [x] **Market Switcher:** Add a keyboard shortcut (e.g., `m`) to open a searchable modal of all active Polymarket markets.
- [x] **Dynamic Subscriptions:** Enable toggling market subscriptions on/off without restarting the engine.
- [x] **Focus Mode:** Allow selecting a specific token to see its depth (Order Book) in real-time in a dedicated panel.

## 2. Risk Management UI 🛡️
- [x] **Real-time Limit Editor:** Add a panel to modify `MAX_CONCURRENT_POSITIONS`, `MAX_SINGLE_ORDER_USDC`, and `VAR_THRESHOLD` during runtime.
- [x] **Circuit Breaker Control:** Visual toggle for the global kill switch (`TRADING_ENABLED`) and a "Reset All" button for tripped breakers.
- [x] **Exposure Heatmap:** A visual representation of current exposure per market/category.

## 3. Order Execution Transparency ⚡
- [x] **Failsafe Visualizer:** When an order enters the 3s fill window, show a countdown timer and a live "drift" indicator (current price vs. entry price).
- [x] **Rejection Reasons Panel:** A dedicated small panel that summarizes why the last 10 signals were skipped (e.g., "Size too small: 85%", "Risk blocked: 10%", "Drift abort: 5%").
- [x] **Fill Latency Histogram:** Show a small bar chart of detection-to-fill latency in microseconds.

## 4. Advanced Portfolio Analytics 📊
- [x] **PnL attribution:** Break down PnL by market or asset type.
- [x] **Drawdown Tracker:** Add a "Max Drawdown" stat and a visual line on the sparkline for the high-water mark.
- [x] **Trade History View:** A scrollable table of closed trades with entry/exit prices and duration.

## 5. System Health & eBPF 🔬
- [x] **Latency Alerting:** Flash the border of the Kernel Telemetry panel if syscall latency or TCP RTT exceeds a specific threshold.
- [ ] **Process Monitor:** Show CPU/RAM usage of the engine process within the TUI.

## 7. World-Class Execution Upgrades 🏁
- [x] **Latency SLO Header:** Show p50/p95/p99 for detection→fill and color-alert when p99 breaches target.
- [x] **Adaptive Fill Policy:** Auto-tune `PAPER_FILL_WINDOW_MS`/check interval based on live volatility and drift.
- [x] **Priority Signal Queue:** Prioritize RN1-follow signals by edge score (notional, spread, depth, recency).
- [x] **Pre-trade Liquidity Guard:** Reject or downsize trades when top-of-book depth cannot absorb intended size.
- [x] **Execution Scorecard:** Per-trade slippage, queue delay, and outcome tags to rank strategy quality.
- [x] **Fast Restart Warm State:** Restore WS subscriptions + last book snapshots + portfolio in one shot after restart.
- [x] **Persistent Rejection Analytics:** Save rejection reason stats across sessions and surface 24h trend in TUI.
- [x] **Fault-Tolerant Autosave:** Atomic state writes with backup rotation + checksum validation.
- [x] **Shadow-vs-Live Comparator:** Compare paper fill assumptions against live market prints for realism gap analysis.
- [x] **Strategy Experiment Switches:** Runtime A/B toggles for sizing/autoclaim/drift policies with side-by-side metrics.

## 6. UX & Aesthetic Polish ✨
- [x] **Color Coding:** Use color gradients for the NAV curve (Green -> Yellow -> Red based on performance).
- [x] **Tabs:** Implement a tabbed interface (`[1] Dashboard`, `[2] Markets`, `[3] History`, `[4] Config`).
- [x] **Notification System:** A toast-like notification in the corner for "BIG FILL" or "CRITICAL RISK BREACH".
