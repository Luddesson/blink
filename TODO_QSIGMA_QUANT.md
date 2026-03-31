# Q-SIGMA — Lead Quantitative Engineer
## Todo List

- [ ] Scaffolda Rust-projektet med jemalloc + crossbeam
  - jemalloc som global allocator
  - crossbeam ring buffers för lock-free data-passing mellan WebSocket-tråd och exekveringstråd

- [ ] WebSocket-anslutning till Polymarket CLOB
  - Persistent multi-threaded WebSocket (tokio runtime)
  - wss://ws-live-data.polymarket.com
  - wss://sports-api.polymarket.com/ws

- [ ] simd-json parser för CLOB-deltas
  - simd-json med CPU vector instructions
  - Maximal throughput på orderbok-uppdateringar

- [ ] Lokal orderbok med L1-cache-aligned BTreeMap
  - In-memory BTreeMap alignad mot L1 CPU-cache
  - Beräkna spreads i nanosekunder utan API-anrop

- [ ] RN1 Sniffer — wallet-filtrering + Shadow Maker-algoritm
  - Filtrera WebSocket-feed för RN1:s wallet-adress
  - Vid detekterad EIP-712 order: Optimal_Size = Target_Size * Liquidity_Multiplier * Volatility_Adjustment
  - Generera Post-Only limit order adjacent till RN1:s position

- [ ] EIP-712 signing med alloy-rs
  - Lokal EIP-712 transaktionssignering via alloy-rs
  - Snabb EVM-encoding för ordersignering

- [ ] FOK/FAK ordertyper för sports-marknader
  - Fill-Or-Kill: exakt pris eller kill
  - Fill-And-Kill: micro-slippage tolerance, fyll det som finns, killa resten
  - Aldrig GTC i sport-meta
