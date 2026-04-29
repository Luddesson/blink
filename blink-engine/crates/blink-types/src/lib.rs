//! Canonical event, intent, and journal types for Blink v2.
//!
//! This crate is the **single source of truth** for the shape of data
//! flowing through the rebuilt pipeline:
//!
//! ```text
//!  ingress → RawEvent → decision → Intent → submit → DecisionOutcome
//!                     └──────── StageTimestamps ────────┘
//! ```
//!
//! Every row written to the decision journal is derivable from this crate's
//! types; every replay / shadow diff reads only these types. Changing their
//! layout without bumping [`SCHEMA_VERSION`] is a bug.
//!
//! # Design principles
//!
//! - **Integer-only prices and sizes.** Matches the existing engine's
//!   "× 1 000" convention — prevents float drift across replays.
//! - **`Copy` wherever possible.** Every type is `Copy` or cheaply
//!   cloneable; no allocations on the hot path.
//! - **Stable wire layout.** `repr(u8)` on every public enum so journal
//!   rows and shadow diffs are format-stable across compiler versions.
//! - **Opt-out serde.** `serde` is on by default but gated behind a
//!   feature; core types compile with zero derives for the benches.
//!
//! # Versioning
//!
//! The journal and shadow runner both inspect [`SCHEMA_VERSION`] on
//! startup; mismatched producer/consumer pairs must refuse to run.
//! Additive changes (new optional fields) bump the version; breaking
//! changes bump the major component.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU64, Ordering};

use blink_timestamps::Timestamp;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// ─── Versioning ───────────────────────────────────────────────────────────

/// Schema version of every type exposed by this crate.
///
/// Written into every decision-journal row and checked by the shadow
/// runner at start-up. Incremented on every **breaking** change to a
/// public type's wire layout; additive changes bump [`SCHEMA_MINOR`].
pub const SCHEMA_VERSION: u32 = 1;

/// Minor version for additive/compatible changes. Reset to 0 on every
/// [`SCHEMA_VERSION`] bump.
///
/// * `1` — additive: `RawEvent.observe_only` field + [`SourceKind::MempoolCtf`]
///   variant introduced by the `blink-ingress` Phase-2 rewrite. Old readers
///   without `#[serde(default)]` awareness will deserialize new rows with
///   missing-field errors; the journal writer MUST continue to emit rows
///   tagged `schema_version = 1, schema_minor = 1` until downstream
///   consumers are bumped.
/// * `2` — additive: `RawEvent.maker_wallet: Option<[u8; 20]>` populated by
///   `BlockchainLogsSource` (Phase-5 flow signals). Other sources leave it
///   as `None`. Serde-defaulted so older journal rows deserialize cleanly.
pub const SCHEMA_MINOR: u32 = 2;

/// Human-readable schema tag used in journal file names and CH table
/// partitioning.
pub const SCHEMA_NAME: &str = "blink.journal.v1";

// ─── Identifiers ──────────────────────────────────────────────────────────

/// Per-process monotonic event identifier.
///
/// **Not globally unique across processes.** Pair with `code_git_sha` +
/// `process_start_ns` in the journal for cross-run uniqueness.
///
/// Uses an `AtomicU64` internally; fetch cost is a single relaxed
/// increment (~3 cycles uncontended). The counter is not reset on
/// `fetch_next` wraparound — at 100k events/s it takes ~5.8 million years
/// to wrap, so we treat wraparound as unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct EventId(pub u64);

impl EventId {
    /// Generate a fresh event id for this process.
    #[inline(always)]
    pub fn fetch_next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Raw integer value.
    #[inline(always)]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A 32-byte hash summarising the content of an [`Intent`], used to diff
/// the legacy and v2 decision kernels in the shadow runner without leaking
/// any sensitive fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IntentHash(pub [u8; 32]);

// ─── Price / size wrappers ────────────────────────────────────────────────

/// Price in ticks (Polymarket convention: probability × 1 000 ∈ [1, 999]).
///
/// Wrapper exists to prevent mixing with raw size integers at the type
/// level — a common source of bugs in the legacy code path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct PriceTicks(pub u64);

/// Size in USDC micro-units (size × 1 000 for Polymarket's 6-dp convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct SizeU(pub u64);

// ─── Enumerations ─────────────────────────────────────────────────────────

/// Trade direction. Wire layout is `u8` for journal stability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum Side {
    /// Taking or resting bid.
    Buy = 0,
    /// Taking or resting offer.
    Sell = 1,
}

