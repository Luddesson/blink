# NEXUS — Data Engineering & AI Telemetry Lead
## Todo List

- [ ] ClickHouse tick-level data warehouse
  - Streama Polymarket orderbok-snapshots, competitor trades, interna system-loggar
  - Lokal ClickHouse columnar databas för high-speed backtesting

- [ ] eBPF kernel telemetry probes
  - eBPF-probes i Linux-kerneln
  - Traca mikrosekunder per paket: nätverksstack vs Rust-applikation
  - Alert vid latency >500us

- [ ] RL-modell for gas price prediction
  - Lightweight reinforcement learning på historiska Polygon block base-fees
  - Predicta required gas price för nästa block innan det mintas

- [ ] CI/CD pipeline — GitHub Actions + Foundry + Reth testnet
  - Automatisk Rust-kompilering
  - Foundry fuzz tests
  - Dry-run mot lokal Reth testnet fork innan deploy till live server
