# BLINK — MASTER TODO & EXECUTION SCHEDULE

> **Project:** Blink HFT Shadow-Maker Bot for Polymarket CLOB  
> **Current Phase:** 4/5 (85% complete) — Paper Trading LIVE  
> **Last Updated:** 2026-04-05  
> **Engine Status:** Running paper mode, NAV ~$252 USDC

---

## LEGEND

- `[ ]` Not started  
- `[~]` In progress  
- `[x]` Complete  
- `[!]` Blocked / Needs input  
- **P0** = Must-have for production  
- **P1** = Should-have (significant value)  
- **P2** = Nice-to-have (future iteration)  
- **Agent:** Owner responsible for execution  

---

## PHASE 1: CORE ENGINE HARDENING (P0)

> Goal: Make the engine production-safe on Linux bare-metal

### 1.1 io_uring Network Backend — Agent: AURA
- [x] **P0** Implement `IoUringNet::connect()` using `tokio_uring::net::TcpStream` (`io_uring_net.rs:163`)
- [x] **P0** Implement `IoUringNet::read()` with registered buffer reads (`io_uring_net.rs:168`)
- [x] **P0** Implement `IoUringNet::write_all()` with registered buffer writes (`io_uring_net.rs:173`)
- [x] **P0** Add `tokio-uring` to workspace dependencies (`Cargo.toml`)
- [ ] **P0** Integration test: WebSocket roundtrip over io_uring on Linux
- [ ] **P0** Benchmark: io_uring vs Tokio latency (target: p99 < 500µs)
- [ ] **P1** Zero-copy buffer pool for io_uring reads (avoid allocation per recv)

### 1.2 Order Execution Hardening — Agent: QSIGMA
- [x] **P0** Validate FOK (Fill-or-Kill) order type — unit tests pass (order_body_fok_sets_order_type)
- [x] **P0** Validate FAK (Fill-and-Kill) order type — unit tests pass (order_body_fak_sets_order_type)
- [ ] **P0** Stress test: 100 concurrent order submissions (measure reject rate)
- [ ] **P1** Add order deduplication (idempotency key per order)
- [ ] **P1** Implement order amendment (cancel + replace in single API call)
- [ ] **P2** Implement iceberg order splitting for large positions

### 1.3 Truth Reconciliation — Agent: QSIGMA
- [ ] **P0** Verify `truth_reconciler.rs` handles partial fills correctly
- [ ] **P0** Test reconciliation after WebSocket reconnect (gap detection)
- [ ] **P1** Add reconciliation metrics to ClickHouse (drift events, corrections)
- [ ] **P2** Implement automatic position correction on drift > threshold

### 1.4 Paper Trading Validation — Agent: SENTINEL
- [x] Paper engine running (NAV $252.27)
- [x] RN1 signal detection active
- [x] Order fills simulated
- [ ] **P0** Run 7-day continuous paper trading session without crashes
- [ ] **P0** Validate P&L calculation matches manual audit (±0.01%)
- [ ] **P0** Stress test: inject 10x RN1 signal volume, verify no deadlocks
- [ ] **P1** Compare paper fills vs actual market fills (slippage analysis)

---

## PHASE 2: RISK & SECURITY AUDIT (P0)

> Goal: Ensure no capital loss scenarios in live trading

### 2.1 Risk Manager — Agent: SENTINEL
- [x] Position caps implemented
- [x] Loss limits active
- [x] VaR circuit breaker (60s rolling window)
- [x] In-play failsafe (3-second delay)
- [x] Game start order wipe
- [x] **P0** Fuzz test risk_manager.rs with proptest (10,000 iterations) — 4 properties, all pass
- [ ] **P0** Verify circuit breaker triggers correctly under extreme volatility
- [ ] **P0** Test max drawdown kill switch ($50 USDC threshold)
- [ ] **P1** Add per-market exposure limits (not just portfolio-level)
- [ ] **P1** Implement time-based position decay (force-close after N hours)
- [ ] **P2** Dynamic position sizing based on volatility regime

