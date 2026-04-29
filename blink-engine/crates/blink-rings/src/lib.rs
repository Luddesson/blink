//! # blink-rings
//!
//! Thin, HFT-flavoured wrapper around [`rtrb`] bounded SPSC lock-free rings plus
//! a small core-affinity helper module. This crate is intentionally small:
//! it's the plumbing the rest of the Blink engine composes higher-level kernels
//! on top of.
//!
//! Design notes:
//! * Capacities are asserted to be powers of two so downstream consumers can
//!   rely on `& (cap - 1)` cheap modulo arithmetic.
//! * Producer's strict `push` returns the rejected `T` on a full ring so the
//!   caller picks the policy (drop, overflow-to-heap, coalesce, …); the lossy
//!   sibling `push_overwrite_oldest` is provided for feeds where staleness is
//!   the correct failure mode.
//! * Per-ring atomic counters (`rows_pushed / rows_popped / rows_dropped`) are
//!   shared between the producer and consumer endpoints via an `Arc` and
//!   snapshotted through [`RingStats`] with `Relaxed` loads — cheap enough to
//!   poll from a metrics thread without perturbing the hot path.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod affinity;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use affinity::{AffinityError, CoreAffinity, pin_current_to, spawn_pinned, verify_pinned};

/// Shared atomic counters, one instance per ring, referenced by both endpoints.
#[derive(Debug, Default)]
struct Counters {
    rows_pushed: AtomicU64,
    rows_popped: AtomicU64,
    rows_dropped: AtomicU64,
}

/// Snapshot of a ring's lifetime counters. All fields are plain `u64`s loaded
/// with `Relaxed` ordering; values are monotonic but may be mildly skewed
/// relative to each other under contention (acceptable for observability).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RingStats {
    /// Successful `push` calls (including the successful half of `push_overwrite_oldest`).
    pub rows_pushed: u64,
    /// Successful `pop` calls from the consumer side.
    pub rows_popped: u64,
    /// Elements rejected by strict `push` plus elements evicted by
    /// `push_overwrite_oldest` — i.e. anything the ring dropped on the floor.
    pub rows_dropped: u64,
}

/// Construct a bounded SPSC ring with the given power-of-two capacity. Panics
/// with a clear message if `cap_pow2` is zero or not a power of two.
pub fn bounded<T>(cap_pow2: usize) -> (Producer<T>, Consumer<T>) {
    assert!(
        cap_pow2.is_power_of_two(),
        "blink-rings: capacity must be a power of two, got {cap_pow2}"
    );
    let (p, c) = rtrb::RingBuffer::<T>::new(cap_pow2);
    let counters = Arc::new(Counters::default());
    let consumer = Arc::new(std::sync::Mutex::new(c));
    (
        Producer {
            inner: p,
            consumer: consumer.clone(),
            counters: counters.clone(),
        },
        Consumer {
            inner: consumer,
            counters,
        },
    )
}

/// Producer half of a bounded SPSC ring.
///
/// Only one `Producer` exists per ring; it is `Send` but not `Sync`.
///
/// The shared reference to the consumer is only touched from
/// [`Producer::push_overwrite_oldest`] on the overflow path; strict `push` is
/// lock-free.
pub struct Producer<T> {
    inner: rtrb::Producer<T>,
    consumer: Arc<std::sync::Mutex<rtrb::Consumer<T>>>,
    counters: Arc<Counters>,
}

impl<T> std::fmt::Debug for Producer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Producer")
            .field("capacity", &self.capacity())
            .field("stats", &self.stats())
            .finish()
    }
}

impl<T> Producer<T> {
    /// Strict push. On success returns `Ok(())` and bumps `rows_pushed`. On a
    /// full ring returns `Err(v)` handing the rejected element back to the
    /// caller and bumps `rows_dropped`.
    #[inline]
    pub fn push(&mut self, v: T) -> Result<(), T> {
        match self.inner.push(v) {
            Ok(()) => {
                self.counters.rows_pushed.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(rtrb::PushError::Full(v)) => {
                self.counters.rows_dropped.fetch_add(1, Ordering::Relaxed);
                Err(v)
            }
        }
    }

    /// `true` iff the ring has no free slots.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// Free slots currently visible to the producer.
    #[inline]
    pub fn slots(&self) -> usize {
        self.inner.slots()
    }

    /// Total ring capacity, identical on both endpoints.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.buffer().capacity()
    }

    /// Snapshot the shared ring counters.
    #[inline]
    pub fn stats(&self) -> RingStats {
        stats_snapshot(&self.counters)
    }

