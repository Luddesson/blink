# WRAITH — MEV & Mempool Evasion Specialist
## Todo List

- [ ] Flashbots/Titan/bloXroute eth_sendBundle integration
  - Routa alla signerade bundles via eth_sendBundle RPC direkt till block builders
  - Säkra WebSocket-anslutningar
  - Helt bypass av publikt P2P gossip-nätverk

- [ ] EIP-1559 dynamisk priority fee-kalkylering
  - Optimal_Priority_Fee = (Expected_Trade_Profit * 0.10) / Gas_Limit
  - Baserat på föregående blocks base fee + mempool congestion

- [ ] MEV-Share + sandwich evasion
  - Privat transaction payload med hints via MEV-Share
  - Smart contracts med strikta block.number / block.timestamp deadlines
  - Invalidera trades vid builder-fördröjning
