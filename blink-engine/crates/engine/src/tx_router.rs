//! Private transaction routing via Flashbots / bloXroute / public RPC.
//!
//! [`TxRouter`] sends signed settlement bundles through private relay
//! endpoints to bypass the public P2P gossip network and avoid front-running.
//!
//! # Fallback chain
//!
//! 1. **Flashbots** relay (`eth_sendBundle` with `X-Flashbots-Signature`)
//! 2. **bloXroute** BDN (`blxr_submit_bundle`)
//! 3. **Public RPC** (`eth_sendRawTransaction`) — last resort
//!
//! All submissions are fire-and-forget with a 100 ms timeout.  The router
//! is non-fatal: if every relay is unreachable, the public RPC is used.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use tracing::{debug, info, warn};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Maximum time to spend on a single relay submission.
const RELAY_TIMEOUT: Duration = Duration::from_millis(100);

/// Maximum consecutive failures before a relay is temporarily skipped.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Default Flashbots relay endpoint.
const DEFAULT_FLASHBOTS_URL: &str = "https://relay.flashbots.net";

/// Default bloXroute endpoint.
const DEFAULT_BLOXROUTE_URL: &str = "https://api.blxrbdn.com";

/// Default Polygon public RPC.
const DEFAULT_PUBLIC_RPC: &str = "https://polygon-rpc.com";

// ─── Types ───────────────────────────────────────────────────────────────────

/// Response from a bundle submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleResponse {
    /// Relay-assigned bundle hash (or tx hash for public RPC).
    pub bundle_hash: String,
    /// Whether the relay accepted the bundle.
    pub accepted: bool,
    /// Which relay handled the submission.
    pub relay: String,
    /// Submission latency.
    pub latency_ms: u64,
}

/// Configuration for the transaction router.
#[derive(Debug, Clone)]
pub struct TxRouterConfig {
    /// Flashbots relay URL.
    pub flashbots_url: String,
    /// bloXroute relay URL.
    pub bloxroute_url: String,
    /// Public RPC fallback URL.
    pub public_rpc_url: String,
    /// bloXroute auth header value (if required).
    pub bloxroute_auth: Option<String>,
}

impl Default for TxRouterConfig {
    fn default() -> Self {
        Self {
            flashbots_url: DEFAULT_FLASHBOTS_URL.to_string(),
            bloxroute_url: DEFAULT_BLOXROUTE_URL.to_string(),
            public_rpc_url: DEFAULT_PUBLIC_RPC.to_string(),
            bloxroute_auth: None,
        }
    }
}

impl TxRouterConfig {
    /// Load configuration from environment variables with sensible defaults.
    pub fn from_env() -> Self {
        Self {
            flashbots_url: std::env::var("FLASHBOTS_RELAY_URL")
                .unwrap_or_else(|_| DEFAULT_FLASHBOTS_URL.to_string()),
            bloxroute_url: std::env::var("BLOXROUTE_URL")
                .unwrap_or_else(|_| DEFAULT_BLOXROUTE_URL.to_string()),
            public_rpc_url: std::env::var("POLYGON_RPC_URL")
                .unwrap_or_else(|_| DEFAULT_PUBLIC_RPC.to_string()),
            bloxroute_auth: std::env::var("BLOXROUTE_AUTH_HEADER").ok(),
        }
    }
}

// ─── TxRouter ────────────────────────────────────────────────────────────────

/// Private transaction router with Flashbots → bloXroute → public fallback.
pub struct TxRouter {
    config: TxRouterConfig,
    client: reqwest::Client,
    /// Optional vault handle for signing Flashbots auth headers.
    vault: Option<Arc<tee_vault::VaultHandle>>,
    /// Consecutive failure counters: [flashbots, bloxroute, public].
    failures: [u32; 3],
}

impl TxRouter {
    /// Create a new router with the given config and optional vault for
    /// Flashbots `X-Flashbots-Signature` header generation.
    pub fn new(config: TxRouterConfig, vault: Option<Arc<tee_vault::VaultHandle>>) -> Self {
        info!(
            flashbots = %config.flashbots_url,
            bloxroute = %config.bloxroute_url,
            public = %config.public_rpc_url,
            "TxRouter initialised"
        );
        Self {
            config,
            client: reqwest::Client::builder()
                .timeout(RELAY_TIMEOUT)
                .build()
                .unwrap_or_default(),
            vault,
            failures: [0; 3],
        }
    }

    /// Create from environment variables.
    pub fn from_env(vault: Option<Arc<tee_vault::VaultHandle>>) -> Self {
        Self::new(TxRouterConfig::from_env(), vault)
    }

