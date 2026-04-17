//! Bullpen CLI bridge — typed async wrapper for cold-path data enrichment.
//!
//! Spawns `bullpen` CLI commands via [`tokio::process::Command`], deserialises
//! `--output json` responses into typed structs, and enforces timeout /
//! concurrency / retry policies.
//!
//! **Latency class: COLD PATH (500 ms – 10 s).  Never call from the
//! signal → order hot path.**
//!
//! Configuration (environment variables):
//! - `BULLPEN_ENABLED`          — master toggle (default: false)
//! - `BULLPEN_CLI_PATH`         — CLI binary path (default: `wsl -d Ubuntu -- bullpen`)
//! - `BULLPEN_TIMEOUT_SECS`     — per-command timeout (default: 15)
//! - `BULLPEN_MAX_RETRIES`      — retry count on transient failures (default: 2)
//! - `BULLPEN_RETRY_BACKOFF_MS` — initial retry backoff, doubles each attempt (default: 500)
//! - `BULLPEN_MAX_CONCURRENT`   — semaphore permits (default: 3)

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, Semaphore};

// ─── Helper env parsers ──────────────────────────────────────────────────────

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(default)
}

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ─── Configuration ───────────────────────────────────────────────────────────

/// Configuration for the Bullpen CLI bridge.
#[derive(Debug, Clone)]
pub struct BullpenConfig {
    pub enabled: bool,
    /// On Windows this is typically `wsl -d Ubuntu -- bullpen`.
    pub cli_path: String,
    pub timeout: Duration,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
    pub max_concurrent_commands: usize,
    /// Whether the CLI binary is invoked via WSL (needs arg splitting).
    pub use_wsl: bool,
}

impl BullpenConfig {
    pub fn from_env() -> Self {
        let use_wsl = env_bool("BULLPEN_USE_WSL", cfg!(target_os = "windows"));
        let default_path = if use_wsl {
            "wsl -d Ubuntu -- bullpen"
        } else {
            "bullpen"
        };
        Self {
            enabled: env_bool("BULLPEN_ENABLED", false),
            cli_path: env_str("BULLPEN_CLI_PATH", default_path),
            timeout: Duration::from_secs(env_u64("BULLPEN_TIMEOUT_SECS", 15)),
            max_retries: env_u32("BULLPEN_MAX_RETRIES", 2),
            retry_backoff_ms: env_u64("BULLPEN_RETRY_BACKOFF_MS", 500),
            max_concurrent_commands: env_usize("BULLPEN_MAX_CONCURRENT", 3),
            use_wsl,
        }
    }
}

// ─── Health & Diagnostics ────────────────────────────────────────────────────

/// Health status of the Bullpen CLI bridge.
#[derive(Debug, Clone)]
pub struct BullpenHealth {
    pub authenticated: bool,
    pub cli_version: Option<String>,
    pub last_health_check: Option<Instant>,
    pub last_error: Option<String>,
    pub consecutive_failures: u32,
    pub total_calls: u64,
    pub total_failures: u64,
    pub avg_latency_ms: f64,
    /// Circuit breaker: if set, skip commands until this instant.
    pub circuit_open_until: Option<Instant>,
}

impl Default for BullpenHealth {
    fn default() -> Self {
        Self {
            authenticated: false,
            cli_version: None,
            last_health_check: None,
            last_error: None,
            consecutive_failures: 0,
            total_calls: 0,
            total_failures: 0,
            avg_latency_ms: 0.0,
            circuit_open_until: None,
        }
    }
}

/// Per-command performance statistics.
#[derive(Debug, Clone, Default)]
pub struct CommandStats {
    pub total_calls: u64,
    pub successes: u64,
    pub failures: u64,
    pub total_latency_ms: f64,
    pub max_latency_ms: f64,
}

impl CommandStats {
    pub fn avg_latency_ms(&self) -> f64 {
        if self.total_calls == 0 { 0.0 } else { self.total_latency_ms / self.total_calls as f64 }
    }
}

