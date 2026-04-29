//! Frozen inputs bundle consumed by [`crate::DecisionKernel::decide`].

use crate::{dedup::RecentKeySet, KernelConfig};
use blink_book::{BookSnapshot, PositionSnapshot};
use blink_types::RawEvent;

/// Everything the kernel reads at decision time, captured atomically
/// **before** the call and borrowed immutably through the call.
///
/// The kernel must never read ambient state (wall clock, global config,
/// live book store) — all of it is reachable through this struct.
pub struct DecisionSnapshot<'a> {
    /// The triggering event.
    pub event: &'a RawEvent,
    /// Book snapshot captured at `event.tsc_in`.
    pub book: &'a BookSnapshot,
    /// Position snapshot for the targeted token.
    pub position: &'a PositionSnapshot,
    /// Recent semantic-key ring for allocation-free dedup.
    pub recent_semantic_keys: &'a RecentKeySet,
    /// Frozen kernel configuration.
    pub config: &'a KernelConfig,
    /// Logical "now" in wall-clock nanoseconds. Caller supplies; kernel
    /// never calls `Timestamp::now` or `SystemTime::now`.
    pub logical_now_ns: u64,
    /// Replay-run identifier. Not used by the v1 decision logic but
    /// carried through the boundary adapter so COID generation can
    /// depend on it.
    pub run_id: [u8; 16],
}
