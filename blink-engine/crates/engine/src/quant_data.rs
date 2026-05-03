//! Data adapters for offline quant replay.
//!
//! The functions here are read-only. They load existing Blink warehouse rows
//! into [`crate::quant_replay::ReplayEvent`] without changing database schema
//! or touching live order flow.

use anyhow::Result;
use tokio_postgres::Client;

use crate::quant_replay::{ReplayEvent, ReplayEventKind, ReplayLevel, TimestampMs};
use crate::types::OrderSide;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickEventMode {
    /// Do not import `blink.ticks`. This is the safest default because that
    /// table can contain CLOB order events rather than confirmed trades.
    Ignore,
    /// Treat ticks as order-book deltas. Useful for rough replay when only
    /// price/size updates were recorded.
    AsBookDelta,
    /// Treat ticks as trade prints. Use only for datasets known to contain
    /// executed trades.
    AsTradeProxy,
}

#[derive(Debug, Clone)]
pub struct PostgresReplayLoadConfig {
    pub start_ms: TimestampMs,
    pub end_ms: TimestampMs,
    /// Optional wallet filter for `blink.rn1_signals`.
    pub rn1_wallet: Option<String>,
    pub tick_mode: TickEventMode,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReplayWarehouseStats {
    pub order_book_snapshots_count: i64,
    pub rn1_signals_count: i64,
    pub ticks_count: i64,
    pub order_book_snapshots_max_timestamp_ms: Option<TimestampMs>,
    pub rn1_signals_max_timestamp_ms: Option<TimestampMs>,
    pub ticks_max_timestamp_ms: Option<TimestampMs>,
}

impl PostgresReplayLoadConfig {
    pub fn last_24h_ending_at(end_ms: TimestampMs, rn1_wallet: Option<String>) -> Self {
        Self {
            start_ms: end_ms - 86_400_000,
            end_ms,
            rn1_wallet,
            tick_mode: TickEventMode::Ignore,
        }
    }
}

pub async fn latest_replay_timestamp_ms(client: &Client) -> Result<Option<TimestampMs>> {
    let row = client
        .query_one(
            "SELECT GREATEST(
                COALESCE((SELECT MAX(timestamp_ms) FROM blink.order_book_snapshots), 0),
                COALESCE((SELECT MAX(timestamp_ms) FROM blink.rn1_signals), 0)
            ) AS max_ts",
            &[],
        )
        .await?;
    let max_ts = row.get::<_, i64>("max_ts");
    Ok((max_ts > 0).then_some(max_ts))
}

pub async fn replay_warehouse_stats(client: &Client) -> Result<ReplayWarehouseStats> {
    let order_books = table_count_and_max(client, "blink.order_book_snapshots").await?;
    let rn1_signals = table_count_and_max(client, "blink.rn1_signals").await?;
    let ticks = table_count_and_max(client, "blink.ticks").await?;

    Ok(ReplayWarehouseStats {
        order_book_snapshots_count: order_books.0,
        rn1_signals_count: rn1_signals.0,
        ticks_count: ticks.0,
        order_book_snapshots_max_timestamp_ms: order_books.1,
        rn1_signals_max_timestamp_ms: rn1_signals.1,
        ticks_max_timestamp_ms: ticks.1,
    })
}

pub async fn latest_replay_timestamp_ms_including_ticks(
    client: &Client,
) -> Result<Option<TimestampMs>> {
    let row = client
        .query_one(
            "SELECT GREATEST(
                COALESCE((SELECT MAX(timestamp_ms) FROM blink.order_book_snapshots), 0),
                COALESCE((SELECT MAX(timestamp_ms) FROM blink.rn1_signals), 0),
                COALESCE((SELECT MAX(timestamp_ms) FROM blink.ticks), 0)
            ) AS max_ts",
            &[],
        )
        .await?;
    let max_ts = row.get::<_, i64>("max_ts");
    Ok((max_ts > 0).then_some(max_ts))
}

