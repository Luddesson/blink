//! Deterministic replay, feature extraction, and fill simulation for quant research.
//!
//! This module is intentionally offline-only. It owns no sockets, no keys, and no
//! order submission path. The goal is to turn captured market events into
//! reproducible feature frames and execution assumptions that can be audited
//! before any strategy is allowed near live capital.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::types::OrderSide;

pub type TimestampMs = i64;

/// Price/size level using Blink's scaled integer convention:
/// price × 1_000, size × 1_000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayLevel {
    pub price: u64,
    pub size: u64,
}

/// A single market event in deterministic replay order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayEvent {
    pub timestamp_ms: TimestampMs,
    /// Stable source-order tie breaker. For DB rows this should be the
    /// insertion sequence; for files it should be the line number.
    pub seq: u64,
    pub kind: ReplayEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplayEventKind {
    BookSnapshot {
        token_id: String,
        bids: Vec<ReplayLevel>,
        asks: Vec<ReplayLevel>,
    },
    BookDelta {
        token_id: String,
        side: OrderSide,
        price: u64,
        size: u64,
    },
    Trade {
        token_id: String,
        side: OrderSide,
        price: u64,
        size: u64,
    },
    Signal {
        token_id: String,
        side: OrderSide,
        price: u64,
        size: u64,
        wallet: String,
    },
}

impl ReplayEvent {
    pub fn token_id(&self) -> &str {
        match &self.kind {
            ReplayEventKind::BookSnapshot { token_id, .. }
            | ReplayEventKind::BookDelta { token_id, .. }
            | ReplayEventKind::Trade { token_id, .. }
            | ReplayEventKind::Signal { token_id, .. } => token_id,
        }
    }
}

