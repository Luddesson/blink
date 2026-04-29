# P6-PGO — Profile-guided optimization build pipeline

**Todo**: `p6-pgo`. **Status**: doc + script, gated on `blink-shadow` replay having a
stable corpus (which it does post-Phase-0).

## Pipeline

```bash
#!/usr/bin/env bash
# scripts/build-pgo.sh
set -euo pipefail
PROFDIR="$(pwd)/target/pgo-data"
rm -rf "$PROFDIR" && mkdir -p "$PROFDIR"

# 1. instrumented build
RUSTFLAGS="-Cprofile-generate=$PROFDIR" cargo build --release \
  --bin shadow-replay -p blink-shadow

# 2. drive instrumented binary through the replay corpus
./target/release/shadow-replay \
  --input tests/corpus/replay-2024q4.jsonl \
  --iterations 5

# 3. merge profiles
llvm-profdata merge -o "$PROFDIR/merged.profdata" "$PROFDIR"

# 4. optimized build
RUSTFLAGS="-Cprofile-use=$PROFDIR/merged.profdata -Cllvm-args=-pgo-warn-missing-function" \
  cargo build --release \
  -p blink-kernel -p blink-submit -p blink-h2 -p blink-signer

# 5. validate: cycle-count bench must stay ≤ baseline
cargo bench -p blink-benches --bench cycles -- --load-baseline main
```

## Expected lift

Historical Rust PGO on signal-processing hot paths: 5–15 % reduction in CPU instructions
on the hot path. Do NOT promise more — real-world often lands at 3–8 %.

## Gotchas

- **LLVM toolchain match**: `llvm-profdata` version must match the rustc LLVM. Use
  `rustup component add llvm-tools-preview` and invoke
  `$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-profdata`.
- **Corpus staleness**: rebuild PGO weekly on refreshed corpus — an old corpus PGO's the
  wrong hot path as the signal mix shifts.
- **Debug symbols**: PGO release keeps `debug = 1` (line-level) for crash readability.
  Full `debug = 2` disables many opts; avoid.

## Gating

Adds a CI job `pgo-bench` (nightly, not PR-blocking) that:
1. Builds PGO as above.
2. Runs `cycles.rs` bench. Records the delta from the non-PGO baseline in ClickHouse.
3. Warns if delta < 2 % (PGO isn't earning its complexity).

## Blocked by

- `blink-shadow` replay corpus must exist and be representative (post-`p0-shadow-capture`).
- Baseline cycle counts recorded on colo host (post-Phase-1 colo).
