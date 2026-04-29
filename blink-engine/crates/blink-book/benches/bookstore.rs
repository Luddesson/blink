//! Criterion microbenchmarks for `blink-book`.
//!
//! Run with:
//! ```text
//! cargo bench -p blink-book -- --measurement-time 2 --warm-up-time 1
//! ```

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use blink_book::{BookSnapshot, BookStore, LadderSide, Level, Timestamp, BOOK_DEPTH};
use criterion::{criterion_group, criterion_main, Criterion};

fn mk_snap(tok: &str, seq: u64) -> BookSnapshot {
    let mut bid = LadderSide::EMPTY;
    let mut ask = LadderSide::EMPTY;
    for i in 0..BOOK_DEPTH {
        bid.levels[i] = Level::new(500 - i as u32, 1_000 + i as u64);
        ask.levels[i] = Level::new(501 + i as u32, 1_000 + i as u64);
    }
    bid.len = BOOK_DEPTH as u8;
    ask.len = BOOK_DEPTH as u8;
    BookSnapshot {
        token_id: tok.to_string(),
        market_id: format!("m-{tok}"),
        seq,
        source_wall_ns: seq,
        tsc_received: Timestamp::UNSET,
        bid,
        ask,
    }
}

fn bench_latest_hit(c: &mut Criterion) {
    let store = BookStore::new();
    store.upsert(mk_snap("hot", 1));
    let key = "hot".to_string();
    c.bench_function("bookstore_latest_hit", |b| {
        b.iter(|| {
            let s = store.latest(black_box(&key)).unwrap();
            black_box(s.seq)
        })
    });
}

fn bench_load_fast(c: &mut Criterion) {
    let store = BookStore::new();
    store.upsert(mk_snap("hot", 1));
    let key = "hot".to_string();
    c.bench_function("bookstore_load_fast", |b| {
        b.iter(|| {
            let g = store.load_fast(black_box(&key)).unwrap();
            black_box(g.seq)
        })
    });
}

fn bench_upsert(c: &mut Criterion) {
    let store = Arc::new(BookStore::new());
    store.upsert(mk_snap("hot", 0));

    // Background reader to create realistic contention.
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let store = Arc::clone(&store);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let key = "hot".to_string();
            while !stop.load(Ordering::Relaxed) {
                if let Some(g) = store.load_fast(&key) {
                    black_box(g.seq);
                }
            }
        })
    };

    let key = "hot".to_string();
    let mut i = 1u64;
    c.bench_function("bookstore_upsert", |b| {
        b.iter(|| {
            let mut snap = mk_snap(&key, i);
            snap.seq = i;
            i = i.wrapping_add(1);
            store.upsert(black_box(snap));
        })
    });

    stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();
}

fn bench_reader_under_write(c: &mut Criterion) {
    let store = Arc::new(BookStore::new());
    store.upsert(mk_snap("hot", 0));

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let store = Arc::clone(&store);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let key = "hot".to_string();
            let mut i = 1u64;
            let target_period = Duration::from_micros(1); // ~1 M/s
            let mut next = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let mut snap = mk_snap(&key, i);
                snap.seq = i;
                i = i.wrapping_add(1);
                store.upsert(snap);
                next += target_period;
                let now = Instant::now();
                if next > now {
                    // Busy-sleep; we want steady pressure on the ArcSwap.
                } else {
                    next = now;
                }
            }
        })
    };

    let key = "hot".to_string();
    c.bench_function("bookstore_reader_under_write", |b| {
        b.iter(|| {
            let g = store.load_fast(black_box(&key)).unwrap();
            black_box(g.seq)
        })
    });

    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
}

criterion_group!(
    benches,
    bench_latest_hit,
    bench_load_fast,
    bench_upsert,
    bench_reader_under_write
);
criterion_main!(benches);
