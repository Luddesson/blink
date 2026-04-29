//! # blink-book
//!
//! Canonical top-of-book and per-token position snapshots plus the
//! ArcSwap-backed stores that the decision kernel reads on the hot path.
//!
//! ## Design goals
//!
//! * **Wait-free reads.** `BookStore::latest` and `BookStore::load_fast`
//!   perform at most one atomic RMW (the `Arc` clone inside `ArcSwap::load`
//!   or nothing at all for `load_fast`). Readers never allocate, never
//!   block, and never spin.
//! * **Lock-free writes.** `BookStore::upsert` publishes a new snapshot via
//!   a single atomic store on the per-token `ArcSwap`. Contention between
//!   writers and readers is resolved by the ArcSwap machinery rather than
//!   any lock this crate introduces.
//! * **Zero heap indirection inside snapshots.** `BookSnapshot` carries its
//!   ladder as a fixed-size inline array (`[Level; BOOK_DEPTH]`) so a
//!   reader touches at most a handful of cache lines. The snapshot itself
//!   lives behind an `Arc` — that's the one indirection — but every field
//!   you read after following the `Arc` is inline.
//!
//! `BookSnapshot` is *not* `Copy`: the `[Level; 16]` ladder makes it ~700
//! bytes, well above the threshold where implicit copies would be a
//! footgun. It is `Clone` and, more importantly, published through an
//! `Arc<ArcSwap<BookSnapshot>>` so consumers share a single heap allocation
//! per published version.
//!
//! ## TokenId / MarketId
//!
//! `blink-types` models these as owned `String`s on `RawEvent`
//! (hex-encoded Polymarket token id, condition id). This crate mirrors
//! that choice with local type aliases `TokenId` / `MarketId` rather than
//! introducing a new nominal type in `blink-types`. Swap for
//! `Arc<str>` / a small-string type later without changing the shape of
//! the public API.

#![forbid(unsafe_code)]

use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};
use dashmap::DashMap;

pub use blink_timestamps::Timestamp;

/// Polymarket token id (hex string). Mirrors `RawEvent::token_id` in
/// `blink-types`.
pub type TokenId = String;

/// Polymarket condition / market id (hex string). Mirrors
/// `RawEvent::market_id` in `blink-types`.
pub type MarketId = String;

/// Ladder depth carried in every published snapshot. Sized to comfortably
/// cover top-of-book plus a few levels of context while keeping the
/// snapshot within a small, predictable number of cache lines.
pub const BOOK_DEPTH: usize = 16;

// ─── Plain-data snapshots ─────────────────────────────────────────────────

/// One side of the ladder. `levels[0 .. len as usize]` is meaningful;
/// entries past `len` are zeroed padding and must be ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LadderSide {
    /// Number of populated levels (`≤ BOOK_DEPTH`).
    pub len: u8,
    /// Fixed-capacity inline ladder. No heap indirection.
    pub levels: [Level; BOOK_DEPTH],
}

impl LadderSide {
    /// An empty ladder side (`len = 0`, zeroed levels).
    pub const EMPTY: Self = Self {
        len: 0,
        levels: [Level::ZERO; BOOK_DEPTH],
    };

    /// Construct a ladder from a slice of levels (truncated to `BOOK_DEPTH`).
    #[inline]
    pub fn from_slice(src: &[Level]) -> Self {
        let mut out = Self::EMPTY;
        let n = src.len().min(BOOK_DEPTH);
        out.levels[..n].copy_from_slice(&src[..n]);
        out.len = n as u8;
        out
    }

    /// Slice over the populated prefix.
    #[inline]
    pub fn as_slice(&self) -> &[Level] {
        &self.levels[..self.len as usize]
    }

    /// Top level (best price on this side), if any.
    #[inline]
    pub fn top(&self) -> Option<Level> {
        if self.len == 0 {
            None
        } else {
            Some(self.levels[0])
        }
    }
}

