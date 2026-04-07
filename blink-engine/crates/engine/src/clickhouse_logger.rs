//! Extended ClickHouse data warehouse for the Blink HFT engine.
//!
//! Buffers high-frequency events in an in-process channel and batch-inserts
//! into ClickHouse via the native HTTP protocol.  Gracefully skips if
//! ClickHouse is unavailable — the engine never crashes due to telemetry.
//!
//! # Tables
//!
//! | Table | Description |
//! |-------|-------------|
//! | `blink.order_book_snapshots` | Periodic top-of-book snapshots per market |
//! | `blink.rn1_signals` | Detected RN1 whale signals |
//! | `blink.trade_executions` | Orders placed by the engine (paper or live) |
//! | `blink.system_metrics` | Periodic engine health metrics |
//!
//! # Activation
//!
//! Set `CLICKHOUSE_URL=http://localhost:8123` in `.env`.  When unset the
//! logger is a silent no-op.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clickhouse::{Client, Row};
use crossbeam_channel::Receiver;
use serde::Serialize;
use tracing::{debug, info, warn};

// ─── Event types ─────────────────────────────────────────────────────────────

/// A periodic top-of-book snapshot for one market.
#[derive(Row, Serialize, Debug, Clone)]
pub struct OrderBookSnapshot {
    pub timestamp_ms: u64,
    pub token_id:     String,
    pub best_bid:     u64,
    pub best_ask:     u64,
    pub bid_depth:    u64,
    pub ask_depth:    u64,
    pub spread:       u64,
}

/// A detected RN1 whale signal.
#[derive(Row, Serialize, Debug, Clone)]
pub struct Rn1SignalRecord {
    pub timestamp_ms: u64,
    pub token_id:     String,
    pub side:         String,
    pub price:        u64,
    pub size:         u64,
    pub wallet:       String,
}

/// An order placed by the engine (paper or live).
#[derive(Row, Serialize, Debug, Clone)]
pub struct TradeExecution {
    pub timestamp_ms: u64,
    pub token_id:     String,
    pub side:         String,
    pub price:        u64,
    pub size:         u64,
    pub order_id:     String,
    pub mode:         String,
    pub status:       String,
}

/// Periodic engine health snapshot.
#[derive(Row, Serialize, Debug, Clone)]
pub struct SystemMetric {
    pub timestamp_ms:     u64,
    pub ws_connected:     u8,
    pub msg_per_sec:      u64,
    pub latency_min_us:   u64,
    pub latency_max_us:   u64,
    pub latency_avg_us:   u64,
    pub latency_p99_us:   u64,
    pub open_positions:   u32,
    pub unrealised_pnl:   i64,
}

/// A risk event (circuit breaker trip, VaR breach, daily loss limit hit, etc).
#[derive(Row, Serialize, Debug, Clone)]
pub struct RiskEvent {
    pub timestamp_ms:     u64,
    /// One of: "circuit_breaker", "var_breach", "daily_loss", "kill_switch",
    ///         "rate_limit", "order_too_large", "too_many_positions"
    pub event_type:       String,
    pub severity:         String,
    pub details:          String,
    /// NAV at the time of the event.
    pub nav_usdc:         i64,
    /// Daily P&L at the time of the event (cents).
    pub daily_pnl_cents:  i64,
    /// Rolling exposure (cents).
    pub exposure_cents:   i64,
}

/// Individual latency sample (one per order lifecycle event).
#[derive(Row, Serialize, Debug, Clone)]
pub struct LatencySample {
    pub timestamp_ms:     u64,
    /// One of: "signal_detect", "order_sign", "order_submit", "order_ack",
    ///         "ws_roundtrip", "book_update"
    pub operation:        String,
    /// Latency in microseconds.
    pub latency_us:       u64,
    pub token_id:         String,
}

// ─── Envelope ────────────────────────────────────────────────────────────────

/// Unified event envelope sent through the channel.
#[derive(Debug, Clone)]
pub enum WarehouseEvent {
    OrderBook(OrderBookSnapshot),
    Rn1Signal(Rn1SignalRecord),
    Trade(TradeExecution),
    Metric(SystemMetric),
    Risk(RiskEvent),
    Latency(LatencySample),
}

// ─── ClickHouseLogger ────────────────────────────────────────────────────────

/// Batch-insert logger for the ClickHouse data warehouse.
pub struct ClickHouseLogger {
    client: Client,
}

impl ClickHouseLogger {
    /// Creates a new logger pointed at `url` (e.g. `"http://localhost:8123"`).
    pub fn new(url: &str) -> Self {
        let client = Client::default().with_url(url);
        Self { client }
    }

