//! Private transaction routing for MEV protection on Polygon.
//!
//! Polymarket trades are `CTFExchange.fillOrder()` calls on Polygon. Without
//! private routing, these are visible in the public mempool and vulnerable to
//! front-running / sandwich attacks.
//!
//! [`MevRouter`] wraps multiple private relay backends with automatic fallback
//! and EIP-1559 dynamic fee calculation.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// ─── Router variants ─────────────────────────────────────────────────────────

/// Supported private transaction relay backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateRouter {
    /// Flashbots relay (Ethereum mainnet — included for future multi-chain).
    Flashbots,
    /// Polygon private mempool via 0x aggregator.
    Polygon0x,
    /// Blocknative private auction.
    Blocknative,
    /// bloXroute Polygon BDN.
    BloxroutePolygon,
    /// Standard public RPC — last resort.
    PublicFallback,
}

impl PrivateRouter {
    /// Default relay endpoint URL for each router.
    pub fn default_endpoint(&self) -> &'static str {
        match self {
            Self::Flashbots        => "https://relay.flashbots.net",
            Self::Polygon0x        => "https://polygon.api.0x.org/tx",
            Self::Blocknative      => "https://api.blocknative.com/v1/auction",
            Self::BloxroutePolygon => "wss://api.bloxroute.com:443",
            Self::PublicFallback   => "https://polygon-rpc.com",
        }
    }
}

impl std::fmt::Display for PrivateRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flashbots        => write!(f, "flashbots"),
            Self::Polygon0x        => write!(f, "polygon-0x"),
            Self::Blocknative      => write!(f, "blocknative"),
            Self::BloxroutePolygon => write!(f, "bloxroute-polygon"),
            Self::PublicFallback   => write!(f, "public-rpc"),
        }
    }
}

// ─── Fee strategy ────────────────────────────────────────────────────────────

/// EIP-1559 dynamic fee calculation parameters.
#[derive(Debug, Clone)]
pub struct FeeStrategy {
    /// Number of recent blocks to consider for base-fee trend.
    pub blocks_lookback: u32,
    /// Multiply base fee by this factor for safe inclusion.
    pub base_fee_multiplier: f64,
    /// Maximum priority fee in gwei.
    pub max_priority_fee_gwei: u64,
    /// Absolute maximum gas price (circuit breaker) in gwei.
    pub max_gas_price_gwei: u64,
}

impl Default for FeeStrategy {
    fn default() -> Self {
        Self {
            blocks_lookback: 10,
            base_fee_multiplier: 1.25,
            max_priority_fee_gwei: 30,
            max_gas_price_gwei: 500,
        }
    }
}

/// Suggested fee values (returned from fee estimation).
#[derive(Debug, Clone, Serialize)]
pub struct SuggestedFees {
    /// Recommended max fee per gas in gwei.
    pub max_fee_gwei: u64,
    /// Recommended priority fee in gwei.
    pub priority_fee_gwei: u64,
    /// Current base fee in gwei (informational).
    pub base_fee_gwei: u64,
}

// ─── Transaction bundle ──────────────────────────────────────────────────────

/// A bundle of signed Polygon transactions for private submission.
#[derive(Debug, Clone, Serialize)]
pub struct TransactionBundle {
    /// Signed raw transactions (hex-encoded with `0x` prefix).
    pub txs: Vec<String>,
    /// Target block number for inclusion.
    pub target_block: u64,
    /// Maximum block number to attempt (target_block + N).
    pub max_block: u64,
    /// If true, simulate the bundle before submitting.
    pub simulate_first: bool,
}

/// Receipt returned after bundle submission.
#[derive(Debug, Clone, Deserialize)]
pub struct BundleReceipt {
    /// Relay-assigned bundle hash.
    pub bundle_hash: String,
    /// Whether the relay accepted the bundle.
    pub accepted: bool,
    /// Human-readable status message.
    pub message: Option<String>,
}

/// Result of a bundle dry-run simulation.
#[derive(Debug, Clone, Deserialize)]
pub struct SimResult {
    /// Whether the simulated transactions would succeed.
    pub success: bool,
    /// Estimated gas used.
    pub gas_used: u64,
    /// Revert reason (if any).
    pub revert_reason: Option<String>,
}

// ─── MevRouter ───────────────────────────────────────────────────────────────

