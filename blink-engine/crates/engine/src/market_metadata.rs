//! Market metadata fetching and caching

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::Value;

use crate::types::MarketMetadata;

/// Cache entry with TTL
struct CacheEntry {
    metadata: MarketMetadata,
    cached_at: Instant,
}

/// Market metadata fetcher with caching
pub struct MetadataFetcher {
    client: Client,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    cache_ttl: Duration,
    gamma_api_url: String,
}

impl MetadataFetcher {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|e| {
                    panic!("Failed to create HTTP client for market metadata: {e}")
                }),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Duration::from_secs(300), // 5 minutes
            gamma_api_url: "https://gamma-api.polymarket.com".to_string(),
        }
    }

    /// Fetch market metadata with caching
    pub async fn fetch(&self, token_id: &str) -> Result<MarketMetadata, String> {
        // Check cache first
        {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = cache.get(token_id) {
                if entry.cached_at.elapsed() < self.cache_ttl {
                    tracing::debug!("Cache hit for token {}", token_id);
                    return Ok(entry.metadata.clone());
                }
            }
        }

        // Fetch from API
        tracing::debug!("Fetching metadata for token {} from API", token_id);
        let metadata = self.fetch_from_api(token_id).await?;

        // Update cache
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            cache.insert(
                token_id.to_string(),
                CacheEntry {
                    metadata: metadata.clone(),
                    cached_at: Instant::now(),
                },
            );
        }

        Ok(metadata)
    }

    async fn fetch_from_api(&self, token_id: &str) -> Result<MarketMetadata, String> {
        // Try Gamma API: /markets?token_id=<token_id>
        let url = format!("{}/markets?token_id={}", self.gamma_api_url, token_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("API request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("API returned {}", response.status()));
        }

        let data: Value = response
            .json()
            .await
            .map_err(|e| format!("JSON parse failed: {}", e))?;

        // Parse response (adjust based on actual API structure)
        self.parse_gamma_response(token_id, &data)
    }

    fn parse_gamma_response(&self, token_id: &str, data: &Value) -> Result<MarketMetadata, String> {
        // Gamma API typically returns array of markets
        let markets = data.as_array().ok_or("Expected array of markets")?;

        if markets.is_empty() {
            return Err("No market found for token".to_string());
        }

        let market = &markets[0];

        // Extract fields (adjust based on actual API response)
        let market_id = market
            .get("condition_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let category = market
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let tags: Vec<String> = market
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let volume_24h = market
            .get("volume24hr")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let liquidity = market
            .get("liquidity")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        // Flexible timestamp parser: RFC3339 → NaiveDateTime → date-only
        let parse_ts = |s: &str| -> Option<i64> {
            let s = s.trim();
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp())
                .or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                        .ok()
                        .map(|ndt| ndt.and_utc().timestamp())
                })
                .or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                        .ok()
                        .map(|ndt| ndt.and_utc().timestamp())
                })
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .ok()
                        .map(|d| {
                            d.and_hms_opt(23, 59, 59)
                                .expect("infallible: 23:59:59 is always valid")
                                .and_utc()
                                .timestamp()
                        })
                })
        };

        // game_start_date / gameStartTime = actual game kickoff (sports markets)
        let event_start_time = [
            "game_start_date",
            "gameStartDate",
            "gameStartTime",
            "game_start_time",
            "start_date_iso",
        ]
        .iter()
        .find_map(|k| market.get(*k)?.as_str().and_then(|s| parse_ts(s)));

        // endDate (full datetime) preferred over end_date_iso (date-only)
        let event_end_time = [
            "endDate",
            "end_date",
            "end_date_iso",
            "endDateIso",
            "resolution_date",
        ]
        .iter()
        .find_map(|k| market.get(*k)?.as_str().and_then(|s| parse_ts(s)));

        let closed = market
            .get("closed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(MarketMetadata {
            market_id,
            token_id: token_id.to_string(),
            category,
            tags,
            volume_24h,
            liquidity,
            event_start_time,
            event_end_time,
            closed,
        })
    }

    /// Clear expired cache entries
    pub fn cleanup_cache(&self) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, entry| entry.cached_at.elapsed() < self.cache_ttl);
    }
}

impl Default for MetadataFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_is_viable_liquidity() {
        use crate::types::FilterConfig;

        let config = FilterConfig::default();

        let mut metadata = MarketMetadata {
            market_id: "test".to_string(),
            token_id: "token".to_string(),
            category: "sports".to_string(),
            tags: vec![],
            volume_24h: 0.0,
            liquidity: 50_000.0, // Too low
            event_start_time: None,
            event_end_time: None,
            closed: false,
        };

        // Should fail - liquidity too low
        assert!(metadata.is_viable(&config).is_err());

        // Fix liquidity
        metadata.liquidity = 150_000.0;
        assert!(metadata.is_viable(&config).is_ok());
    }

    #[test]
    fn test_extract_sport() {
        let mut metadata = MarketMetadata {
            market_id: "test".to_string(),
            token_id: "token".to_string(),
            category: "sports".to_string(),
            tags: vec!["NFL".to_string()],
            volume_24h: 0.0,
            liquidity: 100_000.0,
            event_start_time: None,
            event_end_time: None,
            closed: false,
        };

        assert_eq!(metadata.extract_sport(), Some("NFL".to_string()));

        metadata.tags = vec!["Soccer".to_string(), "Premier League".to_string()];
        assert_eq!(metadata.extract_sport(), Some("Soccer".to_string()));
    }
}
