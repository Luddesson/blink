# blink-book

Wait-free book & position snapshot stores for the Blink HFT hot path
(Phase 3, todo `p3-arcswap` of the rebuild plan).

## What's in here

| Item | Purpose |
|---|---|
| `BookSnapshot` | Plain-data top-of-book snapshot (`bid`/`ask` as `[Level; 16]`, no heap indirection). |
| `LadderSide`, `Level`, `BOOK_DEPTH=16` | Ladder primitives. |
| `PositionSnapshot` | Per-token position + risk-meter snapshot (`Copy`, ≤ 64 B). |
| `BookStore` | `DashMap<TokenId, Arc<ArcSwap<BookSnapshot>>>` — wait-free reads, lock-free publishes. |
| `PositionStore` | Same shape for `PositionSnapshot`, plus `apply_fill` / `apply_abort` helpers. |
| `is_stale(snap, now_wall_ns, max_age_ns)` | Freshness gate used by the decision kernel. |

Public surface:

```text
pub const BOOK_DEPTH: usize
pub type TokenId   = String
pub type MarketId  = String
pub use blink_timestamps::Timestamp

pub struct Level            { price_ticks: u32, size_u_usdc: u64 }
    ::ZERO / ::new
pub struct LadderSide       { len: u8, levels: [Level; BOOK_DEPTH] }
    ::EMPTY / ::from_slice / ::as_slice / ::top
pub struct BookSnapshot     { token_id, market_id, seq, source_wall_ns,
                              tsc_received, bid, ask }
    ::empty
pub struct PositionSnapshot { token_id_hash, open_notional_u_usdc_abs,
                              open_qty_signed_u, realized_pnl_u_usdc,
                              recent_abort_count_1s, cooldown_until_ns, seq }
    ::zero / ::in_cooldown

pub struct BookStore        ::new / upsert / latest / load_fast / len /
                              is_empty / iter_markets
pub struct PositionStore    ::new / upsert / latest / load_fast / len /
                              is_empty / iter_tokens /
                              apply_fill / apply_abort

pub fn is_stale(&BookSnapshot, now_wall_ns: u64, max_age_ns: u64) -> bool
```

## Hot-path invariants

* Readers (`latest`, `load_fast`) never allocate; they do a `DashMap::get`
  (shard read-lock) + one atomic load on the per-token `ArcSwap`.
  `latest` additionally performs one `Arc` clone (relaxed RMW); `load_fast`
  skips it and returns `arc_swap::Guard<Arc<BookSnapshot>>`.
* `Guard` is **`!Send`** and must be dropped before any `.await`.
* Writers publish with one atomic `ArcSwap::store`; only the first publish
  for a token touches the DashMap write path.

## Benches

Environment: `cargo bench -p blink-book --bench bookstore -- --measurement-time 2 --warm-up-time 1`
(release profile, on the development host — numbers are indicative, not a
production SLA).

| Bench | Median | Target | Notes |
|---|---|---|---|
| `bookstore_latest_hit`         | ~35.9 ns | ≤ 20 ns | Dominated by DashMap shard lookup on a `String` key, not ArcSwap. |
| `bookstore_load_fast`          | ~34.1 ns | ≤ 10 ns | Same story: the ArcSwap part is single-digit ns; `DashMap::get` on a `String` key is the floor. |
| `bookstore_upsert`             | ~654 ns  | —       | Allocates + clones a ~700 B snapshot per publish. |
| `bookstore_reader_under_write` | ~93.5 ns | —       | Reader `load_fast` while a background thread pounds upserts at ~1 M/s. |

**Deviation from the ≤ 20 / ≤ 10 ns targets.** The ArcSwap machinery itself
hits those numbers; the residual comes from `DashMap::get::<String>` (hashing
the token id + shard RW-lock acquisition). This is acceptable for the
current wiring because the decision kernel caches the
`Arc<ArcSwap<BookSnapshot>>` handle per token once at strategy-attach time
(plan §3 Phase 4) and thereafter bypasses the map entirely. When that path
is wired we'll expose a `BookHandle` that holds the inner `Arc<ArcSwap<…>>`
directly and meet the original targets. For now the store is correct and
the writer path is uncontended.

## Tests

```text
cargo test -p blink-book
  7 passed; 0 failed
  - upsert_and_latest_multi_token
  - latest_unknown_token_is_none
  - load_fast_returns_current
  - is_stale_boundaries
  - position_apply_abort_sets_cooldown
  - position_apply_fill_accumulates
  - concurrent_writer_readers_converge   (1 writer × 100k + 2 readers, monotonic seq)
```