/// Time-in-force. Wire layout is `u8` for journal stability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum TimeInForce {
    /// Good-till-cancelled; classical maker.
    Gtc = 0,
    /// Fill-or-kill; full fill at exact price or immediate cancel.
    Fok = 1,
    /// Fill-and-kill (IOC); partial fill then cancel remainder.
    Fak = 2,
}

/// Ingress source that produced a [`RawEvent`]. `u8` wire repr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum SourceKind {
    /// Legacy RN1 REST poller (`rn1_poller.rs`). Retired in Phase 2.
    Rn1Rest = 0,
    /// Polygon mempool tap for RN1's unconfirmed txs. Phase 2 primary.
    Rn1Mempool = 1,
    /// Bullpen WebSocket smart-money feed.
    BullpenWs = 2,
    /// Canonical post-match `OrderFilled` event from the CTF exchange.
    CtfLog = 3,
    /// Polymarket CLOB L2 firehose.
    ClobWs = 4,
    /// Operator-injected signal (for tests and manual intervention).
    Manual = 5,
    /// Generic Polygon mempool observation of a CTF-exchange-bound
    /// transaction (not RN1-wallet-specific). Emitted by
    /// `blink_ingress::MempoolSource`; decisions derived from this variant
    /// MUST respect the `observe_only` flag on the accompanying
    /// [`RawEvent`].
    MempoolCtf = 6,
    /// Blockchain `eth_subscribe("logs", ...)` stream for CTF contract
    /// events. Emitted by `blink_ingress::BlockchainLogsSource`. Payload
    /// bytes (topics + data) live in [`RawEvent::extra`].
    BlockchainLogs = 7,
}

/// Reason a decision was aborted or skipped. Mirrors the legacy
/// `pretrade_gate::GateDecision` plus risk/dedup reasons. `u8` wire repr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum AbortReason {
    /// Book snapshot older than the gate's freshness budget.
    StaleBook = 0,
    /// Limit price diverged from the reference by more than `bps`.
    Drift = 1,
    /// Post-only order would have crossed the book.
    PostOnlyCross = 2,
    /// Per-market or per-account risk limit tripped.
    RiskLimit = 3,
    /// Dedup cache hit — duplicate of an earlier event.
    DuplicateDedup = 4,
    /// Circuit breaker open for this market or globally.
    CircuitOpen = 5,
    /// Signer / vault returned an error (retry-safe).
    SignerError = 6,
    /// Submitter exhausted retry budget.
    SubmitError = 7,
}

// ─── Raw ingress event ────────────────────────────────────────────────────

/// On-chain source anchor — `(tx_hash, log_index)` uniquely identifies any
/// CTF event and makes deduplication forgery-proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct OnChainAnchor {
    /// Keccak256 transaction hash.
    pub tx_hash: [u8; 32],
    /// Log index within the transaction, or `u32::MAX` for mempool/pending
    /// events that have no log index yet.
    pub log_index: u32,
}

