//! Polymarket CLOB REST API client.
//!
//! All methods reuse a single [`reqwest::Client`] connection pool.  
//! Every call is traced with its HTTP latency via [`tracing`].

use std::time::Instant;

use anyhow::{Context, Result};
use tracing::instrument;

use crate::types::OrderSide;

/// HTTP client for the Polymarket CLOB REST API.
#[allow(dead_code)]
pub struct ClobClient {
    client: reqwest::Client,
    base_url: String,
}

#[allow(dead_code)]
impl ClobClient {
    /// Creates a new client pointing at `base_url`
    /// (e.g. `"https://clob.polymarket.com"`).
    ///
    /// Uses a single shared [`reqwest::Client`] for connection pooling.
    #[allow(dead_code)]
    pub fn new(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .connection_verbose(false)
            .build()
            .expect("failed to build reqwest client — TLS init error?");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    // ─── Order Book ─────────────────────────────────────────────────────────

    /// Fetches the current order-book snapshot for `token_id`.
    ///
    /// `GET /order-book/{token_id}`
    #[instrument(skip(self), fields(token_id, latency_ms))]
    pub async fn get_order_book(&self, token_id: &str) -> Result<serde_json::Value> {
        let url = format!("{}/order-book/{}", self.base_url, token_id);
        self.get_json(&url).await
    }

    // ─── Price Endpoints ────────────────────────────────────────────────────

    /// Fetches the current best price for `token_id` on the given `side`.
    ///
    /// `GET /price?token_id={id}&side={BUY|SELL}`
    ///
    /// Returns the price as the raw decimal string from the API.
    #[instrument(skip(self), fields(token_id, side = %side, latency_ms))]
    pub async fn get_price(&self, token_id: &str, side: OrderSide) -> Result<String> {
        let url = format!(
            "{}/price?token_id={}&side={}",
            self.base_url, token_id, side
        );
        let value = self.get_json(&url).await?;
        let price = value
            .get("price")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .with_context(|| format!("missing `price` field in response for {token_id}"))?;
        Ok(price)
    }

    /// Fetches the mid-point price for `token_id`.
    ///
    /// `GET /midpoint?token_id={id}`
    ///
    /// Returns the mid-point as the raw decimal string from the API.
    #[instrument(skip(self), fields(token_id, latency_ms))]
    pub async fn get_midpoint(&self, token_id: &str) -> Result<String> {
        let url = format!("{}/midpoint?token_id={}", self.base_url, token_id);
        let value = self.get_json(&url).await?;
        let mid = value
            .get("mid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .with_context(|| format!("missing `mid` field in response for {token_id}"))?;
        Ok(mid)
    }

    // ─── Markets ────────────────────────────────────────────────────────────

    /// Fetches the full list of active Polymarket markets.
    ///
    /// `GET /markets`
    #[instrument(skip(self), fields(latency_ms))]
    pub async fn get_markets(&self) -> Result<serde_json::Value> {
        let url = format!("{}/markets", self.base_url);
        self.get_json(&url).await
    }

    // ─── Internal helpers ───────────────────────────────────────────────────

    /// Issues a GET request, records latency, and deserialises the response
    /// as [`serde_json::Value`].
    async fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let t0 = Instant::now();

        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("HTTP GET failed: {url}"))?;

        let latency_ms = t0.elapsed().as_millis();
        let status = response.status();

        tracing::debug!(
            url     = %url,
            status  = %status,
            latency_ms = latency_ms,
            "CLOB HTTP response"
        );

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("CLOB API error {status} for {url}: {body}");
        }

        let value: serde_json::Value = response
            .json()
            .await
            .with_context(|| format!("failed to deserialise JSON from {url}"))?;

        Ok(value)
    }
}
