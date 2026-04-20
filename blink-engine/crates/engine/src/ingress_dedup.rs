//! Ingress-level signal deduplication.
//!
//! Catches signals that arrive via both the WebSocket and REST (rn1_poller) paths
//! for the same underlying trade, using an LRU cache with a 60-second TTL.

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lru::LruCache;

use crate::types::{OrderSide, RN1Signal};

/// TTL after which a cached key is considered expired and may be re-processed.
const DEDUP_TTL: Duration = Duration::from_secs(60);

/// Maximum number of keys held in the LRU cache.
const DEDUP_CAPACITY: usize = 65_536;

// ─── Dedup key ───────────────────────────────────────────────────────────────

/// The key used to identify potentially duplicate signals.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DedupKey {
    /// Primary key: upstream order/tx id + optional sequence number.
    Ordered { id: String, seq: u64 },
    /// Fallback key when no upstream id is available: observable fields + coarse timestamp.
    Fallback {
        token_id: String,
        side_buy: bool,
        price: u64,
        size: u64,
        /// Unix seconds divided by 5 (5-second dedup bucket).
        coarse_ts_bucket: u64,
    },
}

/// Derive a `DedupKey` from an `RN1Signal`.
///
/// Prefers the `source_order_id`/`source_seq` primary key. Falls back to the
/// observable-field key when neither is present.
pub fn key_for_signal(signal: &RN1Signal) -> DedupKey {
    if signal.source_order_id.is_some() || signal.source_seq.is_some() {
        DedupKey::Ordered {
            id: signal
                .source_order_id
                .clone()
                .unwrap_or_else(|| signal.order_id.clone()),
            seq: signal.source_seq.unwrap_or(0),
        }
    } else {
        let coarse_ts_bucket = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 5;
        DedupKey::Fallback {
            token_id: signal.token_id.clone(),
            side_buy: matches!(signal.side, OrderSide::Buy),
            price: signal.price,
            size: signal.size,
            coarse_ts_bucket,
        }
    }
}

// ─── IngressDedup ─────────────────────────────────────────────────────────────

/// Thread-safe bounded LRU deduplicator with per-entry TTL.
pub struct IngressDedup {
    cache: Mutex<LruCache<DedupKey, Instant>>,
}

impl Default for IngressDedup {
    fn default() -> Self {
        Self::new()
    }
}

impl IngressDedup {
    /// Creates a new deduplicator with the default capacity and TTL.
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(DEDUP_CAPACITY).expect("non-zero capacity"),
            )),
        }
    }

    /// Returns `true` if the key is new (signal should be processed).
    /// Returns `false` if the key is a duplicate within the TTL window (signal should be dropped).
    ///
    /// On a fresh or TTL-expired key: inserts into cache and returns `true`.
    /// On a live duplicate: returns `false` without modifying the cache.
    pub fn check_and_insert(&self, key: &DedupKey) -> bool {
        let now = Instant::now();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&inserted_at) = cache.peek(key) {
            if now.duration_since(inserted_at) < DEDUP_TTL {
                return false; // live duplicate
            }
            // TTL expired — re-insert below
        }
        cache.put(key.clone(), now);
        true
    }
}
