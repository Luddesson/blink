# blink-benches

Micro-benchmarks for the Blink v2 HFT engine hot path. Two independent
harnesses live here:

| Bench file        | Harness        | Purpose                                    |
|-------------------|----------------|--------------------------------------------|
| `benches/latency.rs` | `criterion`    | Wall-clock P50/P99 on real hardware.     |
| `benches/cycles.rs`  | `iai-callgrind`| Deterministic instruction / cycle gates. |

The criterion suite answers *"how fast is this today on this machine?"*
The iai-callgrind suite answers *"did this commit regress a hot path?"* —
its counts are deterministic because callgrind simulates the CPU, so CI
does not need bare-metal isolation to catch a regression.

## Running the criterion suite

```sh
cargo bench -p blink-benches --bench latency
```

Outputs HTML reports under `target/criterion/`. Run this on the colo box
(or at least a quiet laptop) to get meaningful wall-clock numbers.

## Running the iai-callgrind cycle gates

**Requires `valgrind` on `PATH`.** Install with `apt install -y valgrind`
on Debian/Ubuntu, or equivalent.

```sh
cargo bench -p blink-benches --bench cycles
# or, via the CI wrapper:
./crates/blink-benches/scripts/run-cycle-gates.sh
```

The wrapper exits non-zero if valgrind is missing or if any benchmark
breaches its regression threshold.

## Regression thresholds

Each bench carries a `RegressionConfig` that hard-fails on two event
kinds: instructions retired (`Ir`) and callgrind's estimated cycle count
(`EstimatedCycles`). `Ir` is the deterministic signal we trust most;
`EstimatedCycles` folds in cache-miss assumptions and is noisier, so we
allow a wider band.

| Bench                          | Instructions (`Ir`) | Estimated Cycles | Rationale |
|--------------------------------|---------------------|------------------|-----------|
| `cycles_ts_now`                | +5 %                | +10 %            | TSC read — should be 1 `rdtscp`; anything bigger means we regressed the fence strategy. |
| `cycles_event_id_alloc`        | +5 %                | +10 %            | Atomic FAA on a hot counter; regressions usually mean contention or cacheline moves. |
| `cycles_stage_stamp`           | +5 %                | +10 %            | Two stamps into a `StageTimestamps`. Tight bound flags accidental struct growth. |
| `cycles_keccak256_128b`        | **+3 %**            | **+8 %**         | Submit critical path (EIP-712 digest). ~200 ns today → 3 % ≈ 6 ns/submit — meaningful against the 2 ms budget. |
| `cycles_simd_json_parse_small` | +5 %                | +10 %            | Ingress parse on a realistic Polymarket book snippet. Tighter would fight SIMD codegen drift across rustc releases. |
| `cycles_intent_hash_compute`   | +5 %                | +10 %            | Serde-JSON serialize + keccak — the dedup/determinism key. |
| `cycles_k256_sign`             | **+3 %**            | **+8 %**         | ECDSA sign dominates submit latency; we want a tripwire rather than a wide band. |

Thresholds are centralized in `benches/cycles.rs` under the `thresholds`
module. Change them there and the per-bench `config = cfg(...)`
invocations pick them up automatically.

## Baselines

iai-callgrind stores baselines under `target/iai/`. **That directory is
not committed.** Policy:

* Dev machines: re-run the suite before and after your change; the
  second run reports a diff against the first.
* CI on the colo host: the job either re-establishes a fresh baseline
  per run, or (preferred long-term) restores a known baseline from an
  artifact store keyed on the target commit. Implementing that storage
  is later ops work — see `plan.md` §"Cycle gates".

### Updating baselines after a legitimate perf improvement

If you intentionally improve a hot path, the *old* baseline becomes
invalid and future runs will flag the improvement as a "change".
Procedure:

1. Land the improvement.
2. Run `cargo bench -p blink-benches --bench cycles` once to produce a
   new `target/iai/` snapshot on the colo host.
3. Promote that snapshot to the baseline artifact store (mechanism TBD
   under Phase 1 ops).
4. Revisit the thresholds in `cycles.rs` — a genuine 30 % win probably
   warrants a tighter regression band going forward.

## Current baselines on this hardware

**TODO: baselines.**

The CI machine this file was authored on has `valgrind 3.22.0`, which
does not recognise some of the x86 extensions emitted by `k256` on
modern Intel hosts (`SIGILL` on `cycles_k256_sign`). Six of the seven
cycle benches run to completion under callgrind here; the seventh
requires `valgrind ≥ 3.24` or the colo host's CPU/valgrind pairing.

Once the colo host runs the suite, paste the P50/P99 criterion wall
clock numbers plus the iai `Ir` / `EstimatedCycles` figures here. Until
then, treat these fields as unmeasured.

| Bench                          | Ir (baseline) | EstimatedCycles (baseline) | Wall-clock P50 | Wall-clock P99 |
|--------------------------------|---------------|----------------------------|----------------|----------------|
| `cycles_ts_now`                | TODO          | TODO                       | TODO           | TODO           |
| `cycles_event_id_alloc`        | TODO          | TODO                       | TODO           | TODO           |
| `cycles_stage_stamp`           | TODO          | TODO                       | TODO           | TODO           |
| `cycles_keccak256_128b`        | TODO          | TODO                       | TODO           | TODO           |
| `cycles_simd_json_parse_small` | TODO          | TODO                       | TODO           | TODO           |
| `cycles_intent_hash_compute`   | TODO          | TODO                       | TODO           | TODO           |
| `cycles_k256_sign`             | TODO          | TODO                       | TODO           | TODO           |