pub async fn load_replay_events_from_postgres(
    client: &Client,
    config: &PostgresReplayLoadConfig,
) -> Result<Vec<ReplayEvent>> {
    let mut events = Vec::new();

    let book_rows = client
        .query(
            "SELECT timestamp_ms, token_id, best_bid, best_ask, bid_depth, ask_depth
             FROM blink.order_book_snapshots
             WHERE timestamp_ms >= $1 AND timestamp_ms <= $2
             ORDER BY timestamp_ms ASC, token_id ASC, best_bid ASC, best_ask ASC,
                      bid_depth ASC, ask_depth ASC, ctid ASC",
            &[&config.start_ms, &config.end_ms],
        )
        .await?;
    for (row_index, row) in book_rows.into_iter().enumerate() {
        events.push(book_snapshot_event_from_l1(
            row.get::<_, i64>("timestamp_ms"),
            source_seq(ReplaySource::BookSnapshot, row_index),
            row.get::<_, String>("token_id"),
            row.get::<_, i64>("best_bid"),
            row.get::<_, i64>("best_ask"),
            row.get::<_, i64>("bid_depth"),
            row.get::<_, i64>("ask_depth"),
        ));
    }

    let signal_rows = if let Some(wallet) = config.rn1_wallet.as_deref() {
        client
            .query(
                "SELECT timestamp_ms, token_id, side, price, size, wallet
                 FROM blink.rn1_signals
                 WHERE timestamp_ms >= $1
                   AND timestamp_ms <= $2
                   AND lower(wallet) = lower($3)
                 ORDER BY timestamp_ms ASC, token_id ASC, side ASC, price ASC,
                          size ASC, lower(wallet) ASC, ctid ASC",
                &[&config.start_ms, &config.end_ms, &wallet],
            )
            .await?
    } else {
        client
            .query(
                "SELECT timestamp_ms, token_id, side, price, size, wallet
                 FROM blink.rn1_signals
                 WHERE timestamp_ms >= $1 AND timestamp_ms <= $2
                 ORDER BY timestamp_ms ASC, token_id ASC, side ASC, price ASC,
                          size ASC, lower(wallet) ASC, ctid ASC",
                &[&config.start_ms, &config.end_ms],
            )
            .await?
    };
    for (row_index, row) in signal_rows.into_iter().enumerate() {
        if let Some(event) = signal_event_from_parts(
            row.get::<_, i64>("timestamp_ms"),
            source_seq(ReplaySource::Signal, row_index),
            row.get::<_, String>("token_id"),
            row.get::<_, String>("side"),
            row.get::<_, i64>("price"),
            row.get::<_, i64>("size"),
            row.get::<_, String>("wallet"),
        ) {
            events.push(event);
        }
    }

    if config.tick_mode != TickEventMode::Ignore {
        let tick_rows = client
            .query(
                "SELECT timestamp_ms, token_id, side, price, size, wallet
                 FROM blink.ticks
                 WHERE timestamp_ms >= $1 AND timestamp_ms <= $2
                 ORDER BY timestamp_ms ASC, token_id ASC, side ASC, price ASC,
                          size ASC, wallet ASC, ctid ASC",
                &[&config.start_ms, &config.end_ms],
            )
            .await?;
        for (row_index, row) in tick_rows.into_iter().enumerate() {
            if let Some(event) = tick_event_from_parts(
                config.tick_mode,
                row.get::<_, i64>("timestamp_ms"),
                source_seq(ReplaySource::Tick, row_index),
                row.get::<_, String>("token_id"),
                row.get::<_, String>("side"),
                row.get::<_, i64>("price"),
                row.get::<_, i64>("size"),
            ) {
                events.push(event);
            }
        }
    }

    crate::quant_replay::sort_replay_events(&mut events);
    Ok(events)
}