/// A signal arriving at the engine's ingress boundary.
///
/// Intentionally flat (not a tagged enum) so the hot path can switch on
/// `source` without a branch per payload shape. Source-specific payloads
/// that don't fit the common fields go into [`RawEvent::extra`] as an
/// opaque payload, parsed lazily by the strategies that care.
///
/// Size: 152 bytes on x86_64 (verified in tests) — fits in two cache
/// lines, no indirections on the hot path beyond the optional extra payload.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RawEvent {
    /// Process-local monotonic id, generated at ingress.
    pub event_id: EventId,
    /// Producer.
    pub source: SourceKind,
    /// Monotonic upstream sequence if the source emits one
    /// (e.g. websocket seq, block number). `u64::MAX` if unknown.
    pub source_seq: u64,
    /// On-chain `(tx_hash, log_index)` for the forgery-proof dedup key.
    /// `None` for pure-offchain sources (e.g. `ClobWs` book updates).
    pub anchor: Option<OnChainAnchor>,
    /// Polymarket token id (hex). Empty for events that don't target a
    /// specific token (e.g. wallet-level pings).
    pub token_id: String,
    /// Polymarket condition id / market id.
    pub market_id: Option<String>,
    /// Direction implied by the event, if applicable.
    pub side: Option<Side>,
    /// Observed or implied price.
    pub price: Option<PriceTicks>,
    /// Observed or implied size (USDC µunits).
    pub size: Option<SizeU>,
    /// TSC timestamp stamped at the ingress boundary. The `tsc_in` of the
    /// accompanying [`StageTimestamps`] is set from this value.
    pub tsc_in: Timestamp,
    /// Wall-clock nanoseconds since Unix epoch captured at ingress. Used
    /// for journal sanity (detecting TSC backward jumps during VM
    /// migration — see plan risk R-4) and for human-readable journals.
    pub wall_ns: u64,
    /// Source-specific opaque payload (ABI-decoded calldata, raw JSON
    /// slice, etc.). Strategies that need more than the common fields
    /// parse this on demand. `None` for the common sources in steady
    /// state — avoid putting things here that every decision needs.
    pub extra: Option<Box<[u8]>>,
    /// Gating flag for mempool-derived events. When `true` the decision
    /// kernel MUST NOT submit an order — the event is for passive
    /// observation only (see `docs/rebuild/R3_LEGAL_MEMO_STUB.md`).
    /// Always `false` for non-mempool sources.
    ///
    /// Additive in `SCHEMA_MINOR = 1`. Older journal rows deserialize
    /// with this flag defaulted to `false`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub observe_only: bool,
    /// Maker (order-signer) wallet for on-chain trade events, when the
    /// source can extract it from the log topics.
    ///
    /// Populated by [`SourceKind::BlockchainLogs`] when the log topic layout
    /// carries a zero-padded 20-byte address. All other sources emit `None`.
    ///
    /// Consumed by the `blink-flows` cohort signal to classify "who is
    /// trading" without needing to re-parse `extra`.
    ///
    /// Additive in `SCHEMA_MINOR = 2`. Older journal rows deserialize
    /// with this field defaulted to `None`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub maker_wallet: Option<[u8; 20]>,
}

impl RawEvent {
    /// Construct a minimal ingress event with all optional fields unset.
    ///
    /// Intended for unit tests and for ingress adapters that fill in the
    /// fields they have and leave the rest `None`. Production ingress
    /// paths should always set `anchor` where available to keep dedup
    /// strong.
    #[inline]
    pub fn minimal(source: SourceKind, token_id: String, tsc_in: Timestamp) -> Self {
        Self {
            event_id: EventId::fetch_next(),
            source,
            source_seq: u64::MAX,
            anchor: None,
            token_id,
            market_id: None,
            side: None,
            price: None,
            size: None,
            tsc_in,
            wall_ns: wall_clock_ns(),
            extra: None,
            observe_only: false,
            maker_wallet: None,
        }
    }
}

// ─── Stage timestamps ─────────────────────────────────────────────────────

/// Per-event stage timestamps, one TSC read per named boundary.
///
/// Every field is optional because not all events traverse all stages
/// (e.g. a `DuplicateDedup` abort never reaches `tsc_sign`). Absence is
/// encoded as `Timestamp(u64)` raw-value 0 rather than `Option` so the
/// struct is `Copy` and journal rows have a fixed shape.
///
/// Consumers distinguish "not reached" from "reached at t=0 (unreachable
/// in practice)" by checking `raw() == 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct StageTimestamps {
    /// Stamped by the ingress adapter.
    pub tsc_in: Timestamp,
    /// After parse + normalise into `RawEvent`.
    pub tsc_parse: Timestamp,
    /// After dedup + classify.
    pub tsc_classify: Timestamp,
    /// After the decision kernel produces an `Intent` (or decides not to).
    pub tsc_decide: Timestamp,
    /// After risk + pretrade gate.
    pub tsc_risk: Timestamp,
    /// After EIP-712 sign.
    pub tsc_sign: Timestamp,
    /// After the submit frame is handed to the I/O driver.
    pub tsc_submit: Timestamp,
    /// On ack from the venue (HTTP 2xx or first `OrderFilled` log,
    /// whichever arrives first for the same `client_order_id`).
    pub tsc_ack: Timestamp,
}