    /// Creates the `blink` database and all warehouse tables if they do not
    /// already exist.  Errors are logged but do **not** terminate the engine.
    pub async fn ensure_schema(&self) -> Result<()> {
        self.client
            .query("CREATE DATABASE IF NOT EXISTS blink")
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.order_book_snapshots (
                    timestamp_ms UInt64,
                    token_id     String,
                    best_bid     UInt64,
                    best_ask     UInt64,
                    bid_depth    UInt64,
                    ask_depth    UInt64,
                    spread       UInt64
                ) ENGINE = MergeTree()
                ORDER BY (token_id, timestamp_ms)",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.rn1_signals (
                    timestamp_ms UInt64,
                    token_id     String,
                    side         String,
                    price        UInt64,
                    size         UInt64,
                    wallet       String
                ) ENGINE = MergeTree()
                ORDER BY (token_id, timestamp_ms)",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.trade_executions (
                    timestamp_ms UInt64,
                    token_id     String,
                    side         String,
                    price        UInt64,
                    size         UInt64,
                    order_id     String,
                    mode         String,
                    status       String
                ) ENGINE = MergeTree()
                ORDER BY (token_id, timestamp_ms)",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.system_metrics (
                    timestamp_ms     UInt64,
                    ws_connected     UInt8,
                    msg_per_sec      UInt64,
                    latency_min_us   UInt64,
                    latency_max_us   UInt64,
                    latency_avg_us   UInt64,
                    latency_p99_us   UInt64,
                    open_positions   UInt32,
                    unrealised_pnl   Int64
                ) ENGINE = MergeTree()
                ORDER BY timestamp_ms",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.risk_events (
                    timestamp_ms     UInt64,
                    event_type       String,
                    severity         String,
                    details          String,
                    nav_usdc         Int64,
                    daily_pnl_cents  Int64,
                    exposure_cents   Int64
                ) ENGINE = MergeTree()
                ORDER BY (event_type, timestamp_ms)",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.latency_samples (
                    timestamp_ms UInt64,
                    operation    String,
                    latency_us   UInt64,
                    token_id     String
                ) ENGINE = MergeTree()
                ORDER BY (operation, timestamp_ms)",
            )
            .execute()
            .await?;

        // ── Bullpen integration tables ─────────────────────────────────────
        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.bullpen_commands (
                    timestamp_ms UInt64,
                    command      String,
                    success      UInt8,
                    latency_ms   UInt32,
                    error_msg    String
                ) ENGINE = MergeTree()
                ORDER BY (command, timestamp_ms)",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.bullpen_discoveries (
                    timestamp_ms    UInt64,
                    lens            String,
                    markets_found   UInt32,
                    new_markets     UInt32
                ) ENGINE = MergeTree()
                ORDER BY (lens, timestamp_ms)",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.bullpen_smart_money (
                    timestamp_ms       UInt64,
                    wallet             String,
                    action             String,
                    market             String,
                    amount_usd         Float64,
                    price              Float64,
                    convergence_count  UInt32
                ) ENGINE = MergeTree()
                ORDER BY (wallet, timestamp_ms)",
            )
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.bullpen_reconciliation (
                    timestamp_ms   UInt64,
                    check_type     String,
                    market         String,
                    blink_value    Float64,
                    bullpen_value  Float64,
                    drift          Float64,
                    drift_pct      Float64,
                    alert          UInt8
                ) ENGINE = MergeTree()
                ORDER BY (check_type, timestamp_ms)",
            )
            .execute()
            .await?;

        info!("ClickHouse warehouse schema ready (10 tables)");
        Ok(())
    }

    /// Runs the batch-insert loop, draining `rx` indefinitely.
    ///
    /// Flushes every **500 records** or every **1 second** — whichever comes
    /// first.  Errors are logged but never terminate the loop.
    pub async fn run(self, rx: Receiver<WarehouseEvent>) {
        const BATCH_SIZE: usize = 500;
        const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

        let mut ob_batch:  Vec<OrderBookSnapshot> = Vec::with_capacity(BATCH_SIZE);
        let mut rn1_batch: Vec<Rn1SignalRecord>   = Vec::with_capacity(BATCH_SIZE);
        let mut tx_batch:  Vec<TradeExecution>     = Vec::with_capacity(BATCH_SIZE);
        let mut met_batch: Vec<SystemMetric>       = Vec::with_capacity(BATCH_SIZE);
        let mut risk_batch: Vec<RiskEvent>         = Vec::with_capacity(BATCH_SIZE);
        let mut lat_batch:  Vec<LatencySample>     = Vec::with_capacity(BATCH_SIZE);

        let mut last_flush = Instant::now();

        loop {
            while let Ok(event) = rx.try_recv() {
                match event {
                    WarehouseEvent::OrderBook(e) => ob_batch.push(e),
                    WarehouseEvent::Rn1Signal(e) => rn1_batch.push(e),
                    WarehouseEvent::Trade(e)     => tx_batch.push(e),
                    WarehouseEvent::Metric(e)    => met_batch.push(e),
                    WarehouseEvent::Risk(e)      => risk_batch.push(e),
                    WarehouseEvent::Latency(e)   => lat_batch.push(e),
                }

                let total = ob_batch.len() + rn1_batch.len()
                          + tx_batch.len() + met_batch.len()
                          + risk_batch.len() + lat_batch.len();
                if total >= BATCH_SIZE {
                    self.flush_all(&mut ob_batch, &mut rn1_batch,
                                   &mut tx_batch, &mut met_batch,
                                   &mut risk_batch, &mut lat_batch).await;
                    last_flush = Instant::now();
                }
            }

            if last_flush.elapsed() >= FLUSH_INTERVAL {
                let total = ob_batch.len() + rn1_batch.len()
                          + tx_batch.len() + met_batch.len()
                          + risk_batch.len() + lat_batch.len();
                if total > 0 {
                    self.flush_all(&mut ob_batch, &mut rn1_batch,
                                   &mut tx_batch, &mut met_batch,
                                   &mut risk_batch, &mut lat_batch).await;
                }
                last_flush = Instant::now();
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn flush_all(
        &self,
        ob:   &mut Vec<OrderBookSnapshot>,
        rn1:  &mut Vec<Rn1SignalRecord>,
        tx:   &mut Vec<TradeExecution>,
        met:  &mut Vec<SystemMetric>,
        risk: &mut Vec<RiskEvent>,
        lat:  &mut Vec<LatencySample>,
    ) {
        self.flush_table("blink.order_book_snapshots", ob).await;
        self.flush_table("blink.rn1_signals", rn1).await;
        self.flush_table("blink.trade_executions", tx).await;
        self.flush_table("blink.system_metrics", met).await;
        self.flush_table("blink.risk_events", risk).await;
        self.flush_table("blink.latency_samples", lat).await;
    }

    async fn flush_table<T: Row + Serialize>(
        &self,
        table: &str,
        batch: &mut Vec<T>,
    ) {
        if batch.is_empty() {
            return;
        }
        let n = batch.len();
        match self.client.insert(table) {
            Ok(mut inserter) => {
                for row in batch.drain(..) {
                    if let Err(e) = inserter.write(&row).await {
                        warn!(table, "ClickHouse write error: {e}");
                    }
                }
                if let Err(e) = inserter.end().await {
                    warn!(table, "ClickHouse flush error: {e}");
                } else {
                    debug!(table, n, "ClickHouse flushed");
                }
            }
            Err(e) => {
                warn!(table, "ClickHouse insert init error: {e}");
                batch.clear();
            }
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Returns the current time as Unix milliseconds.
#[inline]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_returns_positive() {
        assert!(now_ms() > 1_700_000_000_000); // sanity: after ~2023
    }

    #[test]
    fn warehouse_event_roundtrip() {
        let snap = OrderBookSnapshot {
            timestamp_ms: now_ms(),
            token_id: "abc123".into(),
            best_bid: 650,
            best_ask: 660,
            bid_depth: 5000,
            ask_depth: 4500,
            spread: 10,
        };
        let event = WarehouseEvent::OrderBook(snap.clone());
        match event {
            WarehouseEvent::OrderBook(s) => assert_eq!(s.token_id, "abc123"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn rn1_signal_record_creation() {
        let rec = Rn1SignalRecord {
            timestamp_ms: now_ms(),
            token_id: "token1".into(),
            side: "BUY".into(),
            price: 500,
            size: 100_000,
            wallet: "0xabc".into(),
        };
        assert_eq!(rec.side, "BUY");
    }

    #[test]
    fn trade_execution_creation() {
        let rec = TradeExecution {
            timestamp_ms: now_ms(),
            token_id: "token2".into(),
            side: "SELL".into(),
            price: 700,
            size: 50_000,
            order_id: "ord-123".into(),
            mode: "paper".into(),
            status: "filled".into(),
        };
        assert_eq!(rec.mode, "paper");
    }

    #[test]
    fn system_metric_creation() {
        let m = SystemMetric {
            timestamp_ms: now_ms(),
            ws_connected: 1,
            msg_per_sec: 420,
            latency_min_us: 10,
            latency_max_us: 300,
            latency_avg_us: 50,
            latency_p99_us: 250,
            open_positions: 2,
            unrealised_pnl: -500,
        };
        assert_eq!(m.ws_connected, 1);
        assert_eq!(m.unrealised_pnl, -500);
    }
}