async fn table_count_and_max(
    client: &Client,
    qualified_table_name: &str,
) -> Result<(i64, Option<TimestampMs>)> {
    let exists = client
        .query_one(
            "SELECT to_regclass($1) IS NOT NULL",
            &[&qualified_table_name],
        )
        .await?
        .get::<_, bool>(0);
    if !exists {
        return Ok((0, None));
    }

    let query = format!(
        "SELECT COUNT(*)::BIGINT AS count, MAX(timestamp_ms) AS max_ts FROM {}",
        qualified_table_name
    );
    let row = client.query_one(&query, &[]).await?;
    Ok((row.get("count"), row.get("max_ts")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplaySource {
    Signal = 1,
    BookSnapshot = 2,
    Tick = 3,
}

fn source_seq(source: ReplaySource, row_index: usize) -> u64 {
    ((source as u64) << 56) | row_index as u64
}

fn book_snapshot_event_from_l1(
    timestamp_ms: TimestampMs,
    seq: u64,
    token_id: String,
    best_bid: i64,
    best_ask: i64,
    bid_depth: i64,
    ask_depth: i64,
) -> ReplayEvent {
    let bids = level_from_l1(best_bid, bid_depth).into_iter().collect();
    let asks = level_from_l1(best_ask, ask_depth).into_iter().collect();
    ReplayEvent {
        timestamp_ms,
        seq,
        kind: ReplayEventKind::BookSnapshot {
            token_id,
            bids,
            asks,
        },
    }
}

fn level_from_l1(price: i64, size: i64) -> Option<ReplayLevel> {
    if price <= 0 || size <= 0 {
        return None;
    }
    Some(ReplayLevel {
        price: price as u64,
        size: size as u64,
    })
}

fn signal_event_from_parts(
    timestamp_ms: TimestampMs,
    seq: u64,
    token_id: String,
    side: String,
    price: i64,
    size: i64,
    wallet: String,
) -> Option<ReplayEvent> {
    let side = parse_side(&side)?;
    if price <= 0 || size <= 0 {
        return None;
    }
    Some(ReplayEvent {
        timestamp_ms,
        seq,
        kind: ReplayEventKind::Signal {
            token_id,
            side,
            price: price as u64,
            size: size as u64,
            wallet,
        },
    })
}

fn tick_event_from_parts(
    mode: TickEventMode,
    timestamp_ms: TimestampMs,
    seq: u64,
    token_id: String,
    side: String,
    price: i64,
    size: i64,
) -> Option<ReplayEvent> {
    let side = parse_side(&side)?;
    if price <= 0 || size < 0 {
        return None;
    }
    let price = price as u64;
    let size = size as u64;
    let kind = match mode {
        TickEventMode::Ignore => return None,
        TickEventMode::AsBookDelta => ReplayEventKind::BookDelta {
            token_id,
            side,
            price,
            size,
        },
        TickEventMode::AsTradeProxy => {
            if size == 0 {
                return None;
            }
            ReplayEventKind::Trade {
                token_id,
                side,
                price,
                size,
            }
        }
    };
    Some(ReplayEvent {
        timestamp_ms,
        seq,
        kind,
    })
}

fn parse_side(side: &str) -> Option<OrderSide> {
    match side.trim().to_ascii_uppercase().as_str() {
        "BUY" | "BID" => Some(OrderSide::Buy),
        "SELL" | "ASK" => Some(OrderSide::Sell),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l1_snapshot_maps_positive_levels_only() {
        let event = book_snapshot_event_from_l1(1_000, 7, "tok".to_string(), 490, 510, 10_000, 0);

        let ReplayEventKind::BookSnapshot { bids, asks, .. } = event.kind else {
            panic!("expected book snapshot");
        };
        assert_eq!(
            bids,
            vec![ReplayLevel {
                price: 490,
                size: 10_000
            }]
        );
        assert!(asks.is_empty());
    }

    #[test]
    fn signal_mapping_rejects_invalid_side_and_non_positive_values() {
        assert!(signal_event_from_parts(
            1_000,
            1,
            "tok".to_string(),
            "HOLD".to_string(),
            500,
            1_000,
            "0xwallet".to_string(),
        )
        .is_none());
        assert!(signal_event_from_parts(
            1_000,
            1,
            "tok".to_string(),
            "BUY".to_string(),
            0,
            1_000,
            "0xwallet".to_string(),
        )
        .is_none());
    }

    #[test]
    fn tick_mapping_can_be_book_delta_or_trade_proxy() {
        let delta = tick_event_from_parts(
            TickEventMode::AsBookDelta,
            1_000,
            1,
            "tok".to_string(),
            "SELL".to_string(),
            510,
            0,
        )
        .expect("zero-size book delta should remove a level");
        assert!(matches!(
            delta.kind,
            ReplayEventKind::BookDelta {
                side: OrderSide::Sell,
                size: 0,
                ..
            }
        ));

        let trade = tick_event_from_parts(
            TickEventMode::AsTradeProxy,
            1_000,
            2,
            "tok".to_string(),
            "BUY".to_string(),
            500,
            1_000,
        )
        .expect("positive-size trade proxy should map");
        assert!(matches!(trade.kind, ReplayEventKind::Trade { .. }));
    }

    #[test]
    fn source_seq_orders_signals_before_same_timestamp_market_data() {
        let mut events = vec![
            ReplayEvent {
                timestamp_ms: 1_000,
                seq: source_seq(ReplaySource::BookSnapshot, 0),
                kind: ReplayEventKind::BookSnapshot {
                    token_id: "tok".to_string(),
                    bids: vec![],
                    asks: vec![],
                },
            },
            ReplayEvent {
                timestamp_ms: 1_000,
                seq: source_seq(ReplaySource::Signal, 0),
                kind: ReplayEventKind::Signal {
                    token_id: "tok".to_string(),
                    side: OrderSide::Buy,
                    price: 500,
                    size: 1_000,
                    wallet: "0xwallet".to_string(),
                },
            },
            ReplayEvent {
                timestamp_ms: 1_000,
                seq: source_seq(ReplaySource::Tick, 0),
                kind: ReplayEventKind::Trade {
                    token_id: "tok".to_string(),
                    side: OrderSide::Buy,
                    price: 500,
                    size: 1_000,
                },
            },
        ];

        crate::quant_replay::sort_replay_events(&mut events);

        assert!(matches!(events[0].kind, ReplayEventKind::Signal { .. }));
        assert!(matches!(
            events[1].kind,
            ReplayEventKind::BookSnapshot { .. }
        ));
        assert!(matches!(events[2].kind, ReplayEventKind::Trade { .. }));
    }

    #[test]
    fn source_seq_preserves_source_row_order() {
        assert!(
            source_seq(ReplaySource::BookSnapshot, 7) < source_seq(ReplaySource::BookSnapshot, 8)
        );
    }

    #[test]
    fn last_24h_config_uses_safe_tick_default() {
        let cfg = PostgresReplayLoadConfig::last_24h_ending_at(100_000, Some("0xabc".into()));
        assert_eq!(cfg.start_ms, 100_000 - 86_400_000);
        assert_eq!(cfg.tick_mode, TickEventMode::Ignore);
    }
}
