//! In-memory order book backed by [`BTreeMap`] and a concurrent store
//! backed by [`DashMap`].
//!
//! Prices and sizes are stored as [`u64`] scaled by 1 000 (see
//! [`crate::types::parse_price`]).  A size of `0` signals removal of a
//! price level, matching Polymarket's delta protocol.

use std::collections::BTreeMap;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::types::{parse_price, MarketEvent, OrderSide, PriceLevel};

// ─── OrderBook ───────────────────────────────────────────────────────────────

/// Single-market order book maintaining bid and ask levels as sorted maps.
///
/// Bids are stored in ascending key order; [`best_bid`](Self::best_bid) reads
/// the last (highest) key.  Asks are stored ascending; [`best_ask`](Self::best_ask)
/// reads the first (lowest) key.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    /// bid price (×1 000) → size (×1 000).
    pub bids: BTreeMap<u64, u64>,
    /// ask price (×1 000) → size (×1 000).
    pub asks: BTreeMap<u64, u64>,
}

impl OrderBook {
    /// Creates an empty order book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a bid delta slice.
    ///
    /// Levels with `size == 0` are **removed**; all others are inserted or
    /// updated in-place.
    pub fn apply_bids_delta(&mut self, levels: &[PriceLevel]) {
        for level in levels {
            if level.size == 0 {
                self.bids.remove(&level.price);
            } else {
                self.bids.insert(level.price, level.size);
            }
        }
    }

    /// Applies an ask delta slice.
    ///
    /// Levels with `size == 0` are **removed**; all others are inserted or
    /// updated in-place.
    pub fn apply_asks_delta(&mut self, levels: &[PriceLevel]) {
        for level in levels {
            if level.size == 0 {
                self.asks.remove(&level.price);
            } else {
                self.asks.insert(level.price, level.size);
            }
        }
    }

    /// Returns the highest bid price (×1 000), or `None` if the book is empty.
    #[inline]
    pub fn best_bid(&self) -> Option<u64> {
        self.bids.keys().next_back().copied()
    }

    /// Returns the lowest ask price (×1 000), or `None` if the book is empty.
    #[inline]
    pub fn best_ask(&self) -> Option<u64> {
        self.asks.keys().next().copied()
    }

    /// Returns the bid-ask spread in basis points relative to the mid-price,
    /// or `None` if either side of the book is empty or the mid-price is zero.
    ///
    /// `spread_bps = (ask - bid) × 10 000 / mid`
    pub fn spread_bps(&self) -> Option<u64> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        let mid = (bid + ask) / 2;
        if mid == 0 {
            return None;
        }
        Some(ask.saturating_sub(bid) * 10_000 / mid)
    }

    /// Returns the arithmetic mid-price (×1 000), or `None` if either side
    /// of the book is empty.
    #[allow(dead_code)]
    #[inline]
    pub fn mid_price(&self) -> Option<u64> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        Some((bid + ask) / 2)
    }
}

// ─── OrderBookStore ───────────────────────────────────────────────────────────

/// Thread-safe, multi-market order book store.
///
/// Keyed by Polymarket token ID (`String`).  Concurrent access is handled
/// by [`DashMap`]; no external locking is required.
pub struct OrderBookStore {
    books: DashMap<String, OrderBook>,
}

impl OrderBookStore {
    /// Creates a new, empty store.
    pub fn new() -> Self {
        Self {
            books: DashMap::new(),
        }
    }

    /// Returns the current mid-price (×1 000) for a token, or `None` if the
    /// order book is empty or the token is unknown.
    #[inline]
    pub fn get_mid_price(&self, token_id: &str) -> Option<u64> {
        self.books.get(token_id).and_then(|b| b.mid_price())
    }

    /// Returns a mark price (×1 000) for a token.
    ///
    /// Uses mid-price when both sides are available; otherwise falls back to
    /// the best available side (bid or ask). Returns `None` only when book has
    /// no priced levels at all.
    #[inline]
    pub fn get_mark_price(&self, token_id: &str) -> Option<u64> {
        let book = self.books.get(token_id)?;
        if let Some(mid) = book.mid_price() {
            return Some(mid);
        }
        book.best_bid().or_else(|| book.best_ask())
    }