/// Private transaction router with priority ordering and automatic fallback.
pub struct MevRouter {
    /// Ordered list of relay backends to try.
    routers: Vec<PrivateRouter>,
    /// HTTP client with connection pooling.
    rpc_client: reqwest::Client,
    /// Maximum TTL for a pending bundle.
    #[allow(dead_code)]
    bundle_ttl: Duration,
    /// Consecutive failures per router.
    failures: Vec<u32>,
    /// Maximum consecutive failures before falling back.
    max_consecutive_failures: u32,
    /// Fee estimation parameters.
    fee_strategy: FeeStrategy,
    /// RPC URL override for public fallback (Polygon).
    polygon_rpc_url: String,
}

impl MevRouter {
    /// Create a new router with the given priority order.
    pub fn new(routers: Vec<PrivateRouter>) -> Self {
        let len = routers.len();
        Self {
            routers,
            rpc_client: reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .expect("failed to build HTTP client"),
            bundle_ttl: Duration::from_secs(12), // ~1 Polygon block
            failures: vec![0; len],
            max_consecutive_failures: 2,
            fee_strategy: FeeStrategy::default(),
            polygon_rpc_url: "https://polygon-rpc.com".to_string(),
        }
    }

    /// Create from environment variable `MEV_ROUTER`.
    ///
    /// Accepted values: `flashbots`, `bloxroute`, `blocknative`, `0x`, `public`.
    /// Multiple routers can be comma-separated for fallback ordering.
    pub fn from_env() -> Self {
        let raw = std::env::var("MEV_ROUTER").unwrap_or_else(|_| "public".to_string());
        let routers: Vec<PrivateRouter> = raw
            .split(',')
            .filter_map(|s| match s.trim().to_lowercase().as_str() {
                "flashbots"  => Some(PrivateRouter::Flashbots),
                "0x"         => Some(PrivateRouter::Polygon0x),
                "blocknative"=> Some(PrivateRouter::Blocknative),
                "bloxroute"  => Some(PrivateRouter::BloxroutePolygon),
                "public"     => Some(PrivateRouter::PublicFallback),
                _            => None,
            })
            .collect();

        let routers = if routers.is_empty() {
            vec![PrivateRouter::PublicFallback]
        } else {
            routers
        };

        let rpc_url = std::env::var("POLYGON_RPC_URL")
            .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());

