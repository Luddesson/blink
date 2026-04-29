//! SPSC throughput marker bench. Not part of any CI gate — just a baseline
//! number to eyeball after touching the wrapper.

use blink_rings::bounded;
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct Payload64([u8; 64]);

fn spsc_throughput(c: &mut Criterion) {
    const CAP: usize = 1024;
    const N: usize = 64 * 1024;

    let mut g = c.benchmark_group("blink-rings");
    g.throughput(Throughput::Elements(N as u64));
    g.bench_function("spsc_cap1024_payload64", |b| {
        b.iter(|| {
            let (mut p, mut cons) = bounded::<Payload64>(CAP);
            let prod = std::thread::spawn(move || {
                let msg = Payload64([0xab; 64]);
                let mut i = 0usize;
                while i < N {
                    match p.push(msg) {
                        Ok(()) => i += 1,
                        Err(_) => std::thread::yield_now(),
                    }
                }
            });
            let consumer = std::thread::spawn(move || {
                let mut i = 0usize;
                while i < N {
                    match cons.pop() {
                        Some(v) => {
                            black_box(v);
                            i += 1;
                        }
                        None => std::thread::yield_now(),
                    }
                }
            });
            prod.join().unwrap();
            consumer.join().unwrap();
        });
    });
    g.finish();
}

criterion_group!(benches, spsc_throughput);
criterion_main!(benches);
