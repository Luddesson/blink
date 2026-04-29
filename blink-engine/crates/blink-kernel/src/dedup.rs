//! Fixed-size ring buffer of recently-submitted semantic keys, used for
//! allocation-free dedup inside the kernel.
//!
//! The ring is the caller's responsibility:
//!
//! - kernel receives `&RecentKeySet` via [`crate::DecisionSnapshot`];
//! - kernel calls [`RecentKeySet::contains`] during the dedup check;
//! - on `Submit`, the caller inserts the emitted `semantic_key` **after**
//!   `decide` returns.

/// Ring-buffer capacity. Powers of two only — the wrap is `& (N - 1)`.
pub const RING_CAPACITY: usize = 128;

const _: () = {
    assert!(RING_CAPACITY.is_power_of_two());
};

/// 128-slot ring of 32-byte semantic keys. Pure POD — `Copy` is avoided
/// only because 128 × 32 B would make implicit copies expensive (4 KiB).
#[derive(Clone)]
pub struct RecentKeySet {
    ring: [[u8; 32]; RING_CAPACITY],
    /// Monotonic insertion index. Slot = `head as usize & (N - 1)`.
    /// `head == 0` ⇒ empty (distinguished because slot 0 is zeroed).
    head: u32,
}

impl RecentKeySet {
    /// Empty set.
    pub fn new() -> Self {
        Self {
            ring: [[0u8; 32]; RING_CAPACITY],
            head: 0,
        }
    }

    /// `true` iff `key` is present in the populated portion of the ring.
    #[inline]
    pub fn contains(&self, key: &[u8; 32]) -> bool {
        let populated = (self.head as usize).min(RING_CAPACITY);
        if populated == 0 {
            return false;
        }
        // Scan only the populated slots. The full ring is scanned once
        // the ring has wrapped.
        let scan_all = self.head as usize >= RING_CAPACITY;
        if scan_all {
            self.ring.iter().any(|slot| slot == key)
        } else {
            self.ring[..populated].iter().any(|slot| slot == key)
        }
    }

    /// Append `key` to the ring (FIFO eviction when full).
    #[inline]
    pub fn insert(&mut self, key: [u8; 32]) {
        let slot = (self.head as usize) & (RING_CAPACITY - 1);
        self.ring[slot] = key;
        self.head = self.head.wrapping_add(1);
    }

    /// Number of currently-populated slots.
    #[inline]
    pub fn len(&self) -> usize {
        (self.head as usize).min(RING_CAPACITY)
    }

    /// Whether the ring has no populated slots.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head == 0
    }
}

impl Default for RecentKeySet {
    fn default() -> Self {
        Self::new()
    }
}