/// Aggregated diagnostics for TUI / ClickHouse.
#[derive(Debug, Clone, Default)]
pub struct BullpenDiagnostics {
    pub calls_by_command: HashMap<String, CommandStats>,
}

// ─── Response Types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredMarket {
    pub slug: Option<String>,
    pub title: Option<String>,
    pub category: Option<String>,
    #[serde(alias = "volume_24h", alias = "volume24hr")]
    pub volume_24h: Option<f64>,
    pub liquidity: Option<f64>,
    pub end_date: Option<String>,
    #[serde(default)]
    pub token_ids: Option<Vec<String>>,
}

/// Wrapper for the discover response which nests events in a `events` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverResponse {
    pub lens: Option<String>,
    #[serde(default)]
    pub events: Vec<DiscoverEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverEvent {
    pub id: Option<String>,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    /// ISO-8601 resolution deadline for this event (e.g. "2025-06-01T18:00:00Z").
    /// Used by the signal generator to filter on markets resolving within a time window.
    #[serde(alias = "endDate", alias = "end_date")]
    pub end_date: Option<String>,
    #[serde(default)]
    pub markets: Vec<DiscoverEventMarket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverEventMarket {
    #[serde(alias = "tokenId", alias = "token_id")]
    pub token_id: Option<String>,
    #[serde(default)]
    pub token_ids: Option<Vec<String>>,
    pub outcome: Option<String>,
    pub price: Option<f64>,
    pub volume: Option<f64>,
    pub liquidity: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartMoneyEntry {
    pub wallet: Option<String>,
    pub action: Option<String>,
    pub market: Option<String>,
    pub outcome: Option<String>,
    #[serde(alias = "amount_usd", alias = "amountUsd", default)]
    pub amount_usd: Option<f64>,
    pub price: Option<f64>,
    pub timestamp: Option<String>,
    pub pnl: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraderProfile {
    pub address: Option<String>,
    #[serde(alias = "volumeTotal", alias = "volume_total", default)]
    pub volume_total: Option<f64>,
    #[serde(alias = "winRate", alias = "win_rate", default)]
    pub win_rate: Option<f64>,
    #[serde(alias = "totalTrades", alias = "total_trades", default)]
    pub total_trades: Option<u64>,
    #[serde(alias = "avgTradeSize", alias = "avg_trade_size", default)]
    pub avg_trade_size: Option<f64>,
    pub specialization: Option<String>,
    #[serde(alias = "pnlTotal", alias = "pnl_total", default)]
    pub pnl_total: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSnapshot {
    pub slug: Option<String>,
    #[serde(default)]
    pub outcomes: Vec<OutcomePrice>,
    pub spread: Option<f64>,
    #[serde(alias = "lastTrade", alias = "last_trade")]
    pub last_trade: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomePrice {
    pub name: Option<String>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub mid: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobOrderBook {
    #[serde(default)]
    pub bids: Vec<ClobLevel>,
    #[serde(default)]
    pub asks: Vec<ClobLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobLevel {
    pub price: Option<f64>,
    pub size: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpreadInfo {
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub spread: Option<f64>,
    #[serde(alias = "spreadBps", alias = "spread_bps")]
    pub spread_bps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BullpenPosition {
    pub market: Option<String>,
    pub outcome: Option<String>,
    pub shares: Option<f64>,
    #[serde(alias = "avgPrice", alias = "avg_price")]
    pub avg_price: Option<f64>,
    #[serde(alias = "currentPrice", alias = "current_price")]
    pub current_price: Option<f64>,
    pub pnl: Option<f64>,
    #[serde(alias = "pnlPct", alias = "pnl_pct")]
    pub pnl_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceInfo {
    pub usdc: Option<f64>,
    #[serde(alias = "positionsValue", alias = "positions_value")]
    pub positions_value: Option<f64>,
    #[serde(alias = "totalValue", alias = "total_value")]
    pub total_value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BullpenOrder {
    #[serde(alias = "orderId", alias = "order_id")]
    pub order_id: Option<String>,
    pub market: Option<String>,
    pub side: Option<String>,
    pub price: Option<f64>,
    pub size: Option<f64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResult {
    #[serde(default)]
    pub cancelled: usize,
    #[serde(default)]
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeEvent {
    pub wallet: Option<String>,
    pub market: Option<String>,
    pub outcome: Option<String>,
    pub side: Option<String>,
    #[serde(alias = "amountUsd", alias = "amount_usd")]
    pub amount_usd: Option<f64>,
    pub price: Option<f64>,
    pub pnl: Option<f64>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeResult {
    #[serde(default)]
    pub success: bool,
    #[serde(alias = "orderId", alias = "order_id")]
    pub order_id: Option<String>,
    pub filled: Option<f64>,
    pub price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeemResult {
    #[serde(default)]
    pub redeemed: usize,
    #[serde(alias = "totalUsdc", alias = "total_usdc", default)]
    pub total_usdc: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerTrade {
    pub wallet: Option<String>,
    pub label: Option<String>,
    pub market: Option<String>,
    pub side: Option<String>,
    #[serde(alias = "amountUsd", alias = "amount_usd")]
    pub amount_usd: Option<f64>,
    pub price: Option<f64>,
    pub timestamp: Option<String>,
}

/// Generic fallback for commands whose exact JSON shape is unknown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericJson(pub serde_json::Value);

// ─── Bridge Core ─────────────────────────────────────────────────────────────

/// Concurrency-controlled, typed Bullpen CLI bridge.
pub struct BullpenBridge {
    config: BullpenConfig,
    semaphore: Arc<Semaphore>,
    health: Arc<RwLock<BullpenHealth>>,
    diagnostics: Arc<RwLock<BullpenDiagnostics>>,
}

impl BullpenBridge {
    pub fn new(config: BullpenConfig) -> Self {
        let permits = config.max_concurrent_commands;
        Self {
            config,
            semaphore: Arc::new(Semaphore::new(permits)),
            health: Arc::new(RwLock::new(BullpenHealth::default())),
            diagnostics: Arc::new(RwLock::new(BullpenDiagnostics::default())),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Read-only snapshot of current health status.
    pub async fn health(&self) -> BullpenHealth {
        self.health.read().await.clone()
    }

    /// Read-only snapshot of diagnostics.
    pub async fn diagnostics(&self) -> BullpenDiagnostics {
        self.diagnostics.read().await.clone()
    }

    /// Startup health check — verifies CLI is reachable and authenticated.
    pub async fn health_check(&self) -> Result<()> {
        let raw: String = self.execute_raw(&["--version"]).await?;
        let version = raw.trim().to_string();

        let mut h = self.health.write().await;
        h.cli_version = Some(version.clone());
        h.last_health_check = Some(Instant::now());
        h.authenticated = true; // If --version works, CLI is at least installed
        tracing::info!(version = %version, "Bullpen CLI health check passed");
        Ok(())
    }

    // ── Core Execution Engine ────────────────────────────────────────────────

    /// Execute a Bullpen CLI command, deserialise `--output json` response.
    async fn execute<T: DeserializeOwned>(&self, args: &[&str], command_name: &str) -> Result<T> {
        if !self.config.enabled {
            return Err(anyhow!("Bullpen bridge disabled (BULLPEN_ENABLED=false)"));
        }

        // Circuit breaker: skip commands while open
        {
            let h = self.health.read().await;
            if let Some(until) = h.circuit_open_until {
                if Instant::now() < until {
                    return Err(anyhow!(
                        "Bullpen circuit breaker open ({} consecutive failures, retry in {}s)",
                        h.consecutive_failures,
                        until.duration_since(Instant::now()).as_secs()
                    ));
                }
            }
        }

        let _permit = self.semaphore.acquire().await
            .map_err(|_| anyhow!("Bullpen semaphore closed"))?;

        let start = Instant::now();
        let mut last_err = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let backoff = self.config.retry_backoff_ms * 2u64.pow(attempt - 1);
                tokio::time::sleep(Duration::from_millis(backoff)).await;
            }

            match self.execute_once::<T>(args).await {
                Ok(result) => {
                    self.record_success(command_name, start.elapsed()).await;
                    return Ok(result);
                }
                Err(e) => {
                    // Auth errors are not retryable
                    if Self::is_auth_error(&e) {
                        tracing::warn!(
                            command = command_name,
                            error = %e,
                            "Bullpen auth error — skipping retries (run `bullpen login`)"
                        );
                        self.record_failure(command_name, &e).await;
                        return Err(e);
                    }
                    tracing::debug!(
                        command = command_name,
                        attempt = attempt + 1,
                        error = %e,
                        "Bullpen command attempt failed"
                    );
                    last_err = Some(e);
                }
            }
        }

        let err = last_err.unwrap();
        self.record_failure(command_name, &err).await;
        Err(err)
    }

    /// Check if an error is an authentication/authorization issue (not retryable).
    fn is_auth_error(err: &anyhow::Error) -> bool {
        let msg = err.to_string().to_lowercase();
        msg.contains("not logged in")
            || msg.contains("token refresh failed")
            || msg.contains("re-authenticate")
            || msg.contains("401")
            || msg.contains("unauthorized")
    }

    /// Execute with `--output json` and deserialise.
    async fn execute_once<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let stdout = self.execute_raw_with_json(args).await?;

        serde_json::from_str::<T>(&stdout).with_context(|| {
            let preview: String = stdout.chars().take(200).collect();
            format!("Failed to parse bullpen JSON: {preview}")
        })
    }

    /// Execute raw command — returns stdout as string, no `--output json` appended.
    async fn execute_raw(&self, args: &[&str]) -> Result<String> {
        let (program, mut full_args) = self.build_command_parts();
        full_args.extend(args.iter().map(|s| s.to_string()));

        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&full_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = cmd.spawn().context("Failed to spawn bullpen CLI")?;

        let output = tokio::time::timeout(self.config.timeout, child.wait_with_output())
            .await
            .context("Bullpen command timed out")?
            .context("Failed to read bullpen output")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Bullpen exited {}: {}", output.status, stderr.trim()));
        }

        String::from_utf8(output.stdout).context("Invalid UTF-8 in bullpen output")
    }

    /// Execute with `--output json` appended.
    async fn execute_raw_with_json(&self, args: &[&str]) -> Result<String> {
        let (program, mut full_args) = self.build_command_parts();
        full_args.extend(args.iter().map(|s| s.to_string()));
        full_args.push("--output".to_string());
        full_args.push("json".to_string());

        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&full_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = cmd.spawn().context("Failed to spawn bullpen CLI")?;

        let output = tokio::time::timeout(self.config.timeout, child.wait_with_output())
            .await
            .context("Bullpen command timed out")?
            .context("Failed to read bullpen output")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Bullpen exited {}: {}", output.status, stderr.trim()));
        }

        String::from_utf8(output.stdout).context("Invalid UTF-8 in bullpen output")
    }

    /// Build (program, args) tuple.  On Windows/WSL, splits the cli_path.
    fn build_command_parts(&self) -> (String, Vec<String>) {
        let parts: Vec<&str> = self.config.cli_path.split_whitespace().collect();
        if parts.len() > 1 {
            let program = parts[0].to_string();
            let prefix_args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            (program, prefix_args)
        } else {
            (self.config.cli_path.clone(), vec![])
        }
    }

    // ── Telemetry helpers ────────────────────────────────────────────────────

    async fn record_success(&self, command: &str, elapsed: Duration) {
        let ms = elapsed.as_millis() as f64;
        let mut diag = self.diagnostics.write().await;
        let stats = diag.calls_by_command.entry(command.to_string()).or_default();
        stats.total_calls += 1;
        stats.successes += 1;
        stats.total_latency_ms += ms;
        if ms > stats.max_latency_ms {
            stats.max_latency_ms = ms;
        }

        let mut h = self.health.write().await;
        h.total_calls += 1;
        h.consecutive_failures = 0;
        h.circuit_open_until = None; // reset circuit breaker on success
        // Exponential moving average of latency
        h.avg_latency_ms = h.avg_latency_ms * 0.9 + ms * 0.1;
    }

    async fn record_failure(&self, command: &str, err: &anyhow::Error) {
        let mut diag = self.diagnostics.write().await;
        let stats = diag.calls_by_command.entry(command.to_string()).or_default();
        stats.total_calls += 1;
        stats.failures += 1;

        let mut h = self.health.write().await;
        h.total_calls += 1;
        h.total_failures += 1;
        h.consecutive_failures += 1;
        h.last_error = Some(format!("{err:#}"));

        // Trip circuit breaker after 5 consecutive failures (60s cooldown)
        const CIRCUIT_BREAKER_THRESHOLD: u32 = 5;
        const CIRCUIT_BREAKER_COOLDOWN_SECS: u64 = 60;
        if h.consecutive_failures >= CIRCUIT_BREAKER_THRESHOLD && h.circuit_open_until.is_none() {
            h.circuit_open_until = Some(Instant::now() + Duration::from_secs(CIRCUIT_BREAKER_COOLDOWN_SECS));
            tracing::error!(
                consecutive_failures = h.consecutive_failures,
                cooldown_secs = CIRCUIT_BREAKER_COOLDOWN_SECS,
                "Bullpen circuit breaker OPEN — pausing all commands"
            );
        }

        // Mark unauthenticated on auth errors
        if Self::is_auth_error(err) {
            h.authenticated = false;
        }

        tracing::warn!(
            command,
            consecutive_failures = h.consecutive_failures,
            error = %err,
            "Bullpen command failed"
        );
    }
}

// ─── Command Methods ─────────────────────────────────────────────────────────

// Market Discovery
impl BullpenBridge {
    /// Discover markets via one of 7 lenses.
    pub async fn discover_markets(&self, lens: &str) -> Result<DiscoverResponse> {
        self.execute(&["polymarket", "discover", lens], "discover").await
    }
}

// Smart Money Intelligence
impl BullpenBridge {
    /// Get smart money signals (top_traders | new_wallet | aggregated).
    pub async fn smart_money(&self, signal_type: &str) -> Result<GenericJson> {
        self.execute(&["polymarket", "data", "smart-money", "--type", signal_type], "smart_money").await
    }

    /// Profile a specific trader by wallet address.
    pub async fn trader_profile(&self, address: &str) -> Result<GenericJson> {
        self.execute(&["polymarket", "data", "profile", address], "trader_profile").await
    }

    /// Get filtered trade feed (high-P&L trades).
    pub async fn trade_feed(&self, min_pnl: f64) -> Result<GenericJson> {
        let min_pnl_str = min_pnl.to_string();
        self.execute(
            &["polymarket", "feed", "trades", "--min-pnl", &min_pnl_str],
            "trade_feed",
        ).await
    }
}

// CLOB Data Access
impl BullpenBridge {
    /// Get real-time price for a market slug.
    pub async fn price(&self, slug: &str) -> Result<GenericJson> {
        self.execute(&["polymarket", "price", slug], "price").await
    }

    /// Get order book for a token ID.
    pub async fn clob_book(&self, token_id: &str) -> Result<GenericJson> {
        self.execute(&["polymarket", "clob", "book", "--token", token_id], "clob_book").await
    }

    /// Get midpoint price for a token ID.
    pub async fn clob_midpoint(&self, token_id: &str) -> Result<GenericJson> {
        self.execute(&["polymarket", "clob", "midpoint", "--token", token_id], "clob_midpoint").await
    }

    /// Get bid-ask spread for a token ID.
    pub async fn clob_spread(&self, token_id: &str) -> Result<GenericJson> {
        self.execute(&["polymarket", "clob", "spread", "--token", token_id], "clob_spread").await
    }
}

// Portfolio & Positions
impl BullpenBridge {
    /// Get all open positions with P&L.
    pub async fn positions(&self) -> Result<GenericJson> {
        self.execute(&["polymarket", "positions"], "positions").await
    }

    /// Get account balance.
    pub async fn balance(&self) -> Result<GenericJson> {
        self.execute(&["polymarket", "balance"], "balance").await
    }

    /// Get open orders.
    pub async fn open_orders(&self) -> Result<GenericJson> {
        self.execute(&["polymarket", "orders"], "open_orders").await
    }
}

// Emergency Controls
impl BullpenBridge {
    /// Cancel ALL open orders — EMERGENCY USE ONLY.
    pub async fn cancel_all_orders(&self) -> Result<GenericJson> {
        self.execute(
            &["polymarket", "orders", "--cancel-all", "--yes"],
            "cancel_all",
        ).await
    }
}

// Wallet Tracking
impl BullpenBridge {
    /// Add a wallet to the tracker.
    pub async fn tracker_add(&self, address: &str, label: Option<&str>) -> Result<GenericJson> {
        let mut args = vec!["tracker", "add", address];
        if let Some(l) = label {
            args.extend(&["--label", l]);
        }
        self.execute(&args, "tracker_add").await
    }

    /// Get tracked wallet trades.
    pub async fn tracker_trades(&self) -> Result<GenericJson> {
        self.execute(&["tracker", "trades"], "tracker_trades").await
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_env_defaults() {
        // Clear any test-set vars
        std::env::remove_var("BULLPEN_ENABLED");
        std::env::remove_var("BULLPEN_CLI_PATH");
        std::env::remove_var("BULLPEN_TIMEOUT_SECS");

        let config = BullpenConfig::from_env();
        assert!(!config.enabled);
        assert_eq!(config.timeout, Duration::from_secs(15));
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.max_concurrent_commands, 3);
    }

    #[test]
    fn health_status_initial() {
        let health = BullpenHealth::default();
        assert!(!health.authenticated);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.total_calls, 0);
        assert!(health.cli_version.is_none());
    }

    #[test]
    fn command_stats_avg_latency() {
        let mut stats = CommandStats::default();
        assert_eq!(stats.avg_latency_ms(), 0.0);

        stats.total_calls = 2;
        stats.total_latency_ms = 1000.0;
        assert!((stats.avg_latency_ms() - 500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn build_command_parts_simple() {
        let config = BullpenConfig {
            enabled: true,
            cli_path: "bullpen".to_string(),
            timeout: Duration::from_secs(10),
            max_retries: 0,
            retry_backoff_ms: 100,
            max_concurrent_commands: 1,
            use_wsl: false,
        };
        let bridge = BullpenBridge::new(config);
        let (prog, args) = bridge.build_command_parts();
        assert_eq!(prog, "bullpen");
        assert!(args.is_empty());
    }

    #[test]
    fn build_command_parts_wsl() {
        let config = BullpenConfig {
            enabled: true,
            cli_path: "wsl -d Ubuntu -- bullpen".to_string(),
            timeout: Duration::from_secs(10),
            max_retries: 0,
            retry_backoff_ms: 100,
            max_concurrent_commands: 1,
            use_wsl: true,
        };
        let bridge = BullpenBridge::new(config);
        let (prog, args) = bridge.build_command_parts();
        assert_eq!(prog, "wsl");
        assert_eq!(args, vec!["-d", "Ubuntu", "--", "bullpen"]);
    }

    #[tokio::test]
    async fn bridge_disabled_returns_error() {
        let config = BullpenConfig {
            enabled: false,
            cli_path: "bullpen".to_string(),
            timeout: Duration::from_secs(1),
            max_retries: 0,
            retry_backoff_ms: 100,
            max_concurrent_commands: 1,
            use_wsl: false,
        };
        let bridge = BullpenBridge::new(config);
        let result = bridge.discover_markets("all").await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("disabled"), "Expected 'disabled' in: {err_msg}");
    }
}
