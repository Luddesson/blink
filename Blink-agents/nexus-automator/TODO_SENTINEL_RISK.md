# SENTINEL — Chief Risk Officer & Security Engineer
## Todo List

- [ ] Foundry fuzz testing + Halmos/Certora formell verifiering
  - Foundry (Forge/Cast) invariant fuzz testing
  - Halmos eller Certora: formellt verifiera att Solidity execution contracts ej kan tömmas

- [ ] TEE secure enclave för privata nycklar
  - AWS Nitro Enclaves eller Intel SGX
  - Rust-engine begär signaturer från enclaven
  - Nycklar får aldrig ligga i system RAM

- [ ] 3-sekunders in-play delay failsafe
  - Pinga /price endpoint var 100ms under 3s countdown
  - Om implied probability skiftar >1.5%: omedelbar CancelOrder WebSocket frame

- [ ] Dynamic VaR — rullande 60s exposure + circuit breaker
  - Rullande 60-sekunders Value at Risk
  - Om utestående limit orders > 5% av portfolio NAV: disconnecta tokio WebSocket streams

- [ ] Pre-game order wipe — flush cache vid match-start
  - Detektera starting whistle (CLOB rensar alla utestående orders)
  - Omedelbart flusha intern order-cache
