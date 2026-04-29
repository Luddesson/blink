//! Deterministic cycle-count gating via `iai-callgrind` (requires valgrind).
//!
//! Run with:
//!
//!     cargo bench -p blink-benches --bench cycles
//!
//! Each benchmark carries a [`RegressionConfig`] that hard-fails the run when
//! either the **instruction count** (`EventKind::Ir`) or the **estimated cycle
//! count** (`EventKind::EstimatedCycles`) regresses beyond the per-bench
//! threshold. Thresholds are centralized in [`thresholds`] so a single edit
//! retunes every gate.
//!
//! Rationale for the tight bounds is documented inline; see also
//! `crates/blink-benches/README.md` for the table.

use std::hint::black_box;
use std::sync::Once;

use blink_timestamps::{init_with_policy, InitPolicy, Timestamp};
use blink_types::{
    EventId, Intent, PriceTicks, Side, SizeU, StageTimestamps, TimeInForce,
};
use iai_callgrind::{
    library_benchmark, library_benchmark_group, main, EventKind, LibraryBenchmarkConfig,
    RegressionConfig,
};
use k256::ecdsa::{signature::Signer, Signature, SigningKey};
use sha3::{Digest, Keccak256};

fn init_clock() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = init_with_policy(InitPolicy::AllowFallback);
    });
}

// ---------------------------------------------------------------------------
// Regression thresholds
// ---------------------------------------------------------------------------
//
// Thresholds are expressed as "hard-fail" percentages: a run fails if the
// measured event count exceeds the stored baseline by more than this amount.
// The pair is `(Ir %, EstimatedCycles %)`.
//
// Guidelines:
//   * `Ir` (instructions retired) is the deterministic, compiler-visible
//     signal; we gate it tightly because a regression here almost always
//     reflects a real code-path change.
//   * `EstimatedCycles` folds in cache-miss assumptions and is noisier, so we
//     allow a wider band.
//
// Per-bench rationale is attached at the callsite.

mod thresholds {
    pub const HOT_PATH_IR: f64 = 5.0;
    pub const HOT_PATH_CYCLES: f64 = 10.0;

    // Keccak sits on the submit critical path (EIP-712 digest before sign).
    // Given a 2 ms end-to-end budget and keccak256 at ~200 ns for 128 B, a 3%
    // regression is ~6 ns/submit — small in isolation, meaningful at rate.
    pub const KECCAK_IR: f64 = 3.0;
    pub const KECCAK_CYCLES: f64 = 8.0;

    // Intent hash = serialize + keccak. Dominated by serde_json; we pick the
    // same hot-path Ir band because the serializer is well-exercised code.
    pub const INTENT_HASH_IR: f64 = 5.0;
    pub const INTENT_HASH_CYCLES: f64 = 10.0;

    // k256 ECDSA sign is the single biggest contributor to submit latency.
    // Any regression here compounds under fire, so keep Ir tight.
    pub const SIGN_IR: f64 = 3.0;
    pub const SIGN_CYCLES: f64 = 8.0;

    // simd-json small-payload parse: dominates ingress. Same defaults as the
    // rest of the hot path — tighter would fight SIMD codegen drift.
    pub const JSON_PARSE_IR: f64 = 5.0;
    pub const JSON_PARSE_CYCLES: f64 = 10.0;
}

fn regression(ir_pct: f64, cycles_pct: f64) -> RegressionConfig {
    let mut r = RegressionConfig::default();
    r.limits([
        (EventKind::Ir, ir_pct),
        (EventKind::EstimatedCycles, cycles_pct),
    ])
    .fail_fast(false);
    r
}

fn cfg(ir_pct: f64, cycles_pct: f64) -> LibraryBenchmarkConfig {
    let mut c = LibraryBenchmarkConfig::default();
    c.regression(regression(ir_pct, cycles_pct));
    c
}

// ---------------------------------------------------------------------------
// Hot-path benches
// ---------------------------------------------------------------------------

#[library_benchmark(config = cfg(thresholds::HOT_PATH_IR, thresholds::HOT_PATH_CYCLES))]
fn cycles_ts_now() -> u64 {
    init_clock();
    black_box(Timestamp::now().raw())
}

