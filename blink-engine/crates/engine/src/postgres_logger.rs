//! PostgreSQL data warehouse for the Blink HFT engine.
//!
//! Buffers high-frequency events in an in-process channel and batch-inserts
//! into Postgres. Gracefully skips if Postgres is unavailable.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossbeam_channel::Receiver;
use postgres_native_tls::MakeTlsConnector;
use serde::Serialize;
use tokio_postgres::Client;
use tracing::{error, info};

// ─── Event types ─────────────────────────────────────────────────────────────

#[derive(Serialize, Debug, Clone)]
pub struct OrderBookSnapshot {
    pub timestamp_ms: u64,
    pub token_id: String,
    pub best_bid: u64,
    pub best_ask: u64,
    pub bid_depth: u64,
    pub ask_depth: u64,
    pub spread: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct Rn1SignalRecord {
    pub timestamp_ms: u64,
    pub token_id: String,
    pub side: String,
    pub price: u64,
    pub size: u64,
    pub wallet: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct TradeExecution {
    pub timestamp_ms: u64,
    pub token_id: String,
    pub side: String,
    pub price: u64,
    pub size: u64,
    pub order_id: String,
    pub mode: String,
    pub status: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct SystemMetric {
    pub timestamp_ms: u64,
    pub ws_connected: u8,
    pub msg_per_sec: u64,
    pub latency_min_us: u64,
    pub latency_max_us: u64,
    pub latency_avg_us: u64,
    pub latency_p99_us: u64,
    pub open_positions: u32,
    pub unrealised_pnl: i64,
}

#[derive(Serialize, Debug, Clone)]
pub struct RiskEvent {
    pub timestamp_ms: u64,
    pub event_type: String,
    pub severity: String,
    pub details: String,
    pub nav_usdc: i64,
    pub daily_pnl_cents: i64,
    pub exposure_cents: i64,
}

#[derive(Serialize, Debug, Clone)]
pub struct LatencySample {
    pub timestamp_ms: u64,
    pub operation: String,
    pub latency_us: u64,
    pub token_id: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct EquitySnapshot {
    pub timestamp_ms: u64,
    pub nav_usdc: f64,
    pub cash_usdc: f64,
    pub unrealised_pnl: f64,
    pub open_positions: u32,
}

#[derive(Serialize, Debug, Clone)]
pub struct ClosedTradeFull {
    pub timestamp_ms: u64,
    pub token_id: String,
    pub market_title: String,
    pub side: String,
    pub entry_price: f64,
    pub exit_price: f64,
    pub shares: f64,
    pub realized_pnl: f64,
    pub fees_paid_usdc: f64,
    pub duration_secs: u64,
    pub reason: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct RejectionEventRecord {
    pub timestamp_ms: u64,
    pub reason: String,
    pub token_id: String,
    pub side: String,
    pub signal_price: u64,
    pub signal_size: u64,
    pub signal_source: String,
}

// ─── Envelope ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum WarehouseEvent {
    OrderBook(OrderBookSnapshot),
    Rn1Signal(Rn1SignalRecord),
    Trade(TradeExecution),
    Metric(SystemMetric),
    Risk(RiskEvent),
    Rejection(RejectionEventRecord),
    Latency(LatencySample),
    EquitySnapshot(EquitySnapshot),
    ClosedTrade(ClosedTradeFull),
}

// ─── PostgresLogger ──────────────────────────────────────────────────────────

pub struct PostgresLogger {
    url: String,
}

impl PostgresLogger {
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

        client.execute("CREATE TABLE IF NOT EXISTS blink.order_book_snapshots (
            timestamp_ms BIGINT, token_id TEXT, best_bid BIGINT, best_ask BIGINT, bid_depth BIGINT, ask_depth BIGINT, spread BIGINT
        )", &[]).await?;

        client
            .execute(
                "CREATE TABLE IF NOT EXISTS blink.rn1_signals (
            timestamp_ms BIGINT, token_id TEXT, side TEXT, price BIGINT, size BIGINT, wallet TEXT
        )",
                &[],
            )
            .await?;

        client.execute("CREATE TABLE IF NOT EXISTS blink.trade_executions (
            timestamp_ms BIGINT, token_id TEXT, side TEXT, price BIGINT, size BIGINT, order_id TEXT, mode TEXT, status TEXT
        )", &[]).await?;

        client.execute("CREATE TABLE IF NOT EXISTS blink.system_metrics (
            timestamp_ms BIGINT, ws_connected INT, msg_per_sec BIGINT, latency_min_us BIGINT, latency_max_us BIGINT, latency_avg_us BIGINT, latency_p99_us BIGINT, open_positions INT, unrealised_pnl BIGINT
        )", &[]).await?;

        client.execute("CREATE TABLE IF NOT EXISTS blink.risk_events (
            timestamp_ms BIGINT, event_type TEXT, severity TEXT, details TEXT, nav_usdc BIGINT, daily_pnl_cents BIGINT, exposure_cents BIGINT
        )", &[]).await?;

        client.execute("CREATE TABLE IF NOT EXISTS blink.rejection_events (
            timestamp_ms BIGINT, reason TEXT, token_id TEXT, side TEXT, signal_price BIGINT, signal_size BIGINT, signal_source TEXT
        )", &[]).await?;

        client
            .execute(
                "CREATE TABLE IF NOT EXISTS blink.latency_samples (
            timestamp_ms BIGINT, operation TEXT, latency_us BIGINT, token_id TEXT
        )",
                &[],
            )
            .await?;

        client.execute("CREATE TABLE IF NOT EXISTS blink.equity_snapshots (
            timestamp_ms BIGINT, nav_usdc DOUBLE PRECISION, cash_usdc DOUBLE PRECISION, unrealised_pnl DOUBLE PRECISION, open_positions INT
        )", &[]).await?;

        client.execute("CREATE TABLE IF NOT EXISTS blink.closed_trades_full (
            timestamp_ms BIGINT, token_id TEXT, market_title TEXT, side TEXT, entry_price DOUBLE PRECISION, exit_price DOUBLE PRECISION, shares DOUBLE PRECISION, realized_pnl DOUBLE PRECISION, fees_paid_usdc DOUBLE PRECISION, duration_secs BIGINT, reason TEXT
        )", &[]).await?;

        info!("PostgreSQL warehouse schema ready (9 tables)");
        Ok(())
    }

    pub async fn run(self, rx: Receiver<WarehouseEvent>) {
        const BATCH_SIZE: usize = 500;
        const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

        let mut ob_batch = Vec::with_capacity(BATCH_SIZE);
        let mut rn1_batch = Vec::with_capacity(BATCH_SIZE);
        let mut tx_batch = Vec::with_capacity(BATCH_SIZE);
        let mut met_batch = Vec::with_capacity(BATCH_SIZE);
        let mut risk_batch = Vec::with_capacity(BATCH_SIZE);
        let mut rej_batch = Vec::with_capacity(BATCH_SIZE);
        let mut lat_batch = Vec::with_capacity(BATCH_SIZE);
        let mut eq_batch = Vec::with_capacity(BATCH_SIZE);
        let mut ct_batch = Vec::with_capacity(BATCH_SIZE);

        let mut last_flush = Instant::now();

        loop {
            while let Ok(event) = rx.try_recv() {
                match event {
                    WarehouseEvent::OrderBook(e) => ob_batch.push(e),
                    WarehouseEvent::Rn1Signal(e) => rn1_batch.push(e),
                    WarehouseEvent::Trade(e) => tx_batch.push(e),
                    WarehouseEvent::Metric(e) => met_batch.push(e),
                    WarehouseEvent::Risk(e) => risk_batch.push(e),
                    WarehouseEvent::Rejection(e) => rej_batch.push(e),
                    WarehouseEvent::Latency(e) => lat_batch.push(e),
                    WarehouseEvent::EquitySnapshot(e) => eq_batch.push(e),
                    WarehouseEvent::ClosedTrade(e) => ct_batch.push(e),
                }

                if ob_batch.len()
                    + rn1_batch.len()
                    + tx_batch.len()
                    + met_batch.len()
                    + risk_batch.len()
                    + rej_batch.len()
                    + lat_batch.len()
                    + eq_batch.len()
                    + ct_batch.len()
                    >= BATCH_SIZE
                {
                    self.flush_all(
                        &mut ob_batch,
                        &mut rn1_batch,
                        &mut tx_batch,
                        &mut met_batch,
                        &mut risk_batch,
                        &mut rej_batch,
                        &mut lat_batch,
                        &mut eq_batch,
                        &mut ct_batch,
                    )
                    .await;
                    last_flush = Instant::now();
                }
            }

            if last_flush.elapsed() >= FLUSH_INTERVAL {
                if !ob_batch.is_empty()
                    || !rn1_batch.is_empty()
                    || !tx_batch.is_empty()
                    || !met_batch.is_empty()
                    || !risk_batch.is_empty()
                    || !rej_batch.is_empty()
                    || !lat_batch.is_empty()
                    || !eq_batch.is_empty()
                    || !ct_batch.is_empty()
                {
                    self.flush_all(
                        &mut ob_batch,
                        &mut rn1_batch,
                        &mut tx_batch,
                        &mut met_batch,
                        &mut risk_batch,
                        &mut rej_batch,
                        &mut lat_batch,
                        &mut eq_batch,
                        &mut ct_batch,
                    )
                    .await;
                }
                last_flush = Instant::now();
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn flush_all(
        &self,
        ob: &mut Vec<OrderBookSnapshot>,
        rn1: &mut Vec<Rn1SignalRecord>,
        tx: &mut Vec<TradeExecution>,
        met: &mut Vec<SystemMetric>,
        risk: &mut Vec<RiskEvent>,
        rej: &mut Vec<RejectionEventRecord>,
        lat: &mut Vec<LatencySample>,
        eq: &mut Vec<EquitySnapshot>,
        ct: &mut Vec<ClosedTradeFull>,
    ) {
        if let Ok(client) = self.connect().await {
            for o in ob.drain(..) {
                let _ = client.execute("INSERT INTO blink.order_book_snapshots (timestamp_ms, token_id, best_bid, best_ask, bid_depth, ask_depth, spread) VALUES ($1, $2, $3, $4, $5, $6, $7)", &[&(o.timestamp_ms as i64), &o.token_id, &(o.best_bid as i64), &(o.best_ask as i64), &(o.bid_depth as i64), &(o.ask_depth as i64), &(o.spread as i64)]).await;
            }
            for r in rn1.drain(..) {
                let _ = client.execute("INSERT INTO blink.rn1_signals (timestamp_ms, token_id, side, price, size, wallet) VALUES ($1, $2, $3, $4, $5, $6)", &[&(r.timestamp_ms as i64), &r.token_id, &r.side, &(r.price as i64), &(r.size as i64), &r.wallet]).await;
            }
            for t in tx.drain(..) {
                let _ = client.execute("INSERT INTO blink.trade_executions (timestamp_ms, token_id, side, price, size, order_id, mode, status) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)", &[&(t.timestamp_ms as i64), &t.token_id, &t.side, &(t.price as i64), &(t.size as i64), &t.order_id, &t.mode, &t.status]).await;
            }
            for m in met.drain(..) {
                let _ = client.execute("INSERT INTO blink.system_metrics (timestamp_ms, ws_connected, msg_per_sec, latency_min_us, latency_max_us, latency_avg_us, latency_p99_us, open_positions, unrealised_pnl) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)", &[&(m.timestamp_ms as i64), &(m.ws_connected as i32), &(m.msg_per_sec as i64), &(m.latency_min_us as i64), &(m.latency_max_us as i64), &(m.latency_avg_us as i64), &(m.latency_p99_us as i64), &(m.open_positions as i32), &(m.unrealised_pnl as i64)]).await;
            }
            for r in risk.drain(..) {
                let _ = client.execute("INSERT INTO blink.risk_events (timestamp_ms, event_type, severity, details, nav_usdc, daily_pnl_cents, exposure_cents) VALUES ($1, $2, $3, $4, $5, $6, $7)", &[&(r.timestamp_ms as i64), &r.event_type, &r.severity, &r.details, &(r.nav_usdc as i64), &(r.daily_pnl_cents as i64), &(r.exposure_cents as i64)]).await;
            }
            for r in rej.drain(..) {
                let _ = client.execute("INSERT INTO blink.rejection_events (timestamp_ms, reason, token_id, side, signal_price, signal_size, signal_source) VALUES ($1, $2, $3, $4, $5, $6, $7)", &[&(r.timestamp_ms as i64), &r.reason, &r.token_id, &r.side, &(r.signal_price as i64), &(r.signal_size as i64), &r.signal_source]).await;
            }
            for l in lat.drain(..) {
                let _ = client.execute("INSERT INTO blink.latency_samples (timestamp_ms, operation, latency_us, token_id) VALUES ($1, $2, $3, $4)", &[&(l.timestamp_ms as i64), &l.operation, &(l.latency_us as i64), &l.token_id]).await;
            }
            for e in eq.drain(..) {
                let _ = client.execute("INSERT INTO blink.equity_snapshots (timestamp_ms, nav_usdc, cash_usdc, unrealised_pnl, open_positions) VALUES ($1, $2, $3, $4, $5)", &[&(e.timestamp_ms as i64), &e.nav_usdc, &e.cash_usdc, &e.unrealised_pnl, &(e.open_positions as i32)]).await;
            }
            for c in ct.drain(..) {
                let _ = client.execute("INSERT INTO blink.closed_trades_full (timestamp_ms, token_id, market_title, side, entry_price, exit_price, shares, realized_pnl, fees_paid_usdc, duration_secs, reason) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)", &[&(c.timestamp_ms as i64), &c.token_id, &c.market_title, &c.side, &c.entry_price, &c.exit_price, &c.shares, &c.realized_pnl, &c.fees_paid_usdc, &(c.duration_secs as i64), &c.reason]).await;
            }
        }
    }
}

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
        assert!(now_ms() > 1_700_000_000_000);
    }
}