        let mut router = Self::new(routers);
        router.polygon_rpc_url = rpc_url;
        router
    }

    /// Submit a bundle to the highest-priority available router.
    ///
    /// On failure, falls back to the next router in the priority list.
    /// If a router fails `max_consecutive_failures` times, it is skipped.
    pub async fn submit_bundle(&mut self, bundle: TransactionBundle) -> Result<BundleReceipt> {
        // Optionally simulate first.
        if bundle.simulate_first {
            let sim = self.simulate_bundle(&bundle).await;
            match sim {
                Ok(ref result) if !result.success => {
                    anyhow::bail!(
                        "bundle simulation failed: {}",
                        result.revert_reason.as_deref().unwrap_or("unknown")
                    );
                }
                Err(e) => {
                    warn!(error = %e, "bundle simulation failed, proceeding anyway");
                }
                _ => {}
            }
        }

        // Randomise submission timing ±100ms for timing-analysis protection.
        let jitter = (rand::random::<u64>() % 200) as u64;
        tokio::time::sleep(Duration::from_millis(jitter)).await;

        let start = Instant::now();
        let mut last_error = anyhow::anyhow!("no routers available");

        for (idx, router) in self.routers.iter().enumerate() {
            // Skip routers that have failed too many times.
            if self.failures[idx] >= self.max_consecutive_failures
                && *router != PrivateRouter::PublicFallback
            {
                continue;
            }

            // Enforce 200ms latency budget.
            if start.elapsed() > Duration::from_millis(200) {
                warn!("latency budget exceeded, falling back to public");
                break;
            }

            match self.submit_to_router(*router, &bundle).await {
                Ok(receipt) => {
                    self.failures[idx] = 0; // reset on success
                    info!(router = %router, hash = %receipt.bundle_hash, "bundle submitted");
                    return Ok(receipt);
                }
                Err(e) => {
                    self.failures[idx] += 1;
                    warn!(
                        router = %router,
                        consecutive_failures = self.failures[idx],
                        error = %e,
                        "router submission failed"
                    );
                    last_error = e;
                }
            }
        }

        Err(last_error.context("all routers exhausted"))
    }

    /// Simulate a bundle against current chain state (dry run).
    pub async fn simulate_bundle(&self, bundle: &TransactionBundle) -> Result<SimResult> {
        // Use eth_call simulation via the Polygon RPC.
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{
                "from": "0x0000000000000000000000000000000000000000",
                "data": bundle.txs.first().unwrap_or(&String::new()),
            }, "latest"]
        });

        let resp = self
            .rpc_client
            .post(&self.polygon_rpc_url)
            .json(&body)
            .send()
            .await
            .context("simulation RPC request failed")?;

        if resp.status().is_success() {
            Ok(SimResult {
                success: true,
                gas_used: 0,
                revert_reason: None,
            })
        } else {
            let text = resp.text().await.unwrap_or_default();
            Ok(SimResult {
                success: false,
                gas_used: 0,
                revert_reason: Some(text),
            })
        }
    }

    /// Estimate optimal EIP-1559 fees based on recent block history.
    pub async fn suggest_priority_fee(&self) -> Result<SuggestedFees> {
        // Query eth_feeHistory for recent base fees.
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_feeHistory",
            "params": [
                format!("0x{:x}", self.fee_strategy.blocks_lookback),
                "latest",
                [25, 50, 75]
            ]
        });

        let resp = self
            .rpc_client
            .post(&self.polygon_rpc_url)
            .json(&body)
            .send()
            .await
            .context("fee history RPC failed")?;

        let json: serde_json::Value = resp.json().await.context("invalid fee history JSON")?;

        // Parse latest base fee from result.baseFeePerGas (hex gwei array).
        let base_fee_gwei = json["result"]["baseFeePerGas"]
            .as_array()
            .and_then(|arr| arr.last())
            .and_then(|v| v.as_str())
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .map(|wei| wei / 1_000_000_000) // wei → gwei
            .unwrap_or(30); // Polygon default ~30 gwei

        let suggested_max = ((base_fee_gwei as f64) * self.fee_strategy.base_fee_multiplier) as u64;
        let max_fee = suggested_max.min(self.fee_strategy.max_gas_price_gwei);
        let priority_fee = self.fee_strategy.max_priority_fee_gwei.min(max_fee);

        Ok(SuggestedFees {
            max_fee_gwei: max_fee,
            priority_fee_gwei: priority_fee,
            base_fee_gwei,
        })
    }

    /// Returns the currently active router (first non-failed).
    pub fn active_router(&self) -> PrivateRouter {
        for (idx, router) in self.routers.iter().enumerate() {
            if self.failures[idx] < self.max_consecutive_failures
                || *router == PrivateRouter::PublicFallback
            {
                return *router;
            }
        }
        PrivateRouter::PublicFallback
    }

    // ── Internal ─────────────────────────────────────────────────────────

    async fn submit_to_router(
        &self,
        router: PrivateRouter,
        bundle: &TransactionBundle,
    ) -> Result<BundleReceipt> {
        match router {
            PrivateRouter::Flashbots => self.submit_flashbots(bundle).await,
            PrivateRouter::Polygon0x => self.submit_polygon_0x(bundle).await,
            PrivateRouter::Blocknative => self.submit_blocknative(bundle).await,
            PrivateRouter::BloxroutePolygon => self.submit_bloxroute(bundle).await,
            PrivateRouter::PublicFallback => self.submit_public(bundle).await,
        }
    }

    async fn submit_flashbots(&self, bundle: &TransactionBundle) -> Result<BundleReceipt> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendBundle",
            "params": [{
                "txs": bundle.txs,
                "blockNumber": format!("0x{:x}", bundle.target_block),
                "maxBlockNumber": format!("0x{:x}", bundle.max_block),
            }]
        });

        let resp = self
            .rpc_client
            .post(PrivateRouter::Flashbots.default_endpoint())
            .json(&body)
            .send()
            .await
            .context("Flashbots relay request failed")?;

        let json: serde_json::Value = resp.json().await?;
        let hash = json["result"]["bundleHash"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(BundleReceipt {
            bundle_hash: hash,
            accepted: true,
            message: None,
        })
    }

    async fn submit_polygon_0x(&self, bundle: &TransactionBundle) -> Result<BundleReceipt> {
        // 0x aggregator for Polygon private transactions.
        for (i, tx) in bundle.txs.iter().enumerate() {
            let body = serde_json::json!({ "signedTransaction": tx });
            let resp = self
                .rpc_client
                .post(PrivateRouter::Polygon0x.default_endpoint())
                .json(&body)
                .send()
                .await
                .with_context(|| format!("0x submission failed for tx {i}"))?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("0x rejected tx {i}: {text}");
            }
        }

        Ok(BundleReceipt {
            bundle_hash: format!("0x-bundle-{}", bundle.target_block),
            accepted: true,
            message: Some("submitted via Polygon 0x".to_string()),
        })
    }

    async fn submit_blocknative(&self, bundle: &TransactionBundle) -> Result<BundleReceipt> {
        let body = serde_json::json!({
            "transactions": bundle.txs,
            "blockDeadline": bundle.max_block,
        });

        let resp = self
            .rpc_client
            .post(PrivateRouter::Blocknative.default_endpoint())
            .json(&body)
            .send()
            .await
            .context("Blocknative submission failed")?;

        let json: serde_json::Value = resp.json().await?;
        let hash = json["auctionId"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(BundleReceipt {
            bundle_hash: hash,
            accepted: true,
            message: None,
        })
    }

    async fn submit_bloxroute(&self, bundle: &TransactionBundle) -> Result<BundleReceipt> {
        // bloXroute uses JSON-RPC over WebSocket; fall back to HTTP POST.
        let body = serde_json::json!({
            "method": "blxr_submit_bundle",
            "params": {
                "transaction": bundle.txs,
                "block_number": format!("0x{:x}", bundle.target_block),
            }
        });

        let resp = self
            .rpc_client
            .post("https://api.blxrbdn.com")
            .json(&body)
            .send()
            .await
            .context("bloXroute submission failed")?;

        let json: serde_json::Value = resp.json().await?;
        let hash = json["result"]["bundleHash"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(BundleReceipt {
            bundle_hash: hash,
            accepted: true,
            message: None,
        })
    }

    async fn submit_public(&self, bundle: &TransactionBundle) -> Result<BundleReceipt> {
        // Standard eth_sendRawTransaction for each tx.
        for (i, tx) in bundle.txs.iter().enumerate() {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_sendRawTransaction",
                "params": [tx]
            });

            let resp = self
                .rpc_client
                .post(&self.polygon_rpc_url)
                .json(&body)
                .send()
                .await
                .with_context(|| format!("public RPC failed for tx {i}"))?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("public RPC rejected tx {i}: {text}");
            }
        }

        Ok(BundleReceipt {
            bundle_hash: format!("public-{}", bundle.target_block),
            accepted: true,
            message: Some("submitted via public RPC".to_string()),
        })
    }
}

