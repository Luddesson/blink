# R-4 — Measurement invalidation guards

**Todo**: `r4-measure`. **Status**: largely implemented in `blink-timestamps`; this doc
is the **operator runbook** for what to watch.

## Three ways our clock lies to us

### 1. Cross-core TSC skew
TSC is per-CPU; on some CPUs it's synchronized, on others not. Two threads on two
cores may read TSC values that differ by microseconds even at the "same" wall instant.
**Mitigation**: `blink_timestamps::run_skew_selftest()` at boot. Gate boot on result:
- `Ok` → proceed.
- `Warn(reason)` → proceed but log warn + emit metric `blink_tsc_skew_warn`.
- `Fail(bound_ns)` → refuse to start if bound > 500 ns. Crash → systemd restart → page.

### 2. VM live migration
On KVM/vSphere/EC2, live migration can jump TSC forward or back by milliseconds.
**Mitigation**: periodic drift check. Every 60 s, a background tokio task reads both
`Timestamp::now()` and `CLOCK_MONOTONIC_RAW` and compares slope against the calibrated
`tsc_hz`. If slope drift > 0.1 % for two consecutive samples, re-calibrate and emit
`blink_tsc_drift_recal`. If > 1 % over one sample, refuse further stage-stamping
(use `Instant` fallback) until next boot. Track as:

```
metric: blink_tsc_drift_ppm  (gauge)
metric: blink_tsc_recals_total  (counter)
alert: blink_tsc_drift_ppm > 1000 for 5m ⇒ page
```

### 3. Calibration startup race
`CalibratedState` is behind `OnceLock`. First call to `Timestamp::now()` before calibration
completes returns the `Instant` fallback path. Stage-stamp code must tolerate this;
no invariant "all timestamps in a run come from the same backend".
**Mitigation**: journal records `calibration_source: Cpuid15 | MonotonicRaw | InstantFallback`
per startup. Replay code must normalize.

## Journal invariant

Every `JournalRow` MUST carry:
- `calibration_source` (enum tag)
- `tsc_hz_estimate` (u64)
- `wall_clock_ns` (SystemTime at the stage, for cross-run sanity)

Already frozen in `blink-types` schema v1. Downstream: the shadow replay gate compares
only `(tsc_decision - tsc_in)` deltas, never absolute timestamps, so skew between two
runs cannot trigger false divergence.

## Operational check list (weekly)

1. `journalctl -u blink-engine | grep TSC` — any `Fail`? → incident.
2. Grafana panel "TSC drift PPM (7d)" — any spikes > 500? → investigate VM host.
3. Correlate `blink_tsc_recals_total` with PnL anomalies — should be zero correlation;
   non-zero is a bug.
