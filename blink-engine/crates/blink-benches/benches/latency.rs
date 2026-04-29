//! Wall-clock micro-benchmarks for the Blink v2 hot path.
//!
//! See `plan.md` §"Latency budget" for targets. Run with:
//!
//!     cargo bench -p blink-benches --bench latency

use std::hint::black_box;
use std::sync::Once;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use blink_timestamps::{init_with_policy, InitPolicy, Timestamp};
use blink_types::{
    EventId, Intent, PriceTicks, Side, SizeU, StageTimestamps, TimeInForce,
};

use k256::ecdsa::{signature::Signer, Signature, SigningKey};
use rand::rngs::OsRng;
use sha3::{Digest, Keccak256};

/// Idempotent init — `AllowFallback` lets these benches run on CI /
/// containers where invariant TSC isn't exposed. On bare-metal colo the
/// real production `init()` path enforces `RequireInvariantTsc`.
fn init_clock() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = init_with_policy(InitPolicy::AllowFallback);
    });
}

fn bench_timestamp_now(c: &mut Criterion) {
    init_clock();
    c.bench_function("ts_now", |b| {
        b.iter(|| {
            let t: Timestamp = Timestamp::now();
            black_box(t.raw());
        })
    });
}

fn bench_stage_stamp(c: &mut Criterion) {
    init_clock();
    c.bench_function("stage_stamp/ingress_then_parse", |b| {
        b.iter(|| {
            let mut st = StageTimestamps::UNSET;
            st.tsc_in = Timestamp::now();
            st.tsc_parse = Timestamp::now();
            black_box(st);
        })
    });
}

fn bench_event_id_alloc(c: &mut Criterion) {
    c.bench_function("event_id_alloc", |b| {
        b.iter(|| black_box(EventId::fetch_next()))
    });
}

const SAMPLE_BOOK_UPDATE_JSON: &str = r#"{
  "event_type": "book",
  "market": "0x1234567890abcdef1234567890abcdef12345678",
  "asset_id": "71321045679252212594626385532706912750332728571942134278691923864618068803120",
  "timestamp": "1713974400123",
  "hash": "0xabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
  "bids": [
    {"price": "0.523", "size": "1500.0"},
    {"price": "0.522", "size": "2200.0"},
    {"price": "0.521", "size": "4500.0"},
    {"price": "0.520", "size": "9800.0"},
    {"price": "0.519", "size": "15000.0"}
  ],
  "asks": [
    {"price": "0.525", "size": "1800.0"},
    {"price": "0.526", "size": "3100.0"},
    {"price": "0.527", "size": "5200.0"},
    {"price": "0.528", "size": "11000.0"},
    {"price": "0.529", "size": "22000.0"}
  ]
}"#;

fn bench_json_parse(c: &mut Criterion) {
    let payload_bytes = SAMPLE_BOOK_UPDATE_JSON.as_bytes();

    let mut group = c.benchmark_group("ingress_parse");
    group.throughput(Throughput::Bytes(payload_bytes.len() as u64));

    group.bench_function(BenchmarkId::new("serde_json", payload_bytes.len()), |b| {
        b.iter(|| {
            let v: serde_json::Value =
                serde_json::from_slice(black_box(payload_bytes)).expect("valid json");
            black_box(v);
        })
    });

    group.bench_function(BenchmarkId::new("simd_json", payload_bytes.len()), |b| {
        b.iter_batched(
            || payload_bytes.to_vec(),
            |mut buf| {
                let v: simd_json::OwnedValue =
                    simd_json::to_owned_value(black_box(&mut buf)).expect("valid json");
                black_box(v);
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_keccak256_eip712_digest(c: &mut Criterion) {
    let buf = [0x5au8; 128];
    c.bench_function("keccak256/eip712_128B", |b| {
        b.iter(|| {
            let mut h = Keccak256::new();
            h.update(black_box(&buf));
            black_box(h.finalize());
        })
    });
}

fn bench_k256_sign(c: &mut Criterion) {
    let sk = SigningKey::random(&mut OsRng);
    let digest = [0x17u8; 32];
    c.bench_function("k256/sign_32B_digest", |b| {
        b.iter(|| {
            let sig: Signature = sk.sign(black_box(&digest));
            black_box(sig);
        })
    });
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

fn bench_intent_roundtrip(c: &mut Criterion) {
    init_clock();
    let intent = sample_intent();
    let encoded = serde_json::to_vec(&intent).expect("serialize");

    let mut group = c.benchmark_group("intent_roundtrip");
    group.throughput(Throughput::Bytes(encoded.len() as u64));

    group.bench_function("serialize", |b| {
        b.iter(|| {
            let v = serde_json::to_vec(black_box(&intent)).expect("serialize");
            black_box(v);
        })
    });

    group.bench_function("deserialize", |b| {
        b.iter(|| {
            let v: Intent = serde_json::from_slice(black_box(&encoded)).expect("deserialize");
            black_box(v);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_timestamp_now,
    bench_stage_stamp,
    bench_event_id_alloc,
    bench_json_parse,
    bench_keccak256_eip712_digest,
    bench_k256_sign,
    bench_intent_roundtrip,
);
criterion_main!(benches);