impl Default for LadderSide {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// A single ladder level. Prices are Polymarket ticks (price × 1000),
/// sizes are USDC µ-units (USDC × 1_000_000 on-chain scale, captured here
/// at the 1e-3 `µUSDC` resolution the engine standardises on — see plan
/// §1 "scale").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Level {
    pub price_ticks: u32,
    pub size_u_usdc: u64,
}

impl Level {
    /// All-zero level — used as padding inside [`LadderSide::levels`].
    pub const ZERO: Self = Self {
        price_ticks: 0,
        size_u_usdc: 0,
    };

    #[inline]
    pub const fn new(price_ticks: u32, size_u_usdc: u64) -> Self {
        Self {
            price_ticks,
            size_u_usdc,
        }
    }
}

/// Top-of-book snapshot for a single token.
///
/// ~700 bytes on x86_64 (two `LadderSide` × ~344 B each + headers).
/// **Not** `Copy` — always move/clone explicitly. Published through
/// `Arc<ArcSwap<BookSnapshot>>` so readers share a single heap allocation
/// per version.
#[derive(Debug, Clone)]
pub struct BookSnapshot {
    pub token_id: TokenId,
    pub market_id: MarketId,
    /// Monotonic source-set sequence (e.g. websocket `seq` field).
    pub seq: u64,
    /// Wall-clock nanoseconds as reported by the source at update time.
    pub source_wall_ns: u64,
    /// TSC-timestamp captured when ingress received this update.
    pub tsc_received: Timestamp,
    pub bid: LadderSide,
    pub ask: LadderSide,
}

impl BookSnapshot {
    /// Construct an empty snapshot for the given token. Useful as a
    /// placeholder during warmup and in tests.
    pub fn empty(token_id: TokenId, market_id: MarketId) -> Self {
        Self {
            token_id,
            market_id,
            seq: 0,
            source_wall_ns: 0,
            tsc_received: Timestamp::UNSET,
            bid: LadderSide::EMPTY,
            ask: LadderSide::EMPTY,
        }
    }
}

/// Per-token position + risk-meter snapshot. Small (fits in one cache
/// line) and carries no heap indirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionSnapshot {
    /// Token this position belongs to. Stored as a hash-friendly id; the
    /// raw hex token id is the map key in [`PositionStore`]. Duplicated
    /// here so a consumer holding only the snapshot can identify it.
    pub token_id_hash: u64,
    pub open_notional_u_usdc_abs: u64,
    pub open_qty_signed_u: i64,
    pub realized_pnl_u_usdc: i64,
    pub recent_abort_count_1s: u16,
    /// Wall-clock nanosecond deadline until which new orders on this
    /// token must be vetoed. `0` = no active cooldown.
    pub cooldown_until_ns: u64,
    pub seq: u64,
}

impl PositionSnapshot {
    /// Zero position, no cooldown, no aborts.
    pub const fn zero(token_id_hash: u64) -> Self {
        Self {
            token_id_hash,
            open_notional_u_usdc_abs: 0,
            open_qty_signed_u: 0,
            realized_pnl_u_usdc: 0,
            recent_abort_count_1s: 0,
            cooldown_until_ns: 0,
            seq: 0,
        }
    }

    /// Is the position currently in a cooldown window at `now_wall_ns`?
    #[inline]
    pub fn in_cooldown(&self, now_wall_ns: u64) -> bool {
        now_wall_ns < self.cooldown_until_ns
    }
}

const _: () = {
    // Keep PositionSnapshot small (≤ 64 B cache line) — this assert
    // fires at compile time if a field bloats the type.
    assert!(std::mem::size_of::<PositionSnapshot>() <= 64);
};

// ─── Staleness helper ─────────────────────────────────────────────────────

/// A snapshot is *stale* when its source wall clock falls more than
/// `max_age_ns` behind `now_wall_ns`. Boundary semantics: exactly
/// `max_age_ns` old ⇒ not stale (inclusive).
///
/// If `snap.source_wall_ns` is in the future relative to `now_wall_ns`
/// (clock skew) the snapshot is treated as fresh.
#[inline]
pub fn is_stale(snap: &BookSnapshot, now_wall_ns: u64, max_age_ns: u64) -> bool {
    now_wall_ns.saturating_sub(snap.source_wall_ns) > max_age_ns
}

