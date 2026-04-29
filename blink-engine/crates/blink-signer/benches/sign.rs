//! Head-to-head: k256 vs secp256k1 single-shot sign latency.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use blink_signer::eip712::keccak256;
use blink_signer::{EcdsaSigner, K256Signer, Secp256k1Signer};

fn bench_sign(c: &mut Criterion) {
    let mut pk = [0u8; 32];
    pk[31] = 42;
    let k = K256Signer::from_bytes(&pk).unwrap();
    let s = Secp256k1Signer::from_bytes(&pk).unwrap();
    let digest = keccak256(b"polymarket order digest");

    c.bench_function("k256_sign_prehash", |b| {
        b.iter(|| {
            let sig = k.sign_prehash(black_box(&digest));
            black_box(sig);
        })
    });

    c.bench_function("secp256k1_sign_prehash", |b| {
        b.iter(|| {
            let sig = s.sign_prehash(black_box(&digest));
            black_box(sig);
        })
    });

    c.bench_function("keccak256_32b", |b| {
        let data = [0u8; 32];
        b.iter(|| {
            let h = keccak256(black_box(&data));
            black_box(h);
        })
    });
}

criterion_group!(benches, bench_sign);
criterion_main!(benches);
