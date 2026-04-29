# P1-Node Runbook — Local Polygon full node for mempool + logs

**Todo**: `p1-node`. **Status**: requires provisioning + ~1 TB disk + 24–48 h sync.

## Choice: erigon vs bor

- **bor** (`maticnetwork/bor`) is the canonical Polygon PoS client. Use for correctness.
- **erigon** (`ledgerwatch/erigon`) is faster to sync and smaller on disk, but its Polygon
  support lags bor on hard-forks. Only use if you accept that operational risk.

**Recommendation**: bor, because the mempool feed completeness matters more than disk
footprint, and bor is first-party.

## Resources

- 16 vCPU, 32 GB RAM, 2 TB NVMe (io2 or gp3 w/ 10k IOPS).
- Colocated on the same box as the engine? **Yes** — the point is sub-ms
  IPC/localhost latency for `newPendingTransactions`. Use cgroups to cap node to 8 vCPU
  so it doesn't starve the engine's isolated cores.

## Configuration highlights

```toml
# bor config.toml (excerpts)
[p2p]
  maxpeers = 200              # mempool completeness matters
  bootnodes = [...]            # Polygon official bootnodes

[txpool]
  nolocals = false
  pricelimit = 30000000000
  accountslots = 1024
  globalslots = 65536          # larger pool = fewer mempool misses
  globalqueue = 65536

[rpc]
  ipcpath = "/run/bor/bor.ipc"  # prefer IPC over WS for engine-local access
  ws = true
  wsaddr = "127.0.0.1"
  wsport = 8546
  wsorigins = ["*"]
  wsmodules = ["eth","net","web3","txpool"]
```

## Peering diversity (R-8 mitigation)

A single node's view of the mempool is incomplete. Mitigations, in increasing cost:

1. Raise `maxpeers` to 200+ (free).
2. Add static bootnodes of known high-connectivity nodes.
3. Subscribe to a paid mempool relayer (bloXroute Polygon, Merkle.io, Blocknative) in
   parallel; merge streams with dedup on tx hash. Budget this against expected alpha
   lift — see Phase 5.

## Validation

- `blink-probe mempool --ws ws://127.0.0.1:8546 --duration 600` for 10 min should show
  > 1000 txs/s seen.
- Run two probes in parallel against the local node and a public RPC (or two local
  nodes). The operator-visible "coverage" field in the report must be ≥ 95 % agreement
  before declaring `p2-ingress` ready for shadow.

## Ops

- systemd unit `/etc/systemd/system/bor.service` with `Restart=on-failure`.
- Prometheus scrape on `:7071`.
- Snapshot backup nightly to S3 (sync time is 24–48 h; don't redo if avoidable).