// ─── BookStore ────────────────────────────────────────────────────────────

/// Wait-free reader / lock-free writer store of per-token book snapshots.
///
/// Internally a [`DashMap`] keyed by `TokenId` whose values are
/// `Arc<ArcSwap<BookSnapshot>>`. The outer map only mutates on the rare
/// "first snapshot for this token" event; steady-state publishes swap the
/// inner `ArcSwap` with no map-level contention.
#[derive(Default)]
pub struct BookStore {
    inner: DashMap<TokenId, Arc<ArcSwap<BookSnapshot>>>,
}

impl BookStore {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    /// Publish a new snapshot. O(1) steady-state (one atomic store on
    /// the per-token `ArcSwap`); the first publish for a token takes the
    /// DashMap write path.
    pub fn upsert(&self, snap: BookSnapshot) {
        let key = snap.token_id.clone();
        let arc = Arc::new(snap);
        if let Some(entry) = self.inner.get(&key) {
            entry.store(arc);
            return;
        }
        self.inner
            .entry(key)
            .or_insert_with(|| Arc::new(ArcSwap::from(Arc::clone(&arc))))
            .store(arc);
    }

    /// Read the most recent snapshot for `token`. Returns
    /// `Option<Arc<BookSnapshot>>` — an `Arc` clone (one atomic RMW) is
    /// cheap enough for the hot path but if you need the absolute
    /// minimum overhead prefer [`Self::load_fast`].
    #[inline]
    pub fn latest(&self, token: &TokenId) -> Option<Arc<BookSnapshot>> {
        self.inner.get(token).map(|e| e.load_full())
    }

    /// Borrow-only fast path: returns an [`arc_swap::Guard`] wrapping
    /// the current snapshot without performing the `Arc` clone.
    ///
    /// # Caveats
    ///
    /// * The returned `Guard` is `!Send` — it holds thread-local state.
    /// * Must be dropped before any `.await` point; hold only across
    ///   synchronous code.
    /// * Holding the guard pins the current snapshot in memory; publishers
    ///   can still install new ones without waiting for you, but the
    ///   version you're reading won't be freed until you drop it.
    #[inline]
    pub fn load_fast(&self, token: &TokenId) -> Option<Guard<Arc<BookSnapshot>>> {
        self.inner.get(token).map(|e| e.load())
    }

    /// Number of tokens with at least one published snapshot.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Diagnostic snapshot of known tokens — clones every key, so
    /// **not** for the hot path.
    pub fn iter_markets(&self) -> Vec<TokenId> {
        self.inner.iter().map(|e| e.key().clone()).collect()
    }
}

// ─── PositionStore ────────────────────────────────────────────────────────

/// Same ArcSwap pattern as [`BookStore`], but keyed by [`TokenId`] for
/// per-token position + risk-meter snapshots.
#[derive(Default)]
pub struct PositionStore {
    inner: DashMap<TokenId, Arc<ArcSwap<PositionSnapshot>>>,
}

impl PositionStore {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    /// Publish a new snapshot unconditionally.
    pub fn upsert(&self, token: TokenId, snap: PositionSnapshot) {
        let arc = Arc::new(snap);
        if let Some(entry) = self.inner.get(&token) {
            entry.store(arc);
            return;
        }
        self.inner
            .entry(token)
            .or_insert_with(|| Arc::new(ArcSwap::from(Arc::clone(&arc))))
            .store(arc);
    }

    #[inline]
    pub fn latest(&self, token: &TokenId) -> Option<Arc<PositionSnapshot>> {
        self.inner.get(token).map(|e| e.load_full())
    }

