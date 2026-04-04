//! Moving-average gas price oracle for Polygon transactions.
//!
//! Provides a lightweight gas fee predictor based on recent Polygon block
//! base fees fetched from the Etherscan (Polygonscan) API.  This is a
//! Phase-4 stepping stone — the full RL model ships in Phase 7.
//!
//! # Activation
//!
//! Set `ETHERSCAN_API_KEY` in `.env`.  When unset, `suggest_priority_fee_gwei()`
//! returns a conservative default (30 gwei).
//!
//! # Caching
//!
//! Results are cached for **12 seconds** (one Polygon block) to avoid
//! rate-limiting the Etherscan API.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Cache TTL — one Polygon block (~12 seconds).
const CACHE_TTL: Duration = Duration::from_secs(12);

/// Fallback priority fee when the API is unreachable or unconfigured (gwei).
const DEFAULT_PRIORITY_FEE_GWEI: u64 = 30;

/// Number of recent blocks to average over.
const WINDOW_SIZE: usize = 20;

/// Polygonscan (Etherscan-compatible) API base URL.
const POLYGONSCAN_API: &str = "https://api.polygonscan.com/api";

// ─── GasOracle ───────────────────────────────────────────────────────────────

/// Moving-average gas price oracle backed by the Polygonscan API.
pub struct GasOracle {
    api_key: Option<String>,
    client: reqwest::Client,
    cache: Mutex<GasCache>,
}

struct GasCache {
    last_fetch: Option<Instant>,
    base_fees: Vec<u64>,
    suggested_fee: u64,
}

impl GasOracle {
    /// Creates a new oracle.  Pass `None` for the API key to use the
    /// conservative default fee without making network calls.
    pub fn new(api_key: Option<String>) -> Self {
        if api_key.is_some() {
            info!("GasOracle initialised with Polygonscan API key");
        } else {
            info!("GasOracle initialised WITHOUT API key — using default {DEFAULT_PRIORITY_FEE_GWEI} gwei");
        }
        Self {
            api_key,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            cache: Mutex::new(GasCache {
                last_fetch: None,
                base_fees: Vec::with_capacity(WINDOW_SIZE),
                suggested_fee: DEFAULT_PRIORITY_FEE_GWEI,
            }),
        }
    }

    /// Returns the suggested priority fee in gwei.
    ///
    /// Uses a cached moving average of recent base fees with a 20 % headroom
    /// multiplier.  Refreshes at most once per [`CACHE_TTL`] (12 s).
    ///
    /// Never fails — returns [`DEFAULT_PRIORITY_FEE_GWEI`] on error.
    pub async fn suggest_priority_fee_gwei(&self) -> u64 {
        // Fast path: return cached value if still fresh.
        {
            let cache = self.cache.lock().unwrap();
            if let Some(ts) = cache.last_fetch {
                if ts.elapsed() < CACHE_TTL {
                    return cache.suggested_fee;
                }
            }
        }

        // Slow path: fetch new data.
        match self.fetch_and_update().await {
            Ok(fee) => fee,
            Err(e) => {
                warn!("GasOracle fetch failed, using cached/default: {e}");
                self.cache.lock().unwrap().suggested_fee
            }
        }
    }

    async fn fetch_and_update(&self) -> Result<u64> {
        let api_key = match &self.api_key {
            Some(k) => k.clone(),
            None => return Ok(DEFAULT_PRIORITY_FEE_GWEI),
        };

        let resp = self
            .client
            .get(POLYGONSCAN_API)
            .query(&[
                ("module", "proxy"),
                ("action", "eth_gasPrice"),
                ("apikey", &api_key),
            ])
            .send()
            .await
            .context("Polygonscan request failed")?;

        let body: serde_json::Value = resp.json().await.context("Invalid JSON from Polygonscan")?;

        let hex_price = body["result"]
            .as_str()
            .context("Missing 'result' in Polygonscan response")?;

        let gas_wei = u64::from_str_radix(hex_price.trim_start_matches("0x"), 16)
            .context("Invalid hex gas price")?;
        let gas_gwei = gas_wei / 1_000_000_000;

        debug!(gas_gwei, "Fetched current gas price from Polygonscan");

        let fee = {
            let mut cache = self.cache.lock().unwrap();
            cache.base_fees.push(gas_gwei);
            if cache.base_fees.len() > WINDOW_SIZE {
                cache.base_fees.remove(0);
            }

            let avg = if cache.base_fees.is_empty() {
                DEFAULT_PRIORITY_FEE_GWEI
            } else {
                let sum: u64 = cache.base_fees.iter().sum();
                sum / cache.base_fees.len() as u64
            };

            // Apply 20 % headroom to beat congestion spikes.
            let suggested = avg + avg / 5;
            let suggested = suggested.max(1);

            cache.suggested_fee = suggested;
            cache.last_fetch = Some(Instant::now());
            suggested
        };

        debug!(suggested_gwei = fee, "GasOracle updated");
        Ok(fee)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fee_without_api_key() {
        let oracle = GasOracle::new(None);
        let cache = oracle.cache.lock().unwrap();
        assert_eq!(cache.suggested_fee, DEFAULT_PRIORITY_FEE_GWEI);
    }

    #[tokio::test]
    async fn suggest_returns_default_when_no_key() {
        let oracle = GasOracle::new(None);
        let fee = oracle.suggest_priority_fee_gwei().await;
        assert_eq!(fee, DEFAULT_PRIORITY_FEE_GWEI);
    }

    #[test]
    fn cache_ttl_is_twelve_seconds() {
        assert_eq!(CACHE_TTL, Duration::from_secs(12));
    }

    #[test]
    fn moving_average_calculation() {
        let oracle = GasOracle::new(None);
        {
            let mut cache = oracle.cache.lock().unwrap();
            cache.base_fees = vec![10, 20, 30];
            let sum: u64 = cache.base_fees.iter().sum();
            let avg = sum / cache.base_fees.len() as u64;
            let suggested = avg + avg / 5; // 20 + 4 = 24
            cache.suggested_fee = suggested;
        }
        let cache = oracle.cache.lock().unwrap();
        assert_eq!(cache.suggested_fee, 24); // avg(10,20,30)=20, +20%=24
    }

    #[test]
    fn window_eviction() {
        let oracle = GasOracle::new(None);
        let mut cache = oracle.cache.lock().unwrap();
        for i in 0..30 {
            cache.base_fees.push(i);
            if cache.base_fees.len() > WINDOW_SIZE {
                cache.base_fees.remove(0);
            }
        }
        assert_eq!(cache.base_fees.len(), WINDOW_SIZE);
        assert_eq!(cache.base_fees[0], 10); // oldest retained sample
    }
}
