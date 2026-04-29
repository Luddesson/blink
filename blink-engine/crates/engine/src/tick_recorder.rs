//! Postgres tick-level data warehouse integration.
//!
//! When `POSTGRES_URL` is set, every CLOB order event received from the
//! WebSocket feed is forwarded here via an in-process channel and batch-inserted
//! into `blink.ticks`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossbeam_channel::Receiver;
use postgres_native_tls::MakeTlsConnector;
use serde::Serialize;
use tokio_postgres::Client;
use tracing::{error, info};

// ─── TickRecord ───────────────────────────────────────────────────────────────

#[derive(Serialize, Debug, Clone)]
pub struct TickRecord {
    pub timestamp_ms: u64,
    pub token_id: String,
    pub side: String,
    pub price: u64,
    pub size: u64,
    pub wallet: String,
}

// ─── TickRecorder ─────────────────────────────────────────────────────────────

pub struct TickRecorder {
    url: String,
}

impl TickRecorder {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
        }
    }

    async fn connect(&self) -> Result<Client> {
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()?;
        let connector = MakeTlsConnector::new(connector);
        let (client, connection) = tokio_postgres::connect(&self.url, connector).await?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("Postgres connection error: {}", e);
            }
        });

        Ok(client)
    }

    pub async fn ensure_schema(&self) -> Result<()> {
        let client = self.connect().await?;

        client
            .execute("CREATE SCHEMA IF NOT EXISTS blink", &[])
            .await?;

        client
            .execute(
                "CREATE TABLE IF NOT EXISTS blink.ticks (
            timestamp_ms BIGINT, token_id TEXT, side TEXT, price BIGINT, size BIGINT, wallet TEXT
        )",
                &[],
            )
            .await?;

        info!("PostgreSQL schema ready (blink.ticks)");
        Ok(())
    }

    pub async fn run(self, rx: Receiver<TickRecord>) {
        const BATCH_SIZE: usize = 1_000;
        const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

        let mut batch: Vec<TickRecord> = Vec::with_capacity(BATCH_SIZE);
        let mut last_flush = std::time::Instant::now();

        loop {
            while let Ok(tick) = rx.try_recv() {
                batch.push(tick);
                if batch.len() >= BATCH_SIZE {
                    self.flush_batch(&mut batch).await;
                    last_flush = std::time::Instant::now();
                }
            }

            if !batch.is_empty() && last_flush.elapsed() >= FLUSH_INTERVAL {
                self.flush_batch(&mut batch).await;
                last_flush = std::time::Instant::now();
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn flush_batch(&self, batch: &mut Vec<TickRecord>) {
        if let Ok(client) = self.connect().await {
            for tick in batch.drain(..) {
                let _ = client.execute("INSERT INTO blink.ticks (timestamp_ms, token_id, side, price, size, wallet) VALUES ($1, $2, $3, $4, $5, $6)", &[&(tick.timestamp_ms as i64), &tick.token_id, &tick.side, &(tick.price as i64), &(tick.size as i64), &tick.wallet]).await;
            }
        } else {
            batch.clear();
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[inline]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