    #[inline]
    pub fn load_fast(&self, token: &TokenId) -> Option<Guard<Arc<PositionSnapshot>>> {
        self.inner.get(token).map(|e| e.load())
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn iter_tokens(&self) -> Vec<TokenId> {
        self.inner.iter().map(|e| e.key().clone()).collect()
    }

    /// Apply a fill to the current snapshot for `token`, publishing a new
    /// snapshot. The mutation is `read-current → build-new → store` so
    /// concurrent callers may race; callers serialise fills for a given
    /// token upstream (the plan's risk kernel owns this).
    ///
    /// `qty_delta_u` is signed (positive ⇒ longer, negative ⇒ shorter).
    /// `notional_delta_u` is added to `open_notional_u_usdc_abs`'s
    /// magnitude; callers compute the absolute contribution for the
    /// leg and pass it unsigned.
    pub fn apply_fill(
        &self,
        token: &TokenId,
        qty_delta_u: i64,
        notional_delta_u_abs: u64,
        realized_pnl_delta_u: i64,
    ) {
        let (entry, current) = self.load_or_init(token);
        let mut next = current;
        next.open_qty_signed_u = next.open_qty_signed_u.saturating_add(qty_delta_u);
        next.open_notional_u_usdc_abs = next
            .open_notional_u_usdc_abs
            .saturating_add(notional_delta_u_abs);
        next.realized_pnl_u_usdc = next.realized_pnl_u_usdc.saturating_add(realized_pnl_delta_u);
        next.seq = next.seq.wrapping_add(1);
        entry.store(Arc::new(next));
    }

    /// Record an abort. Bumps the recent-abort counter and arms the
    /// cooldown window to `now_wall_ns + cooldown_ms * 1_000_000`.
    pub fn apply_abort(&self, token: &TokenId, now_wall_ns: u64, cooldown_ms: u64) {
        let (entry, current) = self.load_or_init(token);
        let mut next = current;
        next.recent_abort_count_1s = next.recent_abort_count_1s.saturating_add(1);
        let cooldown_ns = cooldown_ms.saturating_mul(1_000_000);
        let new_deadline = now_wall_ns.saturating_add(cooldown_ns);
        if new_deadline > next.cooldown_until_ns {
            next.cooldown_until_ns = new_deadline;
        }
        next.seq = next.seq.wrapping_add(1);
        entry.store(Arc::new(next));
    }

    fn load_or_init(&self, token: &TokenId) -> (Arc<ArcSwap<PositionSnapshot>>, PositionSnapshot) {
        if let Some(e) = self.inner.get(token) {
            let arc = Arc::clone(e.value());
            let snap = **arc.load();
            return (arc, snap);
        }
        let init = PositionSnapshot::zero(hash_token(token));
        let swap = Arc::new(ArcSwap::from(Arc::new(init)));
        let entry = self
            .inner
            .entry(token.clone())
            .or_insert_with(|| Arc::clone(&swap));
        let arc = Arc::clone(entry.value());
        let snap = **arc.load();
        (arc, snap)
    }
}

fn hash_token(token: &TokenId) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write(token.as_bytes());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc as StdArc;
    use std::thread;

    fn mk_snap(tok: &str, seq: u64, wall_ns: u64) -> BookSnapshot {
        let bid = LadderSide::from_slice(&[Level::new(500, 1_000), Level::new(499, 2_000)]);
        let ask = LadderSide::from_slice(&[Level::new(501, 1_500)]);
        BookSnapshot {
            token_id: tok.to_string(),
            market_id: format!("m-{tok}"),
            seq,
            source_wall_ns: wall_ns,
            tsc_received: Timestamp::UNSET,
            bid,
            ask,
        }
    }

