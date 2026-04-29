//! [`DecisionInput`] — the deterministic capture record fed to both
//! kernels by the parity-replay harness.
//!
//! The rubber-duck review made the point that replay **cannot** be
//! reconstructed from [`blink_types::JournalRow`] alone: the journal
//! records the *outcome* of a decision but not the *inputs* the kernel
//! observed (book snapshot, resolved metadata, clock). Those inputs are
//! what we freeze here. Capture from the live engine is follow-up work
//! in `p0-shadow-capture`; for the MVP the self-test and the replay
//! binary construct records by hand or read them from a JSONL file.

use blink_timestamps::Timestamp;
use blink_types::{PriceTicks, RawEvent, SizeU};
use serde::{Deserialize, Serialize};

use crate::kernel::KernelState;

/// Minimal top-of-book snapshot the stub kernels (and, eventually, the
/// real kernels) consume.
///
/// Deliberately compact: richer book state (full ladder, per-level
/// fills, crossed-book detection, etc.) is out of scope for the MVP and
/// lives behind the `p0-shadow-capture` todo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookSnapshot {
    /// Best bid price in ticks. `None` if the book is one-sided.
    pub best_bid_price: Option<PriceTicks>,
    /// Best bid size in USDC µunits.
    pub best_bid_size: Option<SizeU>,
    /// Best ask price in ticks.
    pub best_ask_price: Option<PriceTicks>,
    /// Best ask size in USDC µunits.
    pub best_ask_size: Option<SizeU>,
    /// Age of the snapshot in milliseconds. Used by the `StaleBook`
    /// gate. Captured, not recomputed, to keep replay deterministic.
    pub snapshot_age_ms: u32,
}

/// Metadata resolved at capture time so replay doesn't have to hit the
/// network.
///
/// Rationale: `paper_engine.rs:2791-2962` currently performs live HTTP
/// metadata lookups inside the decision path. Freezing the resolved
/// values here is what lets replay run offline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedMetadata {
    /// Polymarket token id (hex).
    pub token_id: String,
    /// Polymarket condition id / market id.
    pub market_id: String,
    /// Human-readable market title.
    pub title: String,
    /// Outcome label (e.g. "YES" / "NO").
    pub outcome: String,
    /// Venue fees in basis points at decision time.
    pub venue_fees_bps: u16,
}

/// One deterministic replay input.
///
/// The invariant is: given the same [`DecisionInput`] and the same
/// kernel binary, the kernel MUST produce the same [`DecisionOutcome`].
/// Any field that the kernel observes at decision time belongs here;
/// anything the kernel reads from ambient state (wall clock, live book,
/// live config) is a bug the shadow harness exists to catch.
///
/// [`DecisionOutcome`]: blink_types::DecisionOutcome
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionInput {
    /// Stable per-replay-run id. Lets divergence records reference the
    /// specific harness invocation that produced them.
    pub run_id: u64,
    /// Stable key identifying this logical event across runs. The
    /// self-test uses `"event-{i}"`; live capture will use the
    /// `(tx_hash, log_index)` anchor.
    pub event_key: String,
    /// The raw ingress event the kernel saw.
    pub raw_event: RawEvent,
    /// Top-of-book snapshot at decision time.
    pub book_snapshot: BookSnapshot,
    /// Kernel state the kernel started this event with.
    pub kernel_state: KernelState,
    /// Metadata resolved at capture time.
    pub resolved_metadata: ResolvedMetadata,
    /// Hash of the configuration active at decision time. Parity across
    /// differing config hashes is undefined — the runner surfaces this
    /// but does not enforce it for the MVP.
    pub config_hash: [u8; 32],
    /// Injected clock. Kernels MUST read "now" from this field and
    /// never from [`Timestamp::now`] during replay, or parity is lost.
    pub logical_now: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{KernelState, Position};
    use blink_timestamps::{init_with_policy, InitPolicy};
    use blink_types::{RawEvent, SourceKind};

    fn sample() -> DecisionInput {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        DecisionInput {
            run_id: 7,
            event_key: "event-42".into(),
            raw_event: RawEvent::minimal(
                SourceKind::Manual,
                "0xdead".into(),
                Timestamp::UNSET,
            ),
            book_snapshot: BookSnapshot {
                best_bid_price: Some(PriceTicks(4200)),
                best_bid_size: Some(SizeU(1_000_000)),
                best_ask_price: Some(PriceTicks(4210)),
                best_ask_size: Some(SizeU(500_000)),
                snapshot_age_ms: 12,
            },
            kernel_state: KernelState {
                position: Position { net_size_i: 0 },
                cooldown_until: None,
            },
            resolved_metadata: ResolvedMetadata {
                token_id: "0xdead".into(),
                market_id: "0xbeef".into(),
                title: "Will anything happen?".into(),
                outcome: "YES".into(),
                venue_fees_bps: 10,
            },
            config_hash: [1u8; 32],
            logical_now: Timestamp::UNSET,
        }
    }

    #[test]
    fn decision_input_roundtrips_through_json() {
        let input = sample();
        let bytes = serde_json::to_vec(&input).expect("serialize");
        let back: DecisionInput = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(back.run_id, input.run_id);
        assert_eq!(back.event_key, input.event_key);
        assert_eq!(back.book_snapshot, input.book_snapshot);
        assert_eq!(back.resolved_metadata, input.resolved_metadata);
        assert_eq!(back.config_hash, input.config_hash);
    }
}