    /// Send a bundle of signed transactions targeting a specific block.
    ///
    /// Tries Flashbots first, then bloXroute, then public RPC.
    /// Each attempt has a 100 ms timeout. The method is non-blocking:
    /// failures are logged but never panic.
    pub async fn send_bundle(
        &mut self,
        txs: Vec<String>,
        target_block: u64,
    ) -> Result<BundleResponse> {
        let start = Instant::now();

        // 1. Try Flashbots (if not circuit-broken).
        if self.failures[0] < MAX_CONSECUTIVE_FAILURES {
            match self.send_flashbots(&txs, target_block).await {
                Ok(resp) => {
                    self.failures[0] = 0;
                    return Ok(resp);
                }
                Err(e) => {
                    self.failures[0] += 1;
                    warn!(
                        error = %e,
                        consecutive = self.failures[0],
                        "Flashbots submission failed"
                    );
                }
            }
        } else {
            debug!(
                "Flashbots circuit-broken ({} failures) — skipping",
                self.failures[0]
            );
        }

        // 2. Try bloXroute.
        if self.failures[1] < MAX_CONSECUTIVE_FAILURES {
            match self.send_bloxroute(&txs, target_block).await {
                Ok(resp) => {
                    self.failures[1] = 0;
                    return Ok(resp);
                }
                Err(e) => {
                    self.failures[1] += 1;
                    warn!(
                        error = %e,
                        consecutive = self.failures[1],
                        "bloXroute submission failed"
                    );
                }
            }
        } else {
            debug!(
                "bloXroute circuit-broken ({} failures) — skipping",
                self.failures[1]
            );
        }

        // 3. Public RPC fallback (always attempted).
        let result = self.send_public_rpc(&txs).await;
        match &result {
            Ok(_) => self.failures[2] = 0,
            Err(e) => {
                self.failures[2] += 1;
                warn!(
                    error = %e,
                    latency_ms = start.elapsed().as_millis() as u64,
                    "all relays failed — public RPC also failed"
                );
            }
        }
        result
    }

    /// Reset failure counters (e.g. on successful external health check).
    pub fn reset_failures(&mut self) {
        self.failures = [0; 3];
    }

    /// Returns which relay would currently be tried first.
    pub fn active_relay(&self) -> &str {
        if self.failures[0] < MAX_CONSECUTIVE_FAILURES {
            "flashbots"
        } else if self.failures[1] < MAX_CONSECUTIVE_FAILURES {
            "bloxroute"
        } else {
            "public-rpc"
        }
    }

    // ── Private relay implementations ────────────────────────────────────