    /// Returns a mutable reference to the order book for `token_id`,
    /// inserting a fresh empty book if none exists yet.
    pub fn get_or_create(
        &self,
        token_id: &str,
    ) -> dashmap::mapref::one::RefMut<'_, String, OrderBook> {
        self.books
            .entry(token_id.to_string())
            .or_insert_with(OrderBook::new)
    }

    /// Applies a [`MarketEvent`] to the relevant order book.
    ///
    /// `Book` events replace the full book; `PriceChange` events apply
    /// incremental deltas keyed by asset_id and side.
    #[instrument(skip(self, event), fields(event_type))]
    pub fn apply_update(&self, event: &MarketEvent) {
        match event {
            MarketEvent::Book(book_event) => {
                // Key by asset_id (token) when present; fall back to market (condition ID).
                let key = book_event.asset_id.as_deref().unwrap_or(&book_event.market);

                let bid_levels: Vec<PriceLevel> =
                    book_event.bids.iter().map(|r| r.to_price_level()).collect();
                let ask_levels: Vec<PriceLevel> =
                    book_event.asks.iter().map(|r| r.to_price_level()).collect();

                let mut book = self.get_or_create(key);
                book.apply_bids_delta(&bid_levels);
                book.apply_asks_delta(&ask_levels);

                tracing::debug!(
                    key,
                    best_bid = ?book.best_bid(),
                    best_ask = ?book.best_ask(),
                    spread_bps = ?book.spread_bps(),
                    "book snapshot applied"
                );
            }
            MarketEvent::PriceChange(pc_event) => {
                for change in &pc_event.price_changes {
                    let level = PriceLevel {
                        price: parse_price(&change.price),
                        size: parse_price(&change.size),
                    };
                    let mut book = self.get_or_create(&change.asset_id);
                    match change.side {
                        OrderSide::Buy => book.apply_bids_delta(&[level]),
                        OrderSide::Sell => book.apply_asks_delta(&[level]),
                    }
                    tracing::debug!(
                        asset_id = %change.asset_id,
                        side = %change.side,
                        price = %change.price,
                        size = %change.size,
                        "price_change applied"
                    );
                }
            }
            // Trades, orders, and unknown events handled elsewhere.
            _ => {}
        }
    }

    /// Returns a cloned snapshot of the order book for a given token ID,
    /// or `None` if the token is not tracked.
    pub fn get_book_snapshot(&self, token_id: &str) -> Option<OrderBook> {
        self.books.get(token_id).map(|b| b.clone())
    }

    /// Returns top-of-book level `(price, size)` for the requested side.
    pub fn top_of_book(&self, token_id: &str, side: OrderSide) -> Option<(u64, u64)> {
        let book = self.books.get(token_id)?;
        match side {
            OrderSide::Buy => book.asks.iter().next().map(|(price, size)| (*price, *size)),
            OrderSide::Sell => book
                .bids
                .iter()
                .next_back()
                .map(|(price, size)| (*price, *size)),
        }
    }

    /// Returns full snapshots of all tracked books.
    pub fn all_snapshots(&self) -> Vec<(String, OrderBook)> {
        self.books
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Restores books from snapshots.
    pub fn restore_snapshots(&self, snapshots: &[(String, OrderBook)]) {
        self.books.clear();
        for (token, book) in snapshots {
            self.books.insert(token.clone(), book.clone());
        }
    }

    /// Returns a list of all currently tracked token IDs.
    pub fn token_ids(&self) -> Vec<String> {
        self.books.iter().map(|e| e.key().clone()).collect()
    }
}

impl Default for OrderBookStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn level(price: u64, size: u64) -> PriceLevel {
        PriceLevel { price, size }
    }

    #[test]
    fn best_bid_ask() {
        let mut book = OrderBook::new();
        book.apply_bids_delta(&[level(640, 1_000), level(650, 500), level(630, 200)]);
        book.apply_asks_delta(&[level(660, 800), level(670, 300)]);

        assert_eq!(book.best_bid(), Some(650));
        assert_eq!(book.best_ask(), Some(660));
    }

    #[test]
    fn remove_level_on_zero_size() {
        let mut book = OrderBook::new();
        book.apply_bids_delta(&[level(650, 1_000)]);
        book.apply_bids_delta(&[level(650, 0)]); // removal
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn mid_price_and_spread() {
        let mut book = OrderBook::new();
        book.apply_bids_delta(&[level(650, 100)]);
        book.apply_asks_delta(&[level(660, 100)]);
        // mid = 655, spread = 10, bps = 10*10_000/655 ≈ 152
        assert_eq!(book.mid_price(), Some(655));
        let spread = book.spread_bps().unwrap();
        assert!(spread > 140 && spread < 165, "spread_bps={spread}");
    }
}
