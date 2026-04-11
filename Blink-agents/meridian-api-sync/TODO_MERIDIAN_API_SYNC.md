# MERIDIAN - API Integrity, Synchronization & Visual Truth Chief
## Todo List

- [ ] Canonical API state store
  - En enda sanningsmodell for markets, orders, fills, positions och UI snapshots
  - Inga komponenter far lasa ostrukturerade REST/WS payloads direkt

- [ ] Visual truth contract for web UI
  - Account, NAV, PnL, uptime, open positions, fills och health-status maste komma fran tydliga canonical sources
  - Varje UI-falt ska ha freshness/staleness-status och tydlig fallback, aldrig fake "looks good" state

- [ ] WS + REST reconciliation engine
  - Market WS for live deltas, user WS for order truth, REST for bootstrap/recovery
  - Snapshot-plus-replay vid gap, stale feed, reconnect eller motsagande orderstatus

- [ ] Order lifecycle state machine + idempotency
  - Requested -> Accepted -> Live -> PartiallyFilled -> Matched -> Confirmed -> CancelRequested -> Canceled/Reconciled/Rejected
  - Correlation IDs, retry safety, no duplicate submits eller blind optimistic UI

- [ ] Staleness, heartbeat och latency budgets
  - PING/PONG watchdog pa market/user feeds
  - Mata wire-to-decision, submit-to-ack, ack-to-fill, data age och queue pressure
  - Blocka osaker trading nar data inte ar 1:1 med verkligheten

- [ ] Polymarket API compatibility matrix
  - CLOB REST, market WS, user WS, auth headers, subscription payloads, heartbeat rules
  - Hall docs och implementation synkade med aktuell venue-surface

- [ ] Shadow validation mot verkligheten
  - Jamfor lokal state mot /orders, positions, trades och user feed regelbundet
  - Markera avvikelser tydligt i logs, health endpoints och web UI

- [ ] UI/API snapshot integrity
  - Dashboard, PnL-graf, positions-tabell och account summary ska komma fran samma reconciled snapshot
  - Inga separata calculations i UI som kan drifta fran engine/API-sanningen