impl StageTimestamps {
    /// All stages unstamped (raw = 0).
    pub const UNSET: Self = Self {
        tsc_in: Timestamp::UNSET,
        tsc_parse: Timestamp::UNSET,
        tsc_classify: Timestamp::UNSET,
        tsc_decide: Timestamp::UNSET,
        tsc_risk: Timestamp::UNSET,
        tsc_sign: Timestamp::UNSET,
        tsc_submit: Timestamp::UNSET,
        tsc_ack: Timestamp::UNSET,
    };

    /// Build from the ingress timestamp only; other stages start as UNSET.
    #[inline]
    pub fn starting_at(tsc_in: Timestamp) -> Self {
        Self {
            tsc_in,
            ..Self::UNSET
        }
    }
}

// ─── Intent ───────────────────────────────────────────────────────────────

/// Canonical trading intent emitted by the decision kernel.
///
/// The intent is the **pure output** of the kernel: no network state, no
/// signing key material, no side-effects. Downstream stages (risk, sign,
/// submit) derive a concrete `Order` from it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Intent {
    /// Originating event (for the journal + cancel correlation).
    pub event_id: EventId,
    /// Polymarket token id.
    pub token_id: String,
    /// Market id, always set on Intent (kernel must resolve from
    /// `RawEvent.market_id` + book snapshot).
    pub market_id: String,
    /// Direction.
    pub side: Side,
    /// Limit price. Kernel is responsible for any reprice-to-market logic
    /// (see plan §1 option 2 for the existing drift-abort problem).
    pub price: PriceTicks,
    /// Size (USDC µunits).
    pub size: SizeU,
    /// Time-in-force.
    pub tif: TimeInForce,
    /// Post-only flag. Honoured by the pretrade gate.
    pub post_only: bool,
    /// Engine-generated client order id (<= 64 ASCII bytes). Used for
    /// idempotent submit-recovery (see plan §2 submitter notes).
    pub client_order_id: String,
}

// ─── Decision outcome (journal row body) ──────────────────────────────────

/// Final disposition of a single [`RawEvent`]. Exactly one variant is
/// written to the decision journal per event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DecisionOutcome {
    /// Decision kernel chose not to act.
    NoOp {
        /// Why (e.g. "below edge threshold", "inventory saturated").
        ///
        /// Stored as `String` for deserializability; producers typically
        /// pass `&'static str` via `.to_string()` / `.into()` — a single
        /// allocation per NoOp outcome, off the critical submit path.
        reason: String,
    },
    /// Aborted by risk, pretrade gate, or circuit breaker.
    Aborted {
        /// Structured reason — see [`AbortReason`].
        reason: AbortReason,
        /// For `Drift`, the measured bps.
        metric: Option<i64>,
    },
    /// Signed and submitted; ack pending or received.
    Submitted {
        /// Deterministic hash of the [`Intent`] used for shadow parity.
        intent_hash: IntentHash,
        /// Engine-generated id echoed to the CLOB.
        client_order_id: String,
    },
}

// ─── Journal row ──────────────────────────────────────────────────────────

