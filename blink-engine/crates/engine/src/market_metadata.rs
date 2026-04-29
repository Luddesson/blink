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
        let urls = [
            // Official Gamma query parameter for CLOB token IDs.
            format!("{}/markets?clob_token_ids={}", self.gamma_api_url, token_id),
            // Legacy fallback kept for older deployments and local fixtures.
            format!("{}/markets?token_id={}", self.gamma_api_url, token_id),
        ];

        let mut last_err = None;
        for url in urls {
            let response = match self.client.get(&url).send().await {
                Ok(resp) => resp,
                Err(e) => {
                    last_err = Some(format!("API request failed: {}", e));
                    continue;
                }
            };

            if !response.status().is_success() {
                last_err = Some(format!("API returned {}", response.status()));
                continue;
            }

            let data: Value = match response.json().await {
                Ok(data) => data,
                Err(e) => {
                    last_err = Some(format!("JSON parse failed: {}", e));
                    continue;
                }
            };

            match self.parse_gamma_response(token_id, &data) {
                Ok(metadata) => return Ok(metadata),
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err.unwrap_or_else(|| "metadata fetch failed".to_string()))
    }

    fn parse_gamma_response(&self, token_id: &str, data: &Value) -> Result<MarketMetadata, String> {
        // Gamma API typically returns array of markets
        let markets = data.as_array().ok_or("Expected array of markets")?;

        if markets.is_empty() {
            return Err("No market found for token".to_string());
        }

        let market = &markets[0];

        // Extract fields (adjust based on actual API response)
        let market_id =
            string_field(market, &["conditionId", "condition_id", "id"]).unwrap_or_default();

        let category = string_field(market, &["category"]).unwrap_or_else(|| "unknown".to_string());

        let tags: Vec<String> = market
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        t.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| string_field(t, &["label", "slug", "name"]))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let volume_24h = f64_field(
            market,
            &["volume24hr", "volume24hrClob", "volumeNum", "volume"],
        )
        .unwrap_or(0.0);

        let liquidity =
            f64_field(market, &["liquidityNum", "liquidityClob", "liquidity"]).unwrap_or(0.0);

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

        let closed = bool_field(market, &["closed"]).unwrap_or(false);
        let market_neg_risk = bool_field(
            market,
            &["negRisk", "neg_risk", "enableNegRisk", "enable_neg_risk"],
        )
        .unwrap_or(false);
        let event_neg_risk = market
            .get("events")
            .and_then(|v| v.as_array())
            .map(|events| {
                events.iter().any(|event| {
                    bool_field(
                        event,
                        &["negRisk", "neg_risk", "enableNegRisk", "enable_neg_risk"],
                    )
                    .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        let enable_neg_risk = bool_field(market, &["enableNegRisk", "enable_neg_risk"])
            .unwrap_or(false)
            || event_neg_risk;
        let minimum_tick_size = string_field(
            market,
            &[
                "minimum_tick_size",
                "minimumTickSize",
                "orderPriceMinTickSize",
                "order_price_min_tick_size",
                "tickSize",
                "tick_size",
            ],
        );

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
            neg_risk: market_neg_risk || event_neg_risk,
            enable_neg_risk,
            minimum_tick_size,
        })
    }

    /// Clear expired cache entries
    pub fn cleanup_cache(&self) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, entry| entry.cached_at.elapsed() < self.cache_ttl);
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let v = value.get(*key)?;
        v.as_str()
            .map(|s| s.to_string())
            .or_else(|| v.as_f64().map(|n| n.to_string()))
            .or_else(|| v.as_i64().map(|n| n.to_string()))
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    })
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let v = value.get(*key)?;
        v.as_bool().or_else(|| {
            v.as_str().and_then(|s| {
                if s.eq_ignore_ascii_case("true") || s == "1" {
                    Some(true)
                } else if s.eq_ignore_ascii_case("false") || s == "0" {
                    Some(false)
                } else {
                    None
                }
            })
        })
    })
}

fn f64_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let v = value.get(*key)?;
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    })
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
            neg_risk: false,
            enable_neg_risk: false,
            minimum_tick_size: None,
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
            neg_risk: false,
            enable_neg_risk: false,
            minimum_tick_size: None,
        };

        assert_eq!(metadata.extract_sport(), Some("NFL".to_string()));

        metadata.tags = vec!["Soccer".to_string(), "Premier League".to_string()];
        assert_eq!(metadata.extract_sport(), Some("Soccer".to_string()));
    }

    #[test]
    fn parses_neg_risk_and_tick_size_from_gamma_market() {
        let fetcher = MetadataFetcher::new();
        let data = serde_json::json!([
            {
                "conditionId": "0xabc",
                "category": "sports",
                "volume24hr": "123.45",
                "liquidityNum": 100000.0,
                "closed": false,
                "negRisk": true,
                "orderPriceMinTickSize": 0.01,
                "events": [
                    { "enableNegRisk": true }
                ],
                "tags": [
                    { "label": "Soccer" }
                ]
            }
        ]);

        let metadata = fetcher.parse_gamma_response("token", &data).unwrap();
        assert_eq!(metadata.market_id, "0xabc");
        assert!(metadata.neg_risk);
        assert!(metadata.enable_neg_risk);
        assert_eq!(metadata.minimum_tick_size.as_deref(), Some("0.01"));
        assert_eq!(metadata.volume_24h, 123.45);
        assert_eq!(metadata.tags, vec!["Soccer"]);
    }
}
