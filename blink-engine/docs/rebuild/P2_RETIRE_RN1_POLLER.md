# P2 — Retiring the RN1 REST poller

> Owner: application. Plan: §3 Phase 2 exit criteria.
> Precondition: `blink-ingress::MempoolSource` live in shadow for ≥ 7 days with the
> parity gate green.

The legacy RN1 REST poller (`engine/src/rn1_poller.rs`) pre-dates the mempool-tap
ingress. It polls `data-api.polymarket.com` every 400 ms for trades by the RN1 wallet,
which is 300–2000 ms behind the truth available in the mempool. It is the dominant
source of the "price drifted 125 bps (limit 100 bps)" aborts that motivated this
rebuild.

We do NOT delete it until the mempool source has earned its keep. This runbook
defines "earned".

---

## 1. Parity gate — the 7-day test

The mempool source must, over a rolling 7-day window against the live RN1 feed:

| Metric                                                  | Target       | Tolerance |
|---------------------------------------------------------|--------------|-----------|
| Fraction of RN1 trades seen by mempool (tx hash match)  | ≥ 95 %       | —         |
| Median lead time (mempool_seen_ns − rest_seen_ns)       | ≥ 300 ms     | p5 ≥ 0 ms |
| Duplicate rate (same tx seen twice by mempool)          | ≤ 0.1 %      | —         |
| Missed-then-caught rate (REST saw, mempool did not)     | ≤ 5 %        | —         |
| Decision divergence in `blink-shadow` (v1 kernel on both sources) | 0 events   | —         |

The gate is enforced by an overnight ClickHouse query over the decision journal
plus an ad-hoc `blink-shadow --replay --compare-sources` job. Both queries live in
`ops/queries/parity_gate.sql` (to land with this runbook — TBD).

The "decision divergence" row is the strictest — it says that even on the RN1
trades the REST poller DID see, the v1 kernel emits the same `intent_hash` whether
fed the mempool event or the REST event. If divergence > 0, we investigate before
retiring; it means one of the two sources is lying about something the kernel cares
about (price, size, side).

---

## 2. Sequenced retirement

Stage 0 (current): REST poller ON, mempool source ON, both feed the kernel via the
ingress ring. Kernel dedupes by `OnChainAnchor { tx_hash, log_index }`.

Stage 1 (day 0): flip `BLINK_RN1_REST_FALLBACK=true`. Behaviour unchanged except the
REST poller now ONLY emits events for tx hashes the mempool source has not emitted
within the last 30 s. Expected rate: ≤ 5 % of trades (the "missed-then-caught" row).

Stage 2 (day 7, gate green): flip `BLINK_RN1_REST_FALLBACK=false`. REST poller
is still running but emits nothing. Monitor for 48 h — if the "fraction seen" rate
drops below 95 % without REST as backup, revert.

Stage 3 (day 9): remove the poller from the engine's startup sequence. The module
stays in-tree but unreferenced. Tag the release as `phase-2-rn1-retired`.

Stage 4 (day 30): delete `engine/src/rn1_poller.rs`, its config, and tests. PR
references this runbook.

Rollback at ANY stage: revert the env flag, `systemctl restart blink-engine`. No
data migration, no state to unwind.

---

## 3. Monitoring additions (pre-retirement)

Add to `blink-engine`'s Prometheus exporter:

```
blink_ingress_rn1_source_events_total{source="mempool|rest"}    counter
blink_ingress_rn1_lead_time_ns{source="mempool"}                histogram
blink_ingress_rn1_duplicate_total                               counter
blink_ingress_rn1_rest_fallback_engaged                         gauge 0/1
```

Alert rules (Grafana or equivalent):

- `rate(blink_ingress_rn1_source_events_total{source="mempool"}[1h]) == 0`
  while `rate(blink_ingress_rn1_source_events_total{source="rest"}[1h]) > 0`
  → page. Mempool source silent while REST sees trades ⇒ our Polygon node is down
  or filters are broken.
- `histogram_quantile(0.5, blink_ingress_rn1_lead_time_ns) < 300e6` for 1 h → warn.
  Lead time has collapsed; peering may be bad (see R-8).

---

## 4. Known caveats

- **Private RN1 trades** (submitted via private mempool / Flashbots-style relays):
  mempool source will miss them by design. Current estimate: < 1 % of RN1 volume.
  If this rises, add a paid relay feed (bloXroute / Merkle) before final retirement.
  See R-8.
- **Mempool spoofing**: a tx can appear in the mempool and never be mined. The
  kernel must not act on mempool-observed intents as if they were filled. This is
  enforced by the `RawEvent.observe_only=true` flag the `MempoolSource` sets, which
  `blink-kernel::V1Kernel` maps to `NoOp{FilterMismatch}` per
  `R3_LEGAL_MEMO_STUB.md` (the kernel side of this invariant landed with the v1
  kernel fix — no retirement allowed until that's in a prod build).
- **Chain reorg**: confirmed `OrderFilled` logs from Phase-2's `CtfLogSource` are
  the canonical truth. Mempool events are forward-looking; logs are reconciliation.
  Both stay on after retirement.

---

## 5. Pre-retirement checklist

Operator fills this in on the day of Stage 2:

- [ ] 7-day parity gate query passes (attach ClickHouse link).
- [ ] `blink-shadow --replay --compare-sources` reports 0 divergence.
- [ ] `R3_LEGAL_MEMO_STUB` sign-off confirmed by counsel.
- [ ] Alerting rules in §3 deployed and smoke-tested.
- [ ] Rollback procedure (§2) rehearsed on staging.
- [ ] On-call acknowledged and paged in for the 48 h window.

Once all boxes are checked, flip `BLINK_RN1_REST_FALLBACK=false`.