/// Sorts replay events by timestamp and then stable source sequence.
pub fn sort_replay_events(events: &mut [ReplayEvent]) {
    events.sort_by_key(|event| (event.timestamp_ms, event.seq));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TradeObservation {
    timestamp_ms: TimestampMs,
    side: OrderSide,
    size: u64,
}

#[derive(Debug, Clone, Default)]
struct MarketState {
    bids: BTreeMap<u64, u64>,
    asks: BTreeMap<u64, u64>,
    recent_trades: VecDeque<TradeObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BookFill {
    filled_size: u64,
    avg_price: u64,
    fully_filled: bool,
}

impl MarketState {
    fn apply_book_snapshot(&mut self, bids: &[ReplayLevel], asks: &[ReplayLevel]) {
        self.bids = levels_to_book(bids);
        self.asks = levels_to_book(asks);
    }

    fn apply_book_delta(&mut self, side: OrderSide, price: u64, size: u64) {
        let book = match side {
            OrderSide::Buy => &mut self.bids,
            OrderSide::Sell => &mut self.asks,
        };
        if size == 0 {
            book.remove(&price);
        } else {
            book.insert(price, size);
        }
    }

    fn apply_trade(&mut self, timestamp_ms: TimestampMs, side: OrderSide, size: u64) {
        self.recent_trades.push_back(TradeObservation {
            timestamp_ms,
            side,
            size,
        });
    }

    fn prune_trades(&mut self, min_timestamp_ms: TimestampMs) {
        while self
            .recent_trades
            .front()
            .is_some_and(|trade| trade.timestamp_ms < min_timestamp_ms)
        {
            self.recent_trades.pop_front();
        }
    }

    fn best_bid(&self) -> Option<(u64, u64)> {
        self.bids.iter().next_back().map(|(p, s)| (*p, *s))
    }

    fn best_ask(&self) -> Option<(u64, u64)> {
        self.asks.iter().next().map(|(p, s)| (*p, *s))
    }

    fn visible_depth(&self) -> (u64, u64) {
        (
            self.bids.values().copied().sum(),
            self.asks.values().copied().sum(),
        )
    }

    fn trade_flow(&self) -> (u64, u64, usize) {
        let mut buy_volume = 0u64;
        let mut sell_volume = 0u64;
        for trade in &self.recent_trades {
            match trade.side {
                OrderSide::Buy => buy_volume = buy_volume.saturating_add(trade.size),
                OrderSide::Sell => sell_volume = sell_volume.saturating_add(trade.size),
            }
        }
        (buy_volume, sell_volume, self.recent_trades.len())
    }

    fn fill_taker(
        &self,
        side: OrderSide,
        limit_price: u64,
        requested_size: u64,
        slippage_bps: u64,
    ) -> Option<BookFill> {
        let mut remaining = requested_size;
        let mut filled = 0u64;
        let mut notional = 0u128;

        match side {
            OrderSide::Buy => {
                for (&price, &available_size) in &self.asks {
                    if price > limit_price {
                        break;
                    }
                    let Some(exec_price) =
                        apply_slippage_with_limit(price, side, slippage_bps, limit_price)
                    else {
                        break;
                    };
                    let fill_size = remaining.min(available_size);
                    if fill_size == 0 {
                        continue;
                    }
                    filled = filled.saturating_add(fill_size);
                    remaining -= fill_size;
                    notional += fill_size as u128 * exec_price as u128;
                    if remaining == 0 {
                        break;
                    }
                }
            }
            OrderSide::Sell => {
                for (&price, &available_size) in self.bids.iter().rev() {
                    if price < limit_price {
                        break;
                    }
                    let Some(exec_price) =
                        apply_slippage_with_limit(price, side, slippage_bps, limit_price)
                    else {
                        break;
                    };
                    let fill_size = remaining.min(available_size);
                    if fill_size == 0 {
                        continue;
                    }
                    filled = filled.saturating_add(fill_size);
                    remaining -= fill_size;
                    notional += fill_size as u128 * exec_price as u128;
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }

        (filled > 0).then(|| BookFill {
            filled_size: filled,
            avg_price: (notional / filled as u128) as u64,
            fully_filled: remaining == 0,
        })
    }
}

fn levels_to_book(levels: &[ReplayLevel]) -> BTreeMap<u64, u64> {
    levels
        .iter()
        .filter(|level| level.price > 0 && level.size > 0)
        .map(|level| (level.price, level.size))
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub struct FeatureConfig {
    /// Rolling trade-flow horizon.
    pub flow_window_ms: i64,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            flow_window_ms: 5_000,
        }
    }
}

/// One timestamped feature vector for a token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureFrame {
    pub timestamp_ms: TimestampMs,
    pub token_id: String,
    pub best_bid: Option<u64>,
    pub best_ask: Option<u64>,
    pub mid_price: Option<u64>,
    pub micro_price: Option<u64>,
    pub spread_bps: Option<u64>,
    pub bid_depth: u64,
    pub ask_depth: u64,
    pub book_imbalance_bps: i64,
    pub trade_count_window: usize,
    pub buy_volume_window: u64,
    pub sell_volume_window: u64,
    pub order_flow_imbalance_bps: i64,
}

impl FeatureFrame {
    /// Magnitude-only adverse-selection proxy. Higher means wider spread,
    /// stronger one-sided book, or stronger one-sided recent flow.
    pub fn toxicity_score_bps(&self) -> i64 {
        let flow = self.order_flow_imbalance_bps.abs();
        let book = self.book_imbalance_bps.abs();
        let spread = self.spread_bps.unwrap_or(0) as i64;
        flow.saturating_add(book / 2).saturating_add(spread / 4)
    }
}

/// Pre/post feature frames for one applied event.
///
/// Strategy decisions should use `pre` for the signal event being evaluated.
/// `post` includes the current event and is intended for state advancement,
/// monitoring, and offline diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureStep {
    pub timestamp_ms: TimestampMs,
    pub seq: u64,
    pub token_id: String,
    pub pre: Option<FeatureFrame>,
    pub post: FeatureFrame,
}

/// Stateful feature extractor. Feed events in replay order.
#[derive(Debug, Clone)]
pub struct FeatureStore {
    config: FeatureConfig,
    states: HashMap<String, MarketState>,
}

impl FeatureStore {
    pub fn new(config: FeatureConfig) -> Self {
        Self {
            config,
            states: HashMap::new(),
        }
    }

    /// Applies an event and returns the post-event frame.
    ///
    /// Use [`FeatureStore::apply_with_frames`] when evaluating a strategy at
    /// event time so the decision can consume the pre-event frame.
    pub fn apply(&mut self, event: &ReplayEvent) -> FeatureFrame {
        self.apply_post_event(event)
    }

    pub fn apply_with_frames(&mut self, event: &ReplayEvent) -> FeatureStep {
        let token_id = event.token_id().to_string();
        let pre = self.states.get_mut(&token_id).map(|state| {
            state.prune_trades(event.timestamp_ms - self.config.flow_window_ms);
            build_feature_frame(event.timestamp_ms, token_id.clone(), state)
        });
        let post = self.apply_post_event(event);

        FeatureStep {
            timestamp_ms: event.timestamp_ms,
            seq: event.seq,
            token_id,
            pre,
            post,
        }
    }

    pub fn apply_post_event(&mut self, event: &ReplayEvent) -> FeatureFrame {
        let token_id = event.token_id().to_string();
        let state = self.states.entry(token_id.clone()).or_default();

        match &event.kind {
            ReplayEventKind::BookSnapshot { bids, asks, .. } => {
                state.apply_book_snapshot(bids, asks);
            }
            ReplayEventKind::BookDelta {
                side, price, size, ..
            } => {
                state.apply_book_delta(*side, *price, *size);
            }
            ReplayEventKind::Trade { side, size, .. } => {
                state.apply_trade(event.timestamp_ms, *side, *size);
            }
            ReplayEventKind::Signal { .. } => {}
        }

        state.prune_trades(event.timestamp_ms - self.config.flow_window_ms);
        build_feature_frame(event.timestamp_ms, token_id, state)
    }

    fn state_for(&self, token_id: &str) -> Option<&MarketState> {
        self.states.get(token_id)
    }
}

pub fn replay_features(mut events: Vec<ReplayEvent>, config: FeatureConfig) -> Vec<FeatureFrame> {
    sort_replay_events(&mut events);
    let mut store = FeatureStore::new(config);
    events.iter().map(|event| store.apply(event)).collect()
}

pub fn replay_feature_steps(
    mut events: Vec<ReplayEvent>,
    config: FeatureConfig,
) -> Vec<FeatureStep> {
    sort_replay_events(&mut events);
    let mut store = FeatureStore::new(config);
    events
        .iter()
        .map(|event| store.apply_with_frames(event))
        .collect()
}

fn build_feature_frame(
    timestamp_ms: TimestampMs,
    token_id: String,
    state: &MarketState,
) -> FeatureFrame {
    let best_bid = state.best_bid();
    let best_ask = state.best_ask();
    let (bid_depth, ask_depth) = state.visible_depth();
    let (buy_volume, sell_volume, trade_count) = state.trade_flow();

    let mid_price = match (best_bid, best_ask) {
        (Some((bid, _)), Some((ask, _))) => Some((bid + ask) / 2),
        _ => None,
    };

    let micro_price = match (best_bid, best_ask) {
        (Some((bid, bid_size)), Some((ask, ask_size))) if bid_size + ask_size > 0 => {
            // Queue-aware microprice: bid-heavy books pull fair value toward ask.
            let numerator = (ask as u128 * bid_size as u128) + (bid as u128 * ask_size as u128);
            Some((numerator / (bid_size as u128 + ask_size as u128)) as u64)
        }
        _ => None,
    };

    let spread_bps = match (best_bid, best_ask, mid_price) {
        (Some((bid, _)), Some((ask, _)), Some(mid)) if ask >= bid && mid > 0 => {
            Some(((ask - bid) * 10_000) / mid)
        }
        _ => None,
    };

    FeatureFrame {
        timestamp_ms,
        token_id,
        best_bid: best_bid.map(|(price, _)| price),
        best_ask: best_ask.map(|(price, _)| price),
        mid_price,
        micro_price,
        spread_bps,
        bid_depth,
        ask_depth,
        book_imbalance_bps: imbalance_bps(bid_depth, ask_depth),
        trade_count_window: trade_count,
        buy_volume_window: buy_volume,
        sell_volume_window: sell_volume,
        order_flow_imbalance_bps: imbalance_bps(buy_volume, sell_volume),
    }
}

fn imbalance_bps(left: u64, right: u64) -> i64 {
    let total = left.saturating_add(right);
    if total == 0 {
        return 0;
    }
    let diff = left as i128 - right as i128;
    ((diff * 10_000) / total as i128) as i64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimulatedOrderKind {
    Taker,
    Maker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulatedOrder {
    pub submitted_at_ms: TimestampMs,
    pub token_id: String,
    pub side: OrderSide,
    pub limit_price: u64,
    pub size: u64,
    pub kind: SimulatedOrderKind,
}

#[derive(Debug, Clone, Copy)]
pub struct FillModelConfig {
    pub taker_latency_ms: i64,
    pub maker_latency_ms: i64,
    pub taker_slippage_bps: u64,
    /// Volume already ahead of our maker order at the chosen price.
    pub maker_queue_ahead: u64,
}

impl Default for FillModelConfig {
    fn default() -> Self {
        Self {
            taker_latency_ms: 150,
            maker_latency_ms: 80,
            taker_slippage_bps: 0,
            maker_queue_ahead: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillStatus {
    Filled,
    Partial,
    NoFill,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FillOutcome {
    pub status: FillStatus,
    pub first_fill_ms: Option<TimestampMs>,
    pub filled_size: u64,
    pub avg_price: Option<u64>,
    pub reason: Option<String>,
}

impl FillOutcome {
    fn no_fill(reason: impl Into<String>) -> Self {
        Self {
            status: FillStatus::NoFill,
            first_fill_ms: None,
            filled_size: 0,
            avg_price: None,
            reason: Some(reason.into()),
        }
    }

    fn rejected(reason: impl Into<String>) -> Self {
        Self {
            status: FillStatus::Rejected,
            first_fill_ms: None,
            filled_size: 0,
            avg_price: None,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FillSimulator {
    config: FillModelConfig,
}

impl FillSimulator {
    pub fn new(config: FillModelConfig) -> Self {
        Self { config }
    }

    pub fn simulate(&self, order: &SimulatedOrder, events: &[ReplayEvent]) -> FillOutcome {
        if order.limit_price == 0 || order.limit_price >= 1_000 {
            return FillOutcome::rejected("limit_price_outside_clob_range");
        }
        if order.size == 0 {
            return FillOutcome::rejected("zero_size_order");
        }

        match order.kind {
            SimulatedOrderKind::Taker => self.simulate_taker(order, events),
            SimulatedOrderKind::Maker => self.simulate_maker(order, events),
        }
    }

    fn simulate_taker(&self, order: &SimulatedOrder, events: &[ReplayEvent]) -> FillOutcome {
        let effective_at = order.submitted_at_ms + self.config.taker_latency_ms;
        let mut sorted = events.to_vec();
        sort_replay_events(&mut sorted);

        let mut store = FeatureStore::new(FeatureConfig::default());
        for event in &sorted {
            if event.timestamp_ms > effective_at {
                return self
                    .try_fill_taker_from_state(order, &store, effective_at)
                    .unwrap_or_else(|| FillOutcome::no_fill("no_marketable_book_after_latency"));
            }

            store.apply_post_event(event);
        }

        self.try_fill_taker_from_state(order, &store, effective_at)
            .unwrap_or_else(|| FillOutcome::no_fill("no_marketable_book_after_latency"))
    }

    fn try_fill_taker_from_state(
        &self,
        order: &SimulatedOrder,
        store: &FeatureStore,
        fill_timestamp_ms: TimestampMs,
    ) -> Option<FillOutcome> {
        let state = store.state_for(&order.token_id)?;
        let fill = state.fill_taker(
            order.side,
            order.limit_price,
            order.size,
            self.config.taker_slippage_bps,
        )?;
        Some(FillOutcome {
            status: if fill.fully_filled {
                FillStatus::Filled
            } else {
                FillStatus::Partial
            },
            first_fill_ms: Some(fill_timestamp_ms),
            filled_size: fill.filled_size,
            avg_price: Some(fill.avg_price),
            reason: (!fill.fully_filled).then(|| "visible_book_liquidity_exhausted".to_string()),
        })
    }

    fn simulate_maker(&self, order: &SimulatedOrder, events: &[ReplayEvent]) -> FillOutcome {
        let active_at = order.submitted_at_ms + self.config.maker_latency_ms;
        let mut sorted = events.to_vec();
        sort_replay_events(&mut sorted);

        let mut queue_ahead = self.config.maker_queue_ahead;
        let mut remaining = order.size;
        let mut filled = 0u64;
        let mut notional = 0u128;
        let mut first_fill_ms = None;

        for event in &sorted {
            if event.timestamp_ms < active_at || event.token_id() != order.token_id {
                continue;
            }

            let ReplayEventKind::Trade {
                side, price, size, ..
            } = &event.kind
            else {
                continue;
            };

            let crosses_our_quote = match order.side {
                // Our maker BUY rests on bid and fills against aggressive sells.
                OrderSide::Buy => *side == OrderSide::Sell && *price <= order.limit_price,
                // Our maker SELL rests on ask and fills against aggressive buys.
                OrderSide::Sell => *side == OrderSide::Buy && *price >= order.limit_price,
            };
            if !crosses_our_quote {
                continue;
            }

            let mut trade_size = *size;
            if queue_ahead > 0 {
                let consumed = queue_ahead.min(trade_size);
                queue_ahead -= consumed;
                trade_size -= consumed;
            }
            if trade_size == 0 {
                continue;
            }

            let fill_size = remaining.min(trade_size);
            if fill_size == 0 {
                break;
            }
            if first_fill_ms.is_none() {
                first_fill_ms = Some(event.timestamp_ms);
            }
            filled = filled.saturating_add(fill_size);
            remaining -= fill_size;
            notional += fill_size as u128 * order.limit_price as u128;

            if remaining == 0 {
                break;
            }
        }

        if filled == 0 {
            return FillOutcome::no_fill("queue_not_reached");
        }

        FillOutcome {
            status: if remaining == 0 {
                FillStatus::Filled
            } else {
                FillStatus::Partial
            },
            first_fill_ms,
            filled_size: filled,
            avg_price: Some((notional / filled as u128) as u64),
            reason: None,
        }
    }
}

fn apply_slippage_with_limit(
    price: u64,
    side: OrderSide,
    slippage_bps: u64,
    limit_price: u64,
) -> Option<u64> {
    let adjustment = (price as u128 * slippage_bps as u128) / 10_000;
    let adjusted = match side {
        OrderSide::Buy => (price as u128 + adjustment).min(999) as u64,
        OrderSide::Sell => (price as u128).saturating_sub(adjustment).max(1) as u64,
    };

    match side {
        OrderSide::Buy if adjusted <= limit_price => Some(adjusted),
        OrderSide::Sell if adjusted >= limit_price => Some(adjusted),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(price: u64, size: u64) -> ReplayLevel {
        ReplayLevel { price, size }
    }

    fn book(ts: TimestampMs, seq: u64, token_id: &str) -> ReplayEvent {
        ReplayEvent {
            timestamp_ms: ts,
            seq,
            kind: ReplayEventKind::BookSnapshot {
                token_id: token_id.to_string(),
                bids: vec![level(490, 10_000), level(480, 5_000)],
                asks: vec![level(510, 3_000), level(520, 5_000)],
            },
        }
    }

    fn trade(
        ts: TimestampMs,
        seq: u64,
        token_id: &str,
        side: OrderSide,
        price: u64,
        size: u64,
    ) -> ReplayEvent {
        ReplayEvent {
            timestamp_ms: ts,
            seq,
            kind: ReplayEventKind::Trade {
                token_id: token_id.to_string(),
                side,
                price,
                size,
            },
        }
    }

    fn signal(ts: TimestampMs, seq: u64, token_id: &str) -> ReplayEvent {
        ReplayEvent {
            timestamp_ms: ts,
            seq,
            kind: ReplayEventKind::Signal {
                token_id: token_id.to_string(),
                side: OrderSide::Buy,
                price: 500,
                size: 1_000,
                wallet: "0xwallet".to_string(),
            },
        }
    }

    #[test]
    fn replay_order_is_deterministic_for_same_timestamp() {
        let mut events = vec![
            trade(1_000, 2, "tok", OrderSide::Buy, 501, 1_000),
            book(1_000, 1, "tok"),
            trade(999, 9, "tok", OrderSide::Sell, 500, 1_000),
        ];

        sort_replay_events(&mut events);

        assert_eq!(events[0].timestamp_ms, 999);
        assert_eq!(events[1].seq, 1);
        assert_eq!(events[2].seq, 2);
    }

    #[test]
    fn feature_frame_computes_spread_microprice_and_imbalance() {
        let mut store = FeatureStore::new(FeatureConfig::default());
        let frame = store.apply(&book(1_000, 1, "tok"));

        assert_eq!(frame.best_bid, Some(490));
        assert_eq!(frame.best_ask, Some(510));
        assert_eq!(frame.mid_price, Some(500));
        assert_eq!(frame.spread_bps, Some(400));
        assert_eq!(frame.bid_depth, 15_000);
        assert_eq!(frame.ask_depth, 8_000);
        assert!(frame.book_imbalance_bps > 0);
        assert!(
            frame.micro_price.unwrap() > frame.mid_price.unwrap(),
            "bid-heavy book should pull microprice toward ask"
        );
    }

    #[test]
    fn rolling_flow_window_prunes_old_trades() {
        let mut store = FeatureStore::new(FeatureConfig {
            flow_window_ms: 1_000,
        });
        store.apply(&book(1_000, 1, "tok"));
        store.apply(&trade(1_100, 2, "tok", OrderSide::Buy, 500, 10_000));
        let frame = store.apply(&trade(2_200, 3, "tok", OrderSide::Sell, 499, 2_000));

        assert_eq!(frame.buy_volume_window, 0);
        assert_eq!(frame.sell_volume_window, 2_000);
        assert_eq!(frame.trade_count_window, 1);
    }

    #[test]
    fn pre_event_frame_excludes_current_trade() {
        let mut store = FeatureStore::new(FeatureConfig::default());
        store.apply(&book(1_000, 1, "tok"));

        let step = store.apply_with_frames(&trade(1_100, 2, "tok", OrderSide::Buy, 500, 10_000));

        let pre = step.pre.expect("book snapshot should create pre-frame");
        assert_eq!(pre.trade_count_window, 0);
        assert_eq!(pre.buy_volume_window, 0);
        assert_eq!(step.post.trade_count_window, 1);
        assert_eq!(step.post.buy_volume_window, 10_000);
    }

    #[test]
    fn pre_event_frame_prunes_stale_flow_after_gap() {
        let mut store = FeatureStore::new(FeatureConfig {
            flow_window_ms: 1_000,
        });
        store.apply(&book(1_000, 1, "tok"));
        store.apply(&trade(1_100, 2, "tok", OrderSide::Buy, 500, 10_000));

        let step = store.apply_with_frames(&signal(2_500, 3, "tok"));

        let pre = step.pre.expect("book snapshot should create pre-frame");
        assert_eq!(pre.trade_count_window, 0);
        assert_eq!(pre.buy_volume_window, 0);
        assert_eq!(step.post.trade_count_window, 0);
    }

    #[test]
    fn taker_fill_waits_for_latency_and_does_not_use_prior_book() {
        let events = vec![
            ReplayEvent {
                timestamp_ms: 1_000,
                seq: 1,
                kind: ReplayEventKind::BookSnapshot {
                    token_id: "tok".to_string(),
                    bids: vec![level(490, 10_000)],
                    asks: vec![level(500, 10_000)],
                },
            },
            ReplayEvent {
                timestamp_ms: 1_100,
                seq: 2,
                kind: ReplayEventKind::BookSnapshot {
                    token_id: "tok".to_string(),
                    bids: vec![level(490, 10_000)],
                    asks: vec![level(525, 10_000)],
                },
            },
        ];
        let sim = FillSimulator::new(FillModelConfig {
            taker_latency_ms: 100,
            ..FillModelConfig::default()
        });
        let order = SimulatedOrder {
            submitted_at_ms: 1_000,
            token_id: "tok".to_string(),
            side: OrderSide::Buy,
            limit_price: 505,
            size: 1_000,
            kind: SimulatedOrderKind::Taker,
        };

        let outcome = sim.simulate(&order, &events);

        assert_eq!(outcome.status, FillStatus::NoFill);
        assert_eq!(
            outcome.reason.as_deref(),
            Some("no_marketable_book_after_latency")
        );
    }

    #[test]
    fn taker_fill_uses_resting_book_when_no_new_event_arrives_at_latency() {
        let events = vec![ReplayEvent {
            timestamp_ms: 1_000,
            seq: 1,
            kind: ReplayEventKind::BookSnapshot {
                token_id: "tok".to_string(),
                bids: vec![level(490, 10_000)],
                asks: vec![level(500, 10_000)],
            },
        }];
        let sim = FillSimulator::new(FillModelConfig {
            taker_latency_ms: 100,
            ..FillModelConfig::default()
        });
        let order = SimulatedOrder {
            submitted_at_ms: 1_000,
            token_id: "tok".to_string(),
            side: OrderSide::Buy,
            limit_price: 505,
            size: 1_000,
            kind: SimulatedOrderKind::Taker,
        };

        let outcome = sim.simulate(&order, &events);

        assert_eq!(outcome.status, FillStatus::Filled);
        assert_eq!(outcome.first_fill_ms, Some(1_100));
        assert_eq!(outcome.avg_price, Some(500));
    }

    #[test]
    fn taker_fill_walks_visible_depth_and_can_partial() {
        let events = vec![ReplayEvent {
            timestamp_ms: 1_000,
            seq: 1,
            kind: ReplayEventKind::BookSnapshot {
                token_id: "tok".to_string(),
                bids: vec![level(490, 10_000)],
                asks: vec![level(500, 1_000), level(505, 2_000), level(510, 10_000)],
            },
        }];
        let sim = FillSimulator::new(FillModelConfig {
            taker_latency_ms: 100,
            ..FillModelConfig::default()
        });
        let order = SimulatedOrder {
            submitted_at_ms: 1_000,
            token_id: "tok".to_string(),
            side: OrderSide::Buy,
            limit_price: 505,
            size: 5_000,
            kind: SimulatedOrderKind::Taker,
        };

        let outcome = sim.simulate(&order, &events);

        assert_eq!(outcome.status, FillStatus::Partial);
        assert_eq!(outcome.first_fill_ms, Some(1_100));
        assert_eq!(outcome.filled_size, 3_000);
        assert_eq!(outcome.avg_price, Some(503));
        assert_eq!(
            outcome.reason.as_deref(),
            Some("visible_book_liquidity_exhausted")
        );
    }

    #[test]
    fn taker_slippage_never_violates_limit_price() {
        let events = vec![ReplayEvent {
            timestamp_ms: 1_000,
            seq: 1,
            kind: ReplayEventKind::BookSnapshot {
                token_id: "tok".to_string(),
                bids: vec![level(490, 10_000)],
                asks: vec![level(500, 10_000)],
            },
        }];
        let sim = FillSimulator::new(FillModelConfig {
            taker_latency_ms: 100,
            taker_slippage_bps: 200,
            ..FillModelConfig::default()
        });
        let order = SimulatedOrder {
            submitted_at_ms: 1_000,
            token_id: "tok".to_string(),
            side: OrderSide::Buy,
            limit_price: 505,
            size: 1_000,
            kind: SimulatedOrderKind::Taker,
        };

        let outcome = sim.simulate(&order, &events);

        assert_eq!(outcome.status, FillStatus::NoFill);
        assert_eq!(outcome.avg_price, None);
    }

    #[test]
    fn maker_fill_respects_queue_ahead_before_filling() {
        let events = vec![
            book(1_000, 1, "tok"),
            trade(1_100, 2, "tok", OrderSide::Sell, 490, 4_000),
            trade(1_200, 3, "tok", OrderSide::Sell, 490, 8_000),
        ];
        let sim = FillSimulator::new(FillModelConfig {
            maker_latency_ms: 50,
            maker_queue_ahead: 5_000,
            ..FillModelConfig::default()
        });
        let order = SimulatedOrder {
            submitted_at_ms: 1_000,
            token_id: "tok".to_string(),
            side: OrderSide::Buy,
            limit_price: 490,
            size: 3_000,
            kind: SimulatedOrderKind::Maker,
        };

        let outcome = sim.simulate(&order, &events);

        assert_eq!(outcome.status, FillStatus::Filled);
        assert_eq!(outcome.first_fill_ms, Some(1_200));
        assert_eq!(outcome.filled_size, 3_000);
        assert_eq!(outcome.avg_price, Some(490));
    }

    #[test]
    fn invalid_clob_price_is_rejected_before_simulation() {
        let sim = FillSimulator::new(FillModelConfig::default());
        let order = SimulatedOrder {
            submitted_at_ms: 1_000,
            token_id: "tok".to_string(),
            side: OrderSide::Buy,
            limit_price: 1_000,
            size: 1_000,
            kind: SimulatedOrderKind::Taker,
        };

        let outcome = sim.simulate(&order, &[]);

        assert_eq!(outcome.status, FillStatus::Rejected);
        assert_eq!(
            outcome.reason.as_deref(),
            Some("limit_price_outside_clob_range")
        );
    }
}