### 2.2 Key Security — Agent: SENTINEL
- [x] TEE vault implemented (tee-vault crate)
- [x] Zeroization on drop
- [x] Platform-specific memory locking (Windows VirtualLock / POSIX mlock)
- [x] **P0** Audit: verify no private key leaks in logs — PASS (Debug redacts, no logging of secrets)
- [ ] **P0** Rotate signing key mechanism (hot swap without restart)
- [ ] **P1** Add key backup encryption (AES-256-GCM envelope)
- [ ] **P2** HSM integration path (YubiHSM2 or AWS CloudHSM)

### 2.3 MEV Protection — Agent: WRAITH
- [x] MEV router framework (`mev_router.rs`)
- [x] MEV shield policies (`mev_shield.rs`)
- [x] EIP-1559 gas strategy (`gas_strategy.rs`)
- [ ] **P0** Test Flashbots bundle submission on Polygon
- [ ] **P0** Validate sandwich detection heuristics with historical data
- [ ] **P1** Integrate Titan relay as fallback
- [ ] **P1** Integrate bloXroute as secondary relay
- [ ] **P2** MEV-Share integration (capture own MEV)
- [ ] **P2** Implement order timing randomization (anti-pattern detection)

### 2.4 Formal Verification — Agent: SENTINEL
- [x] proptest harnesses written
- [x] Halmos symbolic execution setup
- [x] Kani bounded model checking
- [x] **P0** Run full proptest suite (10,000 iterations × 7 properties) — all pass (137 engine + 7 vault tests)
- [ ] **P0** Run Halmos on OrderSignerProperties.sol
- [ ] **P0** Run Halmos on RiskManagerProperties.sol
- [ ] **P1** Add Kani integer overflow checks for all arithmetic in risk_manager.rs
- [ ] **P2** Formal proof: "no order submitted without risk check" (invariant)

---

## PHASE 3: DATA PIPELINE & OBSERVABILITY (P1)

> Goal: Production-grade logging, metrics, and data warehouse

### 3.1 ClickHouse Data Warehouse — Agent: NEXUS
- [x] ClickHouse logger implemented (`clickhouse_logger.rs`)
- [x] **P0** Define ClickHouse schema: 6 tables (order_book_snapshots, rn1_signals, trade_executions, system_metrics, risk_events, latency_samples)
- [ ] **P0** Deploy ClickHouse instance (local Docker for dev, dedicated for prod)
- [ ] **P0** Verify logger writes are non-blocking (no engine latency impact)
- [ ] **P1** Set retention policies (raw: 90d, aggregated: 1y)
- [ ] **P1** Build materialized views for: P&L per market, signal hit rate, latency percentiles
- [ ] **P2** Grafana dashboard connected to ClickHouse
- [ ] **P2** Alerting: P&L drops > 5%, latency spikes > 1ms, signal drought > 1h

### 3.2 eBPF Telemetry — Agent: NEXUS
- [x] bpf-probes crate scaffolded (feature-gated)
- [ ] **P1** Implement network latency probe (measure kernel → userspace time)
- [ ] **P1** Implement syscall counter (track hot paths)
- [ ] **P1** Export eBPF metrics to ClickHouse
- [ ] **P2** Context switch monitoring (detect scheduler interference)
- [ ] **P2** Cache miss profiling (perf event integration)

### 3.3 Latency Tracking Improvements — Agent: NEXUS
- [x] Microsecond latency tracker (`latency_tracker.rs`)
- [x] Heartbeat system (`heartbeat.rs`)
- [x] **P1** Add latency histograms (p50, p95, p99, p999) + histogram buckets + /api/latency endpoint
- [x] **P1** Export latency to ClickHouse (latency_samples table added)
- [ ] **P1** Alert on latency regression (p99 > 500µs triggers warning)
- [ ] **P2** Flamegraph integration for hot-path profiling

