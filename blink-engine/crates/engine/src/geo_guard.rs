use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

const DEFAULT_GEOBLOCK_URL: &str = "https://polymarket.com/api/geoblock";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeoblockStatus {
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
}

impl GeoblockStatus {
    pub fn public_json(&self) -> serde_json::Value {
        json!({
            "blocked": self.blocked,
            "country": self.country,
            "region": self.region,
        })
    }

    pub fn location_label(&self) -> String {
        match (self.country.as_deref(), self.region.as_deref()) {
            (Some(country), Some(region)) if !region.is_empty() => {
                format!("{country}-{region}")
            }
            (Some(country), _) => country.to_string(),
            _ => "unknown".to_string(),
        }
    }
}

pub fn geoblock_url() -> String {
    std::env::var("POLYMARKET_GEOBLOCK_URL").unwrap_or_else(|_| DEFAULT_GEOBLOCK_URL.to_string())
}

pub fn guard_enabled() -> bool {
    std::env::var("BLINK_GEO_GUARD_ENABLED")
        .map(|v| !v.eq_ignore_ascii_case("false") && v != "0")
        .unwrap_or(true)
}

pub async fn check_geoblock() -> Result<GeoblockStatus> {
    let url = geoblock_url();
    let timeout_ms = std::env::var("BLINK_GEO_GUARD_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(4000);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .context("geoblock: build HTTP client")?;
    client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("geoblock: request failed for {url}"))?
        .error_for_status()
        .with_context(|| format!("geoblock: non-2xx response from {url}"))?
        .json::<GeoblockStatus>()
        .await
        .with_context(|| format!("geoblock: decode response from {url}"))
}

pub async fn ensure_trading_allowed() -> Result<GeoblockStatus> {
    anyhow::ensure!(
        guard_enabled(),
        "geo guard is disabled; refusing to enable live trading without compliance check"
    );
    let status = check_geoblock().await?;
    anyhow::ensure!(
        !status.blocked,
        "geo guard blocked live trading from {}",
        status.location_label()
    );
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_label_uses_country_region() {
        let status = GeoblockStatus {
            blocked: true,
            ip: None,
            country: Some("US".to_string()),
            region: Some("NY".to_string()),
        };
        assert_eq!(status.location_label(), "US-NY");
    }

    #[test]
    fn public_json_omits_ip() {
        let status = GeoblockStatus {
            blocked: true,
            ip: Some("203.0.113.1".to_string()),
            country: Some("US".to_string()),
            region: None,
        };
        let value = status.public_json();
        assert_eq!(value["blocked"], true);
        assert!(value.get("ip").is_none());
    }
}
