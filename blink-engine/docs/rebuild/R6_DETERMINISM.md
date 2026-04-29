# R-6 — Replay / parity determinism

**Todo**: `r6-determinism`. **Status**: schema implemented in `blink-types`; fingerprint
rules implemented in `blink-shadow`. This doc is the operator's reference.

## Invariants (wire-frozen in `blink-types` v1)

- `SCHEMA_VERSION = 1`, `SCHEMA_NAME = "blink.journal.v1"`. Any change is a new schema
  file + migration; no silent edits.
- `EventId` is monotonic per-process + stable within a run. Across runs, the `(run_id,
  event_id)` pair is the join key. `run_id` is a 128-bit random at process start.
- `IntentHash` is 32 bytes = keccak256 of the EIP-712 order struct-hash. Deterministic
  given an intent; does not include `client_order_id`.
- `config_hash` (u64) = FNV-1a over the sorted, serialized config bag used at decision
  time. Different config ⇒ different hash ⇒ diverging decisions are not a bug.
- `code_git_sha` (20 bytes) is baked in at build time via `build.rs`.

## `OutcomeFingerprintV1` rules (frozen in `blink-shadow`)

Fields **included**:
- `outcome_tag` (Submitted=0, Aborted=1, NoOp=2)
- For Submitted: `intent_hash` (32 B)
- For Aborted: `reason_code` (u8), NOT `metric: Option<i64>` (noisy bps)
- For NoOp: `NoOpCode` (u8 enum: Unknown=0, BelowEdgeThreshold=1, CooldownActive=2,
  InventorySaturated=3, FilterMismatch=4, Dedup=5) — free-text `reason: String` is
  classified, not hashed

Fields **excluded by design**:
- `client_order_id` (nondeterministic)
- `StageTimestamps` (absolute TSC, per-run)
- `wall_clock_ns`
- `metric: Option<i64>` on aborts

## Refusal-to-start against self

`ShadowRunner` panics if `legacy.impl_id() == v2.impl_id()`. This is a *deliberate*
false-green guard — running a kernel against itself always agrees and proves nothing.

## Operator contract

To trust a shadow-mode 24 h divergence report:
1. `blink_shadow_divergence_total` must be 0.
2. `run_id` of the run must appear in `blink_journal_runs_total` on both kernels.
3. `config_hash` must be identical across kernels within the run (if not, divergence
   is by construction — fail the report).
4. `legacy.impl_id()` and `v2.impl_id()` must both be set and distinct.

Divergence reports land in ClickHouse table `blink.divergences.v1`. A grafana panel
breaks them down by `(event_kind, outcome_tag_legacy, outcome_tag_v2)`.

## Not yet implemented (follow-ups)

- `p0-shadow-hook`: wiring `DecisionObserver` into legacy `paper_engine.rs`.
- `p0-shadow-capture`: persisting `DecisionInput` for offline replay.
- `p0-shadow-live`: live WS tap driving both kernels.
- `p0-shadow-gate`: the 24 h ≥ 0 divergence CI gate in `scripts/run-shadow-gate.sh`.