### 3.4 Backtesting Pipeline — Agent: QSIGMA
- [x] Tick recorder (`tick_recorder.rs`)
- [x] Backtest engine (`backtest_engine.rs`)
- [ ] **P1** Record 30 days of tick data to CSV
- [ ] **P1** Backtest: replay RN1 signals with current strategy, compute Sharpe ratio
- [ ] **P1** Compare paper trading results vs backtest predictions
- [ ] **P2** Parameter sweep: optimize shadow offset, position size, timing
- [ ] **P2** Walk-forward optimization framework

---

## PHASE 4: INFRASTRUCTURE & DEPLOYMENT (P0)

> Goal: Deploy to bare-metal Linux for production trading

### 4.1 Server Provisioning — Agent: AURA
- [x] provision.sh complete (177 lines)
- [x] systemd service unit ready
- [x] chrony PTP config ready
- [x] OS tuning script (os_tune.sh)
- [ ] **P0** Provision AWS c7i.metal-24xl (or equivalent) in US-East-1
- [ ] **P0** Run provision.sh on target server
- [ ] **P0** Run os_tune.sh (NUMA pinning, C-state disable, freq scaling off)
- [ ] **P0** Verify NVMe mount point for Reth data
- [ ] **P0** Validate systemd service starts and auto-restarts

### 4.2 Reth Full Node — Agent: AURA
- [x] reth_config.toml prepared
- [ ] **P0** Deploy Reth on production server
- [ ] **P0** Sync Polygon mainnet (estimate: 2-4 days)
- [ ] **P0** Validate block sync latency (target: < 200ms from tip)
- [ ] **P1** Configure Reth JSON-RPC for local engine access
- [ ] **P1** Set up state pruning (keep latest 256 blocks for gas oracle)
- [ ] **P2** Redundant Reth node (failover)

### 4.3 Network Optimization — Agent: AURA
- [ ] **P0** Measure network latency to Polymarket CLOB endpoints
- [ ] **P0** Configure TCP tuning (Nagle off, SO_KEEPALIVE, buffer sizes)
- [ ] **P1** Set up dedicated NIC for trading traffic (isolate from monitoring)
- [ ] **P1** DNS pinning for Polymarket endpoints (avoid DNS lookup latency)
- [ ] **P2** Explore colocation options near Polymarket infra

### 4.4 CI/CD Pipeline — Agent: NEXUS
- [x] **P1** GitHub Actions: cargo build + cargo test on push
- [x] **P1** GitHub Actions: cargo clippy + cargo fmt --check
- [x] **P1** GitHub Actions: web-ui npm run build + npm run lint
- [x] **P1** Pre-commit hook: secret scanning (TruffleHog + pre-commit hook)
- [ ] **P2** Automated deployment to staging on merge to master
- [ ] **P2** Canary deployment (new version on 10% traffic, auto-rollback)

---

## PHASE 5: WEB UI & MONITORING (P1)

> Goal: Real-time monitoring dashboard for live operations

### 5.1 React Dashboard Enhancements — Agent: AURA
- [x] React 19 + Vite + Tailwind + Recharts
- [x] TypeScript build clean
- [x] Proxy to Axum backend (localhost:3030)
- [ ] **P1** Real-time P&L chart (WebSocket stream from engine)
- [ ] **P1** Order book visualization (bid/ask depth chart)
- [ ] **P1** Active positions table with live mark-to-market
- [ ] **P1** RN1 signal feed (real-time signal stream)
- [ ] **P1** Risk dashboard (exposure, VaR, circuit breaker status)
- [ ] **P1** Latency metrics panel (p50/p95/p99 charts)
- [ ] **P2** Trade history table with pagination
- [ ] **P2** Market selector dropdown
- [ ] **P2** Manual order entry form (admin override)
- [ ] **P2** Dark mode / light mode toggle

### 5.2 Tauri Desktop App — Agent: AURA
- [ ] **P2** Initialize Tauri project in `blink-ui/`
- [ ] **P2** Embed web-ui as Tauri WebView
- [ ] **P2** System tray icon with status indicator
- [ ] **P2** Native notifications for: fills, circuit breaker, errors
- [ ] **P2** Auto-update mechanism (Tauri updater)

---

## PHASE 6: AGENT SYSTEM & SKILLS (P2)

