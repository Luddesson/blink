//! ClickHouse tick-level data warehouse integration.
//!
//! When `CLICKHOUSE_URL` is set, every CLOB order event received from the
//! WebSocket feed is forwarded here via an in-process channel and batch-inserted
//! into `blink.ticks` using the native ClickHouse HTTP protocol.
//!
//! # Schema
//! ```sql
//! CREATE TABLE IF NOT EXISTS blink.ticks (
//!     timestamp_ms UInt64,
//!     token_id     String,
//!     side         String,
//!     price        UInt64,
//!     size         UInt64,
//!     wallet       String
//! ) ENGINE = MergeTree()
//! ORDER BY (token_id, timestamp_ms);
//! ```
//!
//! # Activation
//! Set `CLICKHOUSE_URL=http://localhost:8123` in `.env` or the process environment.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clickhouse::{Client, Row};
use crossbeam_channel::Receiver;
use serde::Serialize;
use tracing::{debug, info, warn};

// ─── TickRecord ───────────────────────────────────────────────────────────────

/// A single CLOB order event stored in ClickHouse.
#[derive(Row, Serialize, Debug, Clone)]
pub struct TickRecord {
    /// Unix timestamp of when the event was received, in milliseconds.
    pub timestamp_ms: u64,
    /// Polymarket token (condition) ID.
    pub token_id: String,
    /// `"BUY"` or `"SELL"`.
    pub side: String,
    /// Limit price × 1 000 (e.g. `0.65` → `650`).
    pub price: u64,
    /// Order size × 1 000.
    pub size: u64,
    /// On-chain wallet address of the order owner.
    pub wallet: String,
}

// ─── TickRecorder ─────────────────────────────────────────────────────────────

/// Handles batched insertion of [`TickRecord`]s into ClickHouse.
pub struct TickRecorder {
    client: Client,
}

impl TickRecorder {
    /// Creates a new recorder pointed at `url`
    /// (e.g. `"http://localhost:8123"`).
    pub fn new(url: &str) -> Self {
        let client = Client::default().with_url(url);
        Self { client }
    }

    /// Creates `blink` database and `blink.ticks` table if they do not exist.
    pub async fn ensure_schema(&self) -> Result<()> {
        self.client
            .query("CREATE DATABASE IF NOT EXISTS blink")
            .execute()
            .await?;

        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS blink.ticks (
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

        info!("ClickHouse schema ready (blink.ticks)");
        Ok(())
    }

    /// Runs the batch-insert loop, draining `rx` indefinitely.
    ///
    /// Flushes every **1 000 records** or every **1 second** — whichever
    /// comes first.  Errors are logged but do not terminate the loop.
    ///
    /// Never returns under normal operation.
    pub async fn run(self, rx: Receiver<TickRecord>) {
        const BATCH_SIZE: usize = 1_000;
        const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

        let mut batch: Vec<TickRecord> = Vec::with_capacity(BATCH_SIZE);
        let mut last_flush: std::time::Instant = std::time::Instant::now();

        loop {
            // Drain all pending records without blocking.
            while let Ok(tick) = rx.try_recv() {
                batch.push(tick);
                if batch.len() >= BATCH_SIZE {
                    self.flush_batch(&mut batch).await;
                    last_flush = std::time::Instant::now();
                }
            }

            // Time-based flush.
            if !batch.is_empty() && last_flush.elapsed() >= FLUSH_INTERVAL {
                self.flush_batch(&mut batch).await;
                last_flush = std::time::Instant::now();
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn flush_batch(&self, batch: &mut Vec<TickRecord>) {
        let n = batch.len();
        match self.client.insert("blink.ticks") {
            Ok(mut inserter) => {
                for tick in batch.drain(..) {
                    if let Err(e) = inserter.write(&tick).await {
                        warn!("ClickHouse write error: {e}");
                    }
                }
                if let Err(e) = inserter.end().await {
                    warn!("ClickHouse flush error: {e}");
                } else {
                    debug!("ClickHouse flushed {n} ticks");
                }
            }
            Err(e) => {
                warn!("ClickHouse insert init error: {e}");
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
