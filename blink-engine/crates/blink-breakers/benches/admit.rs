use std::time::Duration;

use blink_breakers::{BreakerSet, BreakerSetConfig};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_admit_submit(c: &mut Criterion) {
    let s = BreakerSet::new(BreakerSetConfig::default());
    let mut now = 1_000_000_000u64;
    c.bench_function("BreakerSet::admit_submit (Closed)", |b| {
        b.iter(|| {
            now = now.wrapping_add(1);
            black_box(s.admit_submit(black_box(now)))
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(200))
        .measurement_time(Duration::from_secs(2));
    targets = bench_admit_submit
}
criterion_main!(benches);