> Goal: Autonomous agent capabilities beyond manual operation

### 6.1 Agent Infrastructure — Agent: NEXUS
- [x] Agent RPC endpoint (`agent_rpc.rs` on localhost:3031)
- [ ] **P1** Define agent RPC protocol (JSON-RPC 2.0 spec)
- [ ] **P1** Authentication for agent RPC (API key or mTLS)
- [ ] **P2** Agent heartbeat monitoring
- [ ] **P2** Agent coordination protocol (prevent conflicting actions)

### 6.2 Market Sentiment Skill — Agent: QSIGMA
- [x] SKILL.md documented
- [x] Sentiment examples reference file
- [ ] **P1** Implement `sentiment_analyzer.py` script
- [ ] **P1** Curate RSS feed list (`references/rss_feeds.md`)
- [ ] **P1** Integrate sentiment score into shadow-maker signal weighting
- [ ] **P2** Add social media sources (Twitter/X, Telegram, Discord)
- [ ] **P2** ML-based sentiment model (fine-tuned on crypto headlines)

### 6.3 Trading Strategist Skill — Agent: QSIGMA
- [x] SKILL.md documented
- [ ] **P1** Implement `fetch_binance.py` (klines + ticker data)
- [ ] **P1** Implement `calculate_ta.py` (SMA, RSI, MACD, Bollinger, Stochastic)
- [ ] **P1** Create `references/ta_formulas.md` (indicator math reference)
- [ ] **P2** Combine TA signals with RN1 shadow-maker for hybrid strategy
- [ ] **P2** Bayesian signal combiner (weight multiple strategies by confidence)

### 6.4 EVM Swiss Knife Skill — Agent: WRAITH
- [x] SKILL.md complete (196 lines)
- [x] Foundry `cast` usage documented
- [ ] **P1** Verify Foundry installed on production server
- [ ] **P1** Add helper scripts for common operations (check balance, approve token)
- [ ] **P2** Batch transaction builder (multi-call patterns)

---

## PHASE 7: ADVANCED FEATURES (P2)

> Goal: Competitive edge through technology

### 7.1 RL Gas Prediction Model — Agent: NEXUS
- [ ] **P2** Collect gas price training data (30 days, 1-second granularity)
- [ ] **P2** Train RL model (PPO or SAC) for gas price prediction
- [ ] **P2** Integrate model output into `gas_strategy.rs`
- [ ] **P2** A/B test: RL gas vs current heuristic gas strategy

### 7.2 Multi-Market Expansion — Agent: QSIGMA
- [ ] **P2** Extend sniffer to track multiple RN1-class wallets
- [ ] **P2** Cross-market correlation detection (correlated outcomes)
- [ ] **P2** Portfolio optimization across N markets (mean-variance)
- [ ] **P2** Market regime detection (high/low volatility adaptation)

### 7.3 Advanced Order Types — Agent: QSIGMA
- [ ] **P2** TWAP (Time-Weighted Average Price) execution
- [ ] **P2** VWAP (Volume-Weighted Average Price) execution
- [ ] **P2** Adaptive aggression (increase urgency near game start)

### 7.4 Disaster Recovery — Agent: AURA
- [ ] **P1** Snapshot engine state to disk every 60s (`blink_twin.rs` extends)
- [ ] **P1** Cold restart from snapshot (verify NAV matches)
- [ ] **P1** Runbook: manual position close procedure
- [ ] **P2** Multi-region failover (secondary server in Frankfurt)
- [ ] **P2** Automated failover detection + switchover

---

## EXECUTION SCHEDULE

### Sprint 1 (Week 1-2): Production Foundation
| Task | Phase | Priority | Agent | Est. |
|------|-------|----------|-------|------|
| io_uring implementation | 1.1 | P0 | AURA | 3d |
| FOK/FAK validation | 1.2 | P0 | QSIGMA | 1d |
| 7-day paper run | 1.4 | P0 | SENTINEL | 7d |
| Risk fuzz testing | 2.1 | P0 | SENTINEL | 2d |
| Key leak audit | 2.2 | P0 | SENTINEL | 1d |
| Formal verification run | 2.4 | P0 | SENTINEL | 1d |

