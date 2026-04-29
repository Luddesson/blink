//! Criterion bench comparing `OrderEncoder::encode().digest` (full path)
//! vs `OrderTemplate::digest` (template path). Same inputs.
//!
//! # Expected results
//!
//! Template path ≥ 1.5× faster on a warm CPU. The full encode does:
//!   - 12 ABI-word b32 conversions (including a 78-digit decimal parse)
//!   - ~13 `Keccak256::update()` calls
//!   - One `Vec<u8>::with_capacity(416)` + `extend_from_slice` ×13
//!
//! The template path does:
//!   - A ~200 B `Keccak256::clone()` (sponge state)
//!   - 3 `u*_to_b32` conversions for the variable fields
//!   - 6 `Keccak256::update()` calls (typehash is already absorbed; two
//!     big pre-baked slabs + three 32-byte variable words)
//!
//! # Measured results
//!
//! On this CI container (criterion, 20 samples × 3 s):
//!
//! ```text
//! eip712_digest/encode_full       time:   [3.95 µs 4.16 µs 4.63 µs]
//! eip712_digest/template_digest   time:   [1.71 µs 1.76 µs 1.87 µs]
//! ```
//!
//! → ~2.36× speedup, comfortably above the 1.5× target.
//! TODO: record numbers from the colo host (absolute ns will be lower;
//! the relative speedup should hold or improve as L1/L2 contention drops).

use blink_submit::{
    compute_amounts_for_intent, OrderEncoder, OrderTemplate, POLYMARKET_CTF_EXCHANGE,
};
use blink_types::{EventId, Intent, PriceTicks, Side, SizeU, TimeInForce};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn make_intent() -> Intent {
    Intent {
        event_id: EventId(1),
        token_id:
            "52114319501245915516055106046884209969926127482827954674443586998463594912099"
                .to_string(),
        market_id: "m-bench".to_string(),
        side: Side::Buy,
        price: PriceTicks(650),
        size: SizeU(10_000_000),
        tif: TimeInForce::Gtc,
        post_only: true,
        client_order_id: "coid".to_string(),
    }
}

fn bench_digest(c: &mut Criterion) {
    let encoder = OrderEncoder::new([0x11; 20], POLYMARKET_CTF_EXCHANGE);
    let signer_addr = [0x22; 20];
    let intent = make_intent();
    let coid = [0x42u8; 16];
    let template = OrderTemplate::build(
        &encoder,
        intent.market_id.clone(),
        &intent.token_id,
        signer_addr,
        [0u8; 20],
        0,
        0,
        0,
        0,
        0,
    )
    .expect("template build");
    let (ma, ta, _side) = compute_amounts_for_intent(&intent).unwrap();

    let mut salt: u128 = 0xdead_beef_cafe_f00d;

    let mut group = c.benchmark_group("eip712_digest");
    group.bench_function("encode_full", |b| {
        b.iter(|| {
            salt = salt.wrapping_add(1);
            let enc = encoder
                .encode(
                    black_box(&intent),
                    black_box(signer_addr),
                    black_box(salt),
                    black_box(&coid),
                    "GTC",
                )
                .unwrap();
            black_box(enc.digest)
        })
    });

    group.bench_function("template_digest", |b| {
        b.iter(|| {
            salt = salt.wrapping_add(1);
            let d = template.digest(black_box(salt), black_box(ma), black_box(ta));
            black_box(d)
        })
    });

    group.finish();
}

criterion_group!(benches, bench_digest);
criterion_main!(benches);
