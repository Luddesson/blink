# AURA-1 — Lead Systems Architect
## Todo List

- [ ] Provisionera bare-metal server (AWS US-East-1 / Frankfurt)
  - Dedicated bare-metal (ej shared VPS)
  - Min 32-core CPU, 128GB RAM, 2TB NVMe SSD
  - Solarflare NIC med kernel-bypass (OpenOnload)

- [ ] OS-tuning — NUMA pinning, disable HT/C-states/freq scaling
  - NUMA node pinning: CPU + NIC på samma minnesbank
  - BIOS/GRUB: stäng av hyperthreading, C-states, CPU frequency scaling

- [ ] Implementera io_uring + kernel bypass
  - io_uring för asynkron I/O
  - Solarflare NIC med OpenOnload — nätverksdata skippar Linux-kerneln

- [ ] Deploya Reth (Rust Ethereum) full node
  - Reth, inte Erigon — max Rust-synergi
  - State i NVMe-backed memory-mapped files

- [ ] PTP tidssynkronisering med chronyd
  - Precision Time Protocol via chronyd
  - Hardware timestamping NIC
  - Mål: mikrosekund-nivå klockprecision mot Polymarkets matching engine