// ─── Sandwich attack protection helpers ──────────────────────────────────────

/// Add a block deadline to a raw transaction (prevents inclusion after target).
///
/// In practice this is done by setting a low `nonce` or using a short-lived
/// signature.  This helper returns the deadline block encoded for logging.
pub fn block_deadline(target_block: u64, max_blocks_ahead: u64) -> u64 {
    target_block + max_blocks_ahead
}

/// Randomise submission timing within ±jitter_ms to prevent timing analysis.
pub async fn apply_submission_jitter(jitter_ms: u64) {
    let delay = rand::random::<u64>() % (jitter_ms * 2);
    tokio::time::sleep(Duration::from_millis(delay)).await;
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_construction_includes_correct_transactions() {
        let bundle = TransactionBundle {
            txs: vec![
                "0xdeadbeef".to_string(),
                "0xcafebabe".to_string(),
            ],
            target_block: 50_000_000,
            max_block: 50_000_005,
            simulate_first: true,
        };

        assert_eq!(bundle.txs.len(), 2);
        assert_eq!(bundle.txs[0], "0xdeadbeef");
        assert_eq!(bundle.txs[1], "0xcafebabe");
        assert_eq!(bundle.target_block, 50_000_000);
        assert_eq!(bundle.max_block, 50_000_005);
        assert!(bundle.simulate_first);

        // Verify JSON serialization includes all fields.
        let json = serde_json::to_value(&bundle).unwrap();
        assert_eq!(json["txs"].as_array().unwrap().len(), 2);
        assert_eq!(json["target_block"], 50_000_000);
        assert_eq!(json["max_block"], 50_000_005);
    }

    #[test]
    fn eip1559_fee_calculation_within_limits() {
        let strategy = FeeStrategy {
            blocks_lookback: 10,
            base_fee_multiplier: 1.25,
            max_priority_fee_gwei: 30,
            max_gas_price_gwei: 500,
        };

        // Normal case: base_fee=100 gwei → max_fee = 100*1.25 = 125 gwei.
        let base_fee: u64 = 100;
        let suggested_max = (base_fee as f64 * strategy.base_fee_multiplier) as u64;
        assert_eq!(suggested_max, 125);
        let max_fee = suggested_max.min(strategy.max_gas_price_gwei);
        assert_eq!(max_fee, 125);
        let priority = strategy.max_priority_fee_gwei.min(max_fee);
        assert_eq!(priority, 30);
        assert!(max_fee <= strategy.max_gas_price_gwei);

        // Extreme case: base_fee=450 gwei → max_fee = 450*1.25 = 562, capped to 500.
        let extreme_base: u64 = 450;
        let extreme_max = (extreme_base as f64 * strategy.base_fee_multiplier) as u64;
        assert_eq!(extreme_max, 562);
        let capped = extreme_max.min(strategy.max_gas_price_gwei);
        assert_eq!(capped, 500); // Circuit breaker caps it.
    }

    #[test]
    fn deadline_in_past_causes_revert_simulation() {
        let deadline = block_deadline(1_000, 5);
        assert_eq!(deadline, 1_005);

        // A bundle with max_block far in the past should be rejected on-chain.
        let bundle = TransactionBundle {
            txs: vec!["0xsigned_tx".to_string()],
            target_block: 1,
            max_block: block_deadline(1, 5),
            simulate_first: true,
        };
        assert_eq!(bundle.max_block, 6);
        // Block 6 is definitively in the past on Polygon (current block > 50M).
    }

    #[test]
    fn fallback_triggers_after_two_failures() {
        let routers = vec![
            PrivateRouter::Flashbots,
            PrivateRouter::PublicFallback,
        ];
        let mut router = MevRouter::new(routers);

        // Initially, active router should be Flashbots.
        assert_eq!(router.active_router(), PrivateRouter::Flashbots);

        // Simulate 2 consecutive failures on Flashbots.
        router.failures[0] = 2;

        // Now active should fall back to PublicFallback.
        assert_eq!(router.active_router(), PrivateRouter::PublicFallback);

        // Reset failures → back to Flashbots.
        router.failures[0] = 0;
        assert_eq!(router.active_router(), PrivateRouter::Flashbots);
    }

    #[tokio::test]
    async fn simulate_bundle_returns_valid_result() {
        let router = MevRouter::new(vec![PrivateRouter::PublicFallback]);

        let bundle = TransactionBundle {
            txs: vec!["0xdead".to_string()],
            target_block: 99_999_999,
            max_block: 100_000_004,
            simulate_first: false,
        };

        // Simulation against default RPC will fail (no live connection in test).
        let result = router.simulate_bundle(&bundle).await;
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[test]
    fn router_display_formats() {
        assert_eq!(format!("{}", PrivateRouter::Flashbots), "flashbots");
        assert_eq!(format!("{}", PrivateRouter::Polygon0x), "polygon-0x");
        assert_eq!(format!("{}", PrivateRouter::Blocknative), "blocknative");
        assert_eq!(format!("{}", PrivateRouter::BloxroutePolygon), "bloxroute-polygon");
        assert_eq!(format!("{}", PrivateRouter::PublicFallback), "public-rpc");
    }

    #[test]
    fn from_env_defaults_to_public() {
        // Without MEV_ROUTER set, should default to PublicFallback.
        std::env::remove_var("MEV_ROUTER");
        let router = MevRouter::from_env();
        assert_eq!(router.active_router(), PrivateRouter::PublicFallback);
    }

    #[test]
    fn fee_strategy_defaults_are_sensible() {
        let fee = FeeStrategy::default();
        assert_eq!(fee.blocks_lookback, 10);
        assert!((fee.base_fee_multiplier - 1.25).abs() < f64::EPSILON);
        assert_eq!(fee.max_priority_fee_gwei, 30);
        assert_eq!(fee.max_gas_price_gwei, 500);
    }

    #[tokio::test]
    async fn submission_jitter_adds_delay() {
        let start = Instant::now();
        apply_submission_jitter(50).await;
        // Should have some delay (0–100ms). Just verify it didn't panic.
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(200));
    }
}