### Sprint 2 (Week 3-4): Infrastructure
| Task | Phase | Priority | Agent | Est. |
|------|-------|----------|-------|------|
| Server provisioning | 4.1 | P0 | AURA | 1d |
| Reth deployment + sync | 4.2 | P0 | AURA | 4d |
| Network optimization | 4.3 | P0 | AURA | 2d |
| ClickHouse schema + deploy | 3.1 | P0 | NEXUS | 2d |
| Flashbots integration test | 2.3 | P0 | WRAITH | 2d |
| CI/CD pipeline | 4.4 | P1 | NEXUS | 2d |

### Sprint 3 (Week 5-6): Dashboard & Observability
| Task | Phase | Priority | Agent | Est. |
|------|-------|----------|-------|------|
| Real-time P&L chart | 5.1 | P1 | AURA | 2d |
| Order book viz | 5.1 | P1 | AURA | 2d |
| Risk dashboard | 5.1 | P1 | AURA | 1d |
| Latency histograms | 3.3 | P1 | NEXUS | 1d |
| eBPF probes | 3.2 | P1 | NEXUS | 3d |
| Backtest 30d replay | 3.4 | P1 | QSIGMA | 2d |

### Sprint 4 (Week 7-8): Agent Skills & Polish
| Task | Phase | Priority | Agent | Est. |
|------|-------|----------|-------|------|
| Sentiment analyzer impl | 6.2 | P1 | QSIGMA | 2d |
| TA calculator impl | 6.3 | P1 | QSIGMA | 2d |
| Agent RPC protocol | 6.1 | P1 | NEXUS | 2d |
| Titan/bloXroute relays | 2.3 | P1 | WRAITH | 2d |
| Disaster recovery | 7.4 | P1 | AURA | 2d |
| Snapshot + cold restart | 7.4 | P1 | AURA | 1d |

### Sprint 5+ (Week 9+): Advanced & Optimization
- RL gas model (P2)
- Multi-market expansion (P2)
- Tauri desktop app (P2)
- Advanced order types (P2)
- Multi-region failover (P2)
- ML sentiment model (P2)

---

## GO-LIVE CHECKLIST

> Must all be [x] before switching from paper to live trading

- [ ] io_uring backend working on Linux (or confirmed Tokio meets latency SLA)
- [ ] 7-day paper run: zero crashes, P&L within 1% of expected
- [ ] All proptest + Halmos + Kani formal verification passing
- [ ] Risk manager fuzz tested (10,000 proptest iterations)
- [ ] No private key material in logs (audit complete)
- [ ] Flashbots bundle submission validated on Polygon
- [ ] Reth synced and block latency < 200ms from tip
- [ ] ClickHouse logging active (all events persisted)
- [ ] Server provisioned with os_tune.sh applied
- [ ] systemd service auto-restarts on crash
- [ ] Circuit breaker tested: triggers at $50 drawdown
- [ ] Manual kill switch verified (Ctrl+C → graceful shutdown, all orders cancelled)
- [ ] Backup signing key prepared (encrypted, offline)
- [ ] Runbook reviewed: "what to do if engine goes down during live session"
- [ ] Fund trading wallet with initial USDC allocation

---

## METRICS & SUCCESS CRITERIA

### Paper Trading Targets (current phase)
- **Uptime:** > 99.5% over 7 days
- **P&L accuracy:** Paper vs manual audit within ±0.01%
- **Signal latency:** RN1 detect → order submit < 50ms
- **WebSocket reconnect:** < 2 seconds after disconnect

### Production Targets (after go-live)
- **Order-to-fill:** p99 < 500µs (with io_uring)
- **Daily P&L:** Positive expectation (Sharpe > 1.5 target)
- **Max drawdown:** < $50 USDC per session
- **Uptime:** > 99.9% during market hours
- **Data completeness:** 100% of trades logged to ClickHouse

---

*This TODO is the single source of truth for Blink development priorities.*  
*Update this file when tasks complete or priorities shift.*