    async fn send_flashbots(&self, txs: &[String], target_block: u64) -> Result<BundleResponse> {
        let start = Instant::now();

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendBundle",
            "params": [{
                "txs": txs,
                "blockNumber": format!("0x{:x}", target_block),
            }]
        });

        let body_str = serde_json::to_string(&body)?;

        // Build X-Flashbots-Signature header.
        let mut request = self
            .client
            .post(&self.config.flashbots_url)
            .header("Content-Type", "application/json");

        if let Some(ref vault) = self.vault {
            let payload_hash = keccak256(body_str.as_bytes());
            // Sign the payload hash with the vault key.
            match tee_vault::KeyVault::sign_digest(vault.as_ref(), &payload_hash) {
                Ok(sig65) => {
                    let sig_hex: String = sig65.iter().map(|b| format!("{b:02x}")).collect();
                    let header_val = format!("{}:0x{}", vault.signer_address(), sig_hex);
                    request = request.header("X-Flashbots-Signature", header_val);
                }
                Err(e) => {
                    warn!(error = %e, "Failed to sign Flashbots header — sending unsigned");
                }
            }
        }

        let resp = request
            .body(body_str)
            .send()
            .await
            .context("Flashbots relay request failed")?;

        let json: serde_json::Value = resp.json().await.context("invalid Flashbots response")?;
        let hash = json["result"]["bundleHash"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(BundleResponse {
            bundle_hash: hash,
            accepted: true,
            relay: "flashbots".to_string(),
            latency_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn send_bloxroute(&self, txs: &[String], target_block: u64) -> Result<BundleResponse> {
        let start = Instant::now();

        let body = serde_json::json!({
            "method": "blxr_submit_bundle",
            "params": {
                "transaction": txs,
                "block_number": format!("0x{:x}", target_block),
            }
        });

        let mut request = self.client.post(&self.config.bloxroute_url).json(&body);

        if let Some(ref auth) = self.config.bloxroute_auth {
            request = request.header("Authorization", auth);
        }

        let resp = request
            .send()
            .await
            .context("bloXroute submission failed")?;

        let json: serde_json::Value = resp.json().await.context("invalid bloXroute response")?;
        let hash = json["result"]["bundleHash"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(BundleResponse {
            bundle_hash: hash,
            accepted: true,
            relay: "bloxroute".to_string(),
            latency_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn send_public_rpc(&self, txs: &[String]) -> Result<BundleResponse> {
        let start = Instant::now();
        let mut last_hash = String::new();

        for (i, tx) in txs.iter().enumerate() {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_sendRawTransaction",
                "params": [tx]
            });

            let resp = self
                .client
                .post(&self.config.public_rpc_url)
                .json(&body)
                .send()
                .await
                .with_context(|| format!("public RPC failed for tx {i}"))?;

            let json: serde_json::Value = resp.json().await?;
            if let Some(hash) = json["result"].as_str() {
                last_hash = hash.to_string();
            }
        }

        Ok(BundleResponse {
            bundle_hash: if last_hash.is_empty() {
                "public-rpc-submitted".to_string()
            } else {
                last_hash
            },
            accepted: true,
            relay: "public-rpc".to_string(),
            latency_ms: start.elapsed().as_millis() as u64,
        })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    hasher.finalize().into()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TxRouterConfig {
        TxRouterConfig::default()
    }

    #[test]
    fn default_config_has_correct_urls() {
        let cfg = TxRouterConfig::default();
        assert_eq!(cfg.flashbots_url, DEFAULT_FLASHBOTS_URL);
        assert_eq!(cfg.bloxroute_url, DEFAULT_BLOXROUTE_URL);
        assert_eq!(cfg.public_rpc_url, DEFAULT_PUBLIC_RPC);
        assert!(cfg.bloxroute_auth.is_none());
    }

    #[test]
    fn config_from_env_uses_defaults_when_unset() {
        // Clear env vars to test defaults.
        std::env::remove_var("FLASHBOTS_RELAY_URL");
        std::env::remove_var("BLOXROUTE_URL");
        std::env::remove_var("POLYGON_RPC_URL");
        std::env::remove_var("BLOXROUTE_AUTH_HEADER");

        let cfg = TxRouterConfig::from_env();
        assert_eq!(cfg.flashbots_url, DEFAULT_FLASHBOTS_URL);
        assert_eq!(cfg.bloxroute_url, DEFAULT_BLOXROUTE_URL);
        assert_eq!(cfg.public_rpc_url, DEFAULT_PUBLIC_RPC);
    }

    #[test]
    fn active_relay_starts_with_flashbots() {
        let router = TxRouter::new(test_config(), None);
        assert_eq!(router.active_relay(), "flashbots");
    }

    #[test]
    fn active_relay_falls_back_after_failures() {
        let mut router = TxRouter::new(test_config(), None);

        // Simulate Flashbots circuit break.
        router.failures[0] = MAX_CONSECUTIVE_FAILURES;
        assert_eq!(router.active_relay(), "bloxroute");

        // Simulate bloXroute circuit break too.
        router.failures[1] = MAX_CONSECUTIVE_FAILURES;
        assert_eq!(router.active_relay(), "public-rpc");
    }

    #[test]
    fn reset_failures_clears_all() {
        let mut router = TxRouter::new(test_config(), None);
        router.failures = [5, 5, 5];
        router.reset_failures();
        assert_eq!(router.failures, [0, 0, 0]);
        assert_eq!(router.active_relay(), "flashbots");
    }

    #[test]
    fn keccak256_produces_32_bytes() {
        let hash = keccak256(b"test data");
        assert_eq!(hash.len(), 32);
        // Same input → same output.
        assert_eq!(hash, keccak256(b"test data"));
        // Different input → different output.
        assert_ne!(hash, keccak256(b"other data"));
    }

    #[test]
    fn bundle_response_serializes() {
        let resp = BundleResponse {
            bundle_hash: "0xabc".to_string(),
            accepted: true,
            relay: "flashbots".to_string(),
            latency_ms: 42,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["relay"], "flashbots");
        assert_eq!(json["latency_ms"], 42);
        assert!(json["accepted"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn send_bundle_falls_through_to_public_rpc() {
        // Use closed localhost ports so behavior is deterministic and does not
        // depend on external network access in CI.
        let cfg = TxRouterConfig {
            flashbots_url: "http://127.0.0.1:9".to_string(),
            bloxroute_url: "http://127.0.0.1:9".to_string(),
            public_rpc_url: "http://127.0.0.1:9".to_string(),
            bloxroute_auth: None,
        };
        let mut router = TxRouter::new(cfg, None);
        let result = router
            .send_bundle(vec!["0xdead".to_string()], 99_999_999)
            .await;

        // All relays unreachable in test — expect error.
        assert!(result.is_err());
        // But failures should have been recorded.
        assert!(router.failures[0] > 0 || router.failures[1] > 0 || router.failures[2] > 0);
    }
}