    #[test]
    fn upsert_and_latest_multi_token() {
        let s = BookStore::new();
        s.upsert(mk_snap("a", 1, 10));
        s.upsert(mk_snap("b", 2, 20));
        s.upsert(mk_snap("a", 3, 30));

        let a = s.latest(&"a".to_string()).unwrap();
        assert_eq!(a.seq, 3);
        assert_eq!(a.source_wall_ns, 30);
        assert_eq!(a.bid.len, 2);
        assert_eq!(a.ask.top().unwrap().price_ticks, 501);

        let b = s.latest(&"b".to_string()).unwrap();
        assert_eq!(b.seq, 2);
        assert_eq!(s.len(), 2);

        let mut keys = s.iter_markets();
        keys.sort();
        assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn latest_unknown_token_is_none() {
        let s = BookStore::new();
        assert!(s.latest(&"missing".to_string()).is_none());
        assert!(s.load_fast(&"missing".to_string()).is_none());
    }

    #[test]
    fn load_fast_returns_current() {
        let s = BookStore::new();
        s.upsert(mk_snap("x", 7, 100));
        let g = s.load_fast(&"x".to_string()).unwrap();
        assert_eq!(g.seq, 7);
    }

    #[test]
    fn concurrent_writer_readers_converge() {
        let s = StdArc::new(BookStore::new());
        s.upsert(mk_snap("tok", 0, 0));

        let stop = StdArc::new(AtomicBool::new(false));
        let mut handles = Vec::new();

        for _ in 0..2 {
            let s = StdArc::clone(&s);
            let stop = StdArc::clone(&stop);
            handles.push(thread::spawn(move || {
                let key = "tok".to_string();
                let mut last = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    if let Some(snap) = s.latest(&key) {
                        assert!(snap.seq >= last, "seq went backwards");
                        last = snap.seq;
                    }
                }
                last
            }));
        }

        let writer = {
            let s = StdArc::clone(&s);
            thread::spawn(move || {
                for i in 1..=100_000u64 {
                    s.upsert(mk_snap("tok", i, i));
                }
            })
        };

        writer.join().unwrap();
        stop.store(true, Ordering::Relaxed);
        for h in handles {
            let last = h.join().unwrap();
            assert!(last <= 100_000);
        }

        let final_snap = s.latest(&"tok".to_string()).unwrap();
        assert_eq!(final_snap.seq, 100_000);
        assert_eq!(final_snap.source_wall_ns, 100_000);
    }

    #[test]
    fn is_stale_boundaries() {
        let snap = mk_snap("t", 1, 1_000);
        // exactly max_age ⇒ not stale
        assert!(!is_stale(&snap, 1_500, 500));
        // one ns over ⇒ stale
        assert!(is_stale(&snap, 1_501, 500));
        // future wall (skew) ⇒ not stale
        assert!(!is_stale(&snap, 900, 500));
        // same wall ⇒ not stale
        assert!(!is_stale(&snap, 1_000, 0));
    }

    #[test]
    fn position_apply_abort_sets_cooldown() {
        let s = PositionStore::new();
        let tok = "pos-tok".to_string();
        s.apply_abort(&tok, 1_000_000_000, 50); // 50ms cooldown
        let snap = s.latest(&tok).unwrap();
        assert_eq!(snap.recent_abort_count_1s, 1);
        assert_eq!(snap.cooldown_until_ns, 1_000_000_000 + 50 * 1_000_000);
        assert!(snap.in_cooldown(1_049_000_000));
        assert!(!snap.in_cooldown(1_050_000_000));

        // second abort extends (max) the cooldown
        s.apply_abort(&tok, 1_020_000_000, 100);
        let snap = s.latest(&tok).unwrap();
        assert_eq!(snap.recent_abort_count_1s, 2);
        assert_eq!(snap.cooldown_until_ns, 1_020_000_000 + 100 * 1_000_000);
    }

    #[test]
    fn position_apply_fill_accumulates() {
        let s = PositionStore::new();
        let tok = "fill-tok".to_string();
        s.apply_fill(&tok, 1_000, 500, 0);
        s.apply_fill(&tok, -200, 100, 25);
        let snap = s.latest(&tok).unwrap();
        assert_eq!(snap.open_qty_signed_u, 800);
        assert_eq!(snap.open_notional_u_usdc_abs, 600);
        assert_eq!(snap.realized_pnl_u_usdc, 25);
        assert_eq!(snap.seq, 2);
    }
}
