//! Shared HTTP client context passed to every command.

use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;

use crate::OutputFormat;

/// Shared state injected into every command handler.
pub struct CliContext {
    pub engine_url: String,
    pub clob_url: String,
    pub gamma_url: String,
    pub client: Client,
    pub output: OutputFormat,
}

impl CliContext {
    pub fn new(engine_host: String, output: OutputFormat) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");

        Self {
            engine_url: engine_host,
            clob_url: "https://clob.polymarket.com".to_string(),
            gamma_url: "https://gamma-api.polymarket.com".to_string(),
            client,
            output,
        }
    }

    /// GET from the Blink engine API.
    pub async fn engine_get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.engine_url, path);
        self.get_json(&url).await
    }

    /// POST to the Blink engine API.
    pub async fn engine_post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.engine_url, path);
        let resp: reqwest::Response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        resp.json::<serde_json::Value>()
            .await
            .with_context(|| "failed to parse engine response")
    }

    /// GET from the Polymarket CLOB API.
    pub async fn clob_get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.clob_url, path);
        self.get_json(&url).await
    }

    /// GET from the Polymarket Gamma API.
    pub async fn gamma_get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.gamma_url, path);
        self.get_json(&url).await
    }

    async fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let resp: reqwest::Response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("HTTP {status} from {url}: {body}");
        }
        resp.json::<serde_json::Value>()
            .await
            .with_context(|| format!("failed to parse JSON from {url}"))
    }
}