#[library_benchmark(config = cfg(thresholds::HOT_PATH_IR, thresholds::HOT_PATH_CYCLES))]
fn cycles_event_id_alloc() -> u64 {
    black_box(EventId::fetch_next().raw())
}

#[library_benchmark(config = cfg(thresholds::HOT_PATH_IR, thresholds::HOT_PATH_CYCLES))]
fn cycles_stage_stamp() -> StageTimestamps {
    init_clock();
    let mut st = StageTimestamps::UNSET;
    st.tsc_in = Timestamp::now();
    st.tsc_parse = Timestamp::now();
    black_box(st)
}

#[library_benchmark(config = cfg(thresholds::KECCAK_IR, thresholds::KECCAK_CYCLES))]
fn cycles_keccak256_128b() -> [u8; 32] {
    let buf = [0x5au8; 128];
    let mut h = Keccak256::new();
    h.update(black_box(&buf));
    black_box(h.finalize().into())
}

// ---------------------------------------------------------------------------
// Extended benches (Phase 0 additions)
// ---------------------------------------------------------------------------

/// Self-contained copy of the Polymarket book-snippet fixture used by the
/// criterion latency suite. Duplicated deliberately — iai benches must not
/// depend on anything outside this file.
const SAMPLE_BOOK_SNIPPET: &[u8] = br#"{"event_type":"book","market":"0x1234567890abcdef1234567890abcdef12345678","asset_id":"71321045679252212594626385532706912750332728571942134278691923864618068803120","timestamp":"1713974400123","bids":[{"price":"0.523","size":"1500.0"},{"price":"0.522","size":"2200.0"}],"asks":[{"price":"0.525","size":"1800.0"}]}"#;

#[library_benchmark(config = cfg(thresholds::JSON_PARSE_IR, thresholds::JSON_PARSE_CYCLES))]
fn cycles_simd_json_parse_small() -> usize {
    // simd-json mutates the input buffer in-place; clone per call.
    let mut buf = SAMPLE_BOOK_SNIPPET.to_vec();
    let v: simd_json::OwnedValue =
        simd_json::to_owned_value(black_box(&mut buf)).expect("valid json");
    // Return a non-trivial derived value so the optimizer can't DCE the parse.
    black_box(match &v {
        simd_json::OwnedValue::Object(map) => map.len(),
        _ => 0,
    })
}

fn sample_intent() -> Intent {
    Intent {
        event_id: EventId::fetch_next(),
        token_id:
            "71321045679252212594626385532706912750332728571942134278691923864618068803120"
                .to_string(),
        market_id: "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        side: Side::Buy,
        price: PriceTicks(523),
        size: SizeU(1_500_000),
        tif: TimeInForce::Fak,
        post_only: false,
        client_order_id: "blink-benches-00000000000000000001".to_string(),
    }
}

#[library_benchmark(config = cfg(thresholds::INTENT_HASH_IR, thresholds::INTENT_HASH_CYCLES))]
fn cycles_intent_hash_compute() -> [u8; 32] {
    // Represents the canonical dedup/determinism key: serialize then hash.
    let intent = sample_intent();
    let bytes = serde_json::to_vec(black_box(&intent)).expect("serialize");
    let mut h = Keccak256::new();
    h.update(&bytes);
    black_box(h.finalize().into())
}

#[library_benchmark(config = cfg(thresholds::SIGN_IR, thresholds::SIGN_CYCLES))]
fn cycles_k256_sign() -> Signature {
    // Deterministic key (not OsRng) so the bench is reproducible under
    // callgrind. k256 signing is otherwise RFC-6979 deterministic given the
    // (sk, digest) pair.
    let sk_bytes = [0x42u8; 32];
    let sk = SigningKey::from_bytes((&sk_bytes).into()).expect("valid scalar");
    let digest = [0x17u8; 32];
    black_box(sk.sign(black_box(&digest)))
}

library_benchmark_group!(
    name = hot_path;
    benchmarks =
        cycles_ts_now,
        cycles_event_id_alloc,
        cycles_stage_stamp,
        cycles_keccak256_128b,
        cycles_simd_json_parse_small,
        cycles_intent_hash_compute,
        cycles_k256_sign,
);

main!(library_benchmark_groups = hot_path);