    /// Lossy push: on a full ring, pop the oldest pending element (via the
    /// shared consumer handle) and push `v` in its place. Returns the dropped
    /// element, or `None` on a clean push.
    ///
    /// This takes a short mutex on the consumer side only on the overflow
    /// path; under normal "ring has space" conditions it is lock-free.
    /// `rows_dropped` is bumped for every evicted element.
    #[inline]
    pub fn push_overwrite_oldest(&mut self, v: T) -> Option<T> {
        match self.inner.push(v) {
            Ok(()) => {
                self.counters.rows_pushed.fetch_add(1, Ordering::Relaxed);
                None
            }
            Err(rtrb::PushError::Full(v)) => {
                // Overflow: evict one old element, count it as dropped, then
                // re-push. If re-push still fails (consumer concurrently
                // filled it — impossible in SPSC) return the new element.
                let evicted = {
                    let mut c = self
                        .consumer
                        .lock()
                        .expect("blink-rings: consumer mutex poisoned");
                    c.pop().ok()
                };
                if evicted.is_some() {
                    self.counters.rows_dropped.fetch_add(1, Ordering::Relaxed);
                }
                match self.inner.push(v) {
                    Ok(()) => {
                        self.counters.rows_pushed.fetch_add(1, Ordering::Relaxed);
                        evicted
                    }
                    Err(rtrb::PushError::Full(v)) => {
                        self.counters.rows_dropped.fetch_add(1, Ordering::Relaxed);
                        // Return the *new* value as dropped; also surface the
                        // evicted one via drop-at-end.
                        drop(evicted);
                        Some(v)
                    }
                }
            }
        }
    }
}

/// Consumer half of a bounded SPSC ring.
pub struct Consumer<T> {
    inner: Arc<std::sync::Mutex<rtrb::Consumer<T>>>,
    counters: Arc<Counters>,
}

impl<T> std::fmt::Debug for Consumer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Consumer")
            .field("stats", &self.stats())
            .finish()
    }
}

impl<T> Consumer<T> {
    /// Pop one element. Returns `None` if the ring is empty.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        let mut c = self
            .inner
            .lock()
            .expect("blink-rings: consumer mutex poisoned");
        match c.pop() {
            Ok(v) => {
                self.counters.rows_popped.fetch_add(1, Ordering::Relaxed);
                Some(v)
            }
            Err(rtrb::PopError::Empty) => None,
        }
    }

    /// `true` iff no element is currently readable.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .expect("blink-rings: consumer mutex poisoned")
            .is_empty()
    }

    /// Elements currently available to the consumer.
    #[inline]
    pub fn slots(&self) -> usize {
        self.inner
            .lock()
            .expect("blink-rings: consumer mutex poisoned")
            .slots()
    }

    /// Total ring capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner
            .lock()
            .expect("blink-rings: consumer mutex poisoned")
            .buffer()
            .capacity()
    }

    /// Snapshot the shared ring counters.
    #[inline]
    pub fn stats(&self) -> RingStats {
        stats_snapshot(&self.counters)
    }
}

fn stats_snapshot(c: &Counters) -> RingStats {
    RingStats {
        rows_pushed: c.rows_pushed.load(Ordering::Relaxed),
        rows_popped: c.rows_popped.load(Ordering::Relaxed),
        rows_dropped: c.rows_dropped.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_single_thread() {
        let (mut p, mut c) = bounded::<u32>(4);
        assert_eq!(p.capacity(), 4);
        assert_eq!(c.capacity(), 4);
        assert!(c.pop().is_none());
        p.push(1).unwrap();
        p.push(2).unwrap();
        assert_eq!(c.pop(), Some(1));
        assert_eq!(c.pop(), Some(2));
        assert_eq!(c.pop(), None);
        let s = p.stats();
        assert_eq!(s.rows_pushed, 2);
        assert_eq!(s.rows_popped, 2);
        assert_eq!(s.rows_dropped, 0);
    }

    #[test]
    fn fill_rejects_and_counts_dropped() {
        let (mut p, _c) = bounded::<u32>(4);
        for i in 0..4 {
            p.push(i).unwrap();
        }
        assert!(p.is_full());
        let err = p.push(99).expect_err("ring should be full");
        assert_eq!(err, 99);
        assert_eq!(p.stats().rows_dropped, 1);
        assert_eq!(p.stats().rows_pushed, 4);
    }

    #[test]
    fn push_overwrite_oldest_on_full_returns_dropped() {
        let (mut p, mut c) = bounded::<u32>(2);
        assert_eq!(p.push_overwrite_oldest(10), None);
        assert_eq!(p.push_overwrite_oldest(11), None);
        // Ring is full; next call should evict the oldest (10) and land 12.
        let dropped = p.push_overwrite_oldest(12);
        assert_eq!(dropped, Some(10), "expected the previously-oldest element");
        assert_eq!(p.stats().rows_dropped, 1);
        // Drain — FIFO order preserved for the survivors.
        let mut drained = Vec::new();
        while let Some(v) = c.pop() {
            drained.push(v);
        }
        assert_eq!(drained, vec![11, 12]);
    }

    #[test]
    #[should_panic(expected = "capacity must be a power of two")]
    fn non_pow2_capacity_panics() {
        let _ = bounded::<u8>(3);
    }

    #[test]
    fn spsc_smoke_10k_in_order() {
        const N: u32 = 10_000;
        let (mut p, mut c) = bounded::<u32>(1024);

        let prod = std::thread::spawn(move || {
            let mut i: u32 = 0;
            while i < N {
                match p.push(i) {
                    Ok(()) => i += 1,
                    Err(_rejected) => std::thread::yield_now(),
                }
            }
        });

        let cons = std::thread::spawn(move || {
            let mut next: u32 = 0;
            while next < N {
                match c.pop() {
                    Some(v) => {
                        assert_eq!(v, next, "out-of-order delivery");
                        next += 1;
                    }
                    None => std::thread::yield_now(),
                }
            }
            next
        });

        prod.join().unwrap();
        let seen = cons.join().unwrap();
        assert_eq!(seen, N);
    }
}