/// One row of the decision journal. Append-only, one per [`RawEvent`].
///
/// The physical ClickHouse schema can evolve independently (columns may
/// be added / renamed / back-filled); this struct is the **logical**
/// contract that producers (engine, shadow runner) and consumers (replay,
/// CH ingestor) agree on.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct JournalRow {
    /// Schema version at write time. Consumers refuse to ingest rows
    /// from a version they don't recognise.
    pub schema_version: u32,
    /// Schema minor (additive bump, not a breaking change).
    pub schema_minor: u32,
    /// Event identity.
    pub event_id: EventId,
    /// Ingress source.
    pub source: SourceKind,
    /// On-chain anchor where available.
    pub anchor: Option<OnChainAnchor>,
    /// Token / market.
    pub token_id: String,
    /// Market id if resolved.
    pub market_id: Option<String>,
    /// Stage latencies.
    pub stages: StageTimestamps,
    /// Wall-clock ns at ingress (detects TSC jumps / VM migration — R-4).
    pub wall_ns: u64,
    /// Running git SHA of the engine binary (needed for R-6 replay).
    pub code_git_sha: [u8; 20],
    /// Stable hash of the active configuration at decision time.
    pub config_hash: [u8; 8],
    /// The outcome.
    pub outcome: DecisionOutcome,
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Nanoseconds since the Unix epoch (`CLOCK_REALTIME`). Used only for
/// journal sanity fields, never for hot-path measurement.
#[inline]
pub fn wall_clock_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blink_timestamps::{init_with_policy, InitPolicy};

    fn ts() -> Timestamp {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        Timestamp::now()
    }

    #[test]
    fn event_id_is_monotonic() {
        let a = EventId::fetch_next();
        let b = EventId::fetch_next();
        assert!(b.raw() > a.raw());
    }

    #[test]
    fn raw_event_minimal_has_defaults() {
        let ev = RawEvent::minimal(SourceKind::Rn1Mempool, "0xabc".into(), ts());
        assert_eq!(ev.source, SourceKind::Rn1Mempool);
        assert_eq!(ev.source_seq, u64::MAX);
        assert!(ev.anchor.is_none());
        assert!(ev.side.is_none());
        assert!(ev.wall_ns > 0);
    }

    #[test]
    fn stage_timestamps_unset_is_all_zero() {
        let s = StageTimestamps::UNSET;
        assert_eq!(s.tsc_in.raw(), 0);
        assert_eq!(s.tsc_ack.raw(), 0);
    }

    #[test]
    fn stage_timestamps_starting_at_preserves_in() {
        let t = ts();
        let s = StageTimestamps::starting_at(t);
        assert_eq!(s.tsc_in, t);
        assert_eq!(s.tsc_parse.raw(), 0);
    }

    #[test]
    fn schema_version_is_one() {
        assert_eq!(SCHEMA_VERSION, 1);
        assert_eq!(SCHEMA_NAME, "blink.journal.v1");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn intent_round_trips_json() {
        let intent = Intent {
            event_id: EventId(42),
            token_id: "0xdead".into(),
            market_id: "0xbeef".into(),
            side: Side::Buy,
            price: PriceTicks(650),
            size: SizeU(1_500_000),
            tif: TimeInForce::Gtc,
            post_only: true,
            client_order_id: "blk-1".into(),
        };
        let s = serde_json::to_string(&intent).unwrap();
        let back: Intent = serde_json::from_str(&s).unwrap();
        assert_eq!(intent, back);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn abort_reason_tag_is_stable() {
        // Journal stability depends on these wire numbers NOT changing.
        assert_eq!(AbortReason::StaleBook as u8, 0);
        assert_eq!(AbortReason::Drift as u8, 1);
        assert_eq!(AbortReason::PostOnlyCross as u8, 2);
        assert_eq!(AbortReason::RiskLimit as u8, 3);
        assert_eq!(AbortReason::DuplicateDedup as u8, 4);
        assert_eq!(AbortReason::CircuitOpen as u8, 5);
        assert_eq!(AbortReason::SignerError as u8, 6);
        assert_eq!(AbortReason::SubmitError as u8, 7);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn source_kind_tag_is_stable() {
        assert_eq!(SourceKind::Rn1Rest as u8, 0);
        assert_eq!(SourceKind::Rn1Mempool as u8, 1);
        assert_eq!(SourceKind::BullpenWs as u8, 2);
        assert_eq!(SourceKind::CtfLog as u8, 3);
        assert_eq!(SourceKind::ClobWs as u8, 4);
        assert_eq!(SourceKind::Manual as u8, 5);
        assert_eq!(SourceKind::MempoolCtf as u8, 6);
        assert_eq!(SourceKind::BlockchainLogs as u8, 7);
    }
}
