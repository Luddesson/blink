//! Builders for tests and examples. Gated behind `feature = "test-support"`.
//!
//! The functions here return owned items so a test can construct all
//! four borrowed fields of a `DecisionSnapshot` from a single `let`
//! binding:
//!
//! ```ignore
//! let fx = blink_kernel::test_support::fixture();
//! let snap = fx.snapshot();
//! let verdict = V1Kernel::new().decide(&snap, &mut stats);
//! ```

use crate::{config::KernelConfig, dedup::RecentKeySet, snapshot::DecisionSnapshot};
use blink_book::{BookSnapshot, LadderSide, Level, PositionSnapshot, Timestamp};
use blink_types::{EventId, OnChainAnchor, PriceTicks, RawEvent, Side, SizeU, SourceKind};

/// Bundle of owned items; `DecisionSnapshot` borrows from this.
pub struct Fixture {
    /// Event used as the decision trigger.
    pub event: RawEvent,
    /// Book snapshot the kernel reads.
    pub book: BookSnapshot,
    /// Position for the target token.
    pub position: PositionSnapshot,
    /// Dedup ring.
    pub recent: RecentKeySet,
    /// Kernel configuration.
    pub config: KernelConfig,
    /// Logical now, wall-clock ns.
    pub logical_now_ns: u64,
    /// Run id passed through to the boundary adapter.
    pub run_id: [u8; 16],
}

impl Fixture {
    /// Borrow a `DecisionSnapshot` from this fixture.
    pub fn snapshot(&self) -> DecisionSnapshot<'_> {
        DecisionSnapshot {
            event: &self.event,
            book: &self.book,
            position: &self.position,
            recent_semantic_keys: &self.recent,
            config: &self.config,
            logical_now_ns: self.logical_now_ns,
            run_id: self.run_id,
        }
    }
}

/// Default fixture: fresh book, neutral position, submit-ready event.
pub fn fixture() -> Fixture {
    let token_id = "0xtoken".to_string();
    let market_id = "0xmarket".to_string();
    let bid = LadderSide::from_slice(&[Level::new(500, 10_000)]);
    let ask = LadderSide::from_slice(&[Level::new(520, 10_000)]);
    let book = BookSnapshot {
        token_id: token_id.clone(),
        market_id: market_id.clone(),
        seq: 1,
        source_wall_ns: 1_000_000_000,
        tsc_received: Timestamp::UNSET,
        bid,
        ask,
    };
    let position = PositionSnapshot::zero(0xAABBCCDDu64);
    let event = raw_event(&token_id, &market_id, Side::Buy, 510, 1_000);
    Fixture {
        event,
        book,
        position,
        recent: RecentKeySet::new(),
        config: KernelConfig::conservative(),
        logical_now_ns: 1_000_500_000, // 500ms after book
        run_id: [0u8; 16],
    }
}

/// Build a minimal `RawEvent` carrying direction, price, and size.
pub fn raw_event(token_id: &str, market_id: &str, side: Side, price: u64, size: u64) -> RawEvent {
    RawEvent {
        event_id: EventId(1),
        source: SourceKind::Manual,
        source_seq: 0,
        anchor: Some(OnChainAnchor { tx_hash: [0u8; 32], log_index: 0 }),
        token_id: token_id.to_string(),
        market_id: Some(market_id.to_string()),
        side: Some(side),
        price: Some(PriceTicks(price)),
        size: Some(SizeU(size)),
        tsc_in: Timestamp::UNSET,
        wall_ns: 0,
        extra: None,
        observe_only: false,
        maker_wallet: None,
    }
}

/// Build a book with only the two given top-of-book prices (in ticks).
pub fn book_with_top(
    token_id: &str,
    market_id: &str,
    best_bid: u32,
    best_ask: u32,
    source_wall_ns: u64,
) -> BookSnapshot {
    BookSnapshot {
        token_id: token_id.to_string(),
        market_id: market_id.to_string(),
        seq: 1,
        source_wall_ns,
        tsc_received: Timestamp::UNSET,
        bid: LadderSide::from_slice(&[Level::new(best_bid, 10_000)]),
        ask: LadderSide::from_slice(&[Level::new(best_ask, 10_000)]),
    }
}
