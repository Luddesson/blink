//! Market Discovery Module
//!
//! Continuously polls the Polymarket Gamma API to discover active markets
//! that match configurable filters. Discovered market token IDs are pushed
//! into the engine's dynamic subscription list so the WS client can subscribe.
//!
//! Configuration (environment variables):
//! - `DISCOVERY_ENABLED`      — set to `true` to activate (default: false)
//! - `DISCOVERY_LENS`         — comma-separated lenses: all|sports|crypto|politics|geo (default: all)
//! - `DISCOVERY_MIN_LIQUIDITY`— minimum USD liquidity (default: 10000)
//! - `DISCOVERY_MIN_VOLUME`   — minimum 24h USD volume (default: 5000)
//! - `DISCOVERY_MAX_MARKETS`  — maximum concurrent subscriptions via discovery (default: 50)
//! - `DISCOVERY_REFRESH_SECS` — how often to refresh the market list (default: 300)

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Client;
use tracing::{debug, error, info, warn};

use crate::activity_log::{ActivityLog, EntryKind, push as log_push};

// ─── Constants ────────────────────────────────────────────────────────────────

const GAMMA_API: &str = "https://gamma-api.polymarket.com";
const DEFAULT_MIN_LIQUIDITY: f64 = 10_000.0;
const DEFAULT_MIN_VOLUME: f64 = 5_000.0;
const DEFAULT_MAX_MARKETS: usize = 50;
const DEFAULT_REFRESH_SECS: u64 = 300;

// ─── Config ───────────────────────────────────────────────────────────────────

/// Discovery configuration derived from environment variables.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    /// Comma-separated lens names: all, sports, crypto, politics, geo.
    pub lenses: Vec<String>,
    pub min_liquidity: f64,
    pub min_volume: f64,
    pub max_markets: usize,
    pub refresh_secs: u64,
}

impl DiscoveryConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("DISCOVERY_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let lenses = std::env::var("DISCOVERY_LENS")
            .unwrap_or_else(|_| "all".to_string())
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .collect();
        let min_liquidity = std::env::var("DISCOVERY_MIN_LIQUIDITY")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MIN_LIQUIDITY);
        let min_volume = std::env::var("DISCOVERY_MIN_VOLUME")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MIN_VOLUME);
        let max_markets = std::env::var("DISCOVERY_MAX_MARKETS")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_MARKETS);
        let refresh_secs = std::env::var("DISCOVERY_REFRESH_SECS")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_REFRESH_SECS);

        Self { enabled, lenses, min_liquidity, min_volume, max_markets, refresh_secs }
    }
}

// ─── Diagnostics ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DiscoveryDiagnostics {
    pub total_refreshes: u64,
    pub discovered_count: usize,
    pub last_error: Option<String>,
}

pub type DiscoveryDiagHandle = Arc<Mutex<DiscoveryDiagnostics>>;

// ─── Discovery Result ─────────────────────────────────────────────────────────

/// A discovered market ready to subscribe to.
#[derive(Debug, Clone)]
pub struct DiscoveredMarket {
    pub token_id: String,
    pub slug: String,
    pub question: String,
    pub liquidity: f64,
    pub volume_24h: f64,
    pub lens: String,
}

// ─── Runner ───────────────────────────────────────────────────────────────────

/// Spawns the market discovery loop.
///
/// Discovered token IDs are appended to `subscription_list` (shared with the
/// WS client's dynamic-subscribe channel).
pub async fn run_discovery(
    config: DiscoveryConfig,
    subscription_list: Arc<Mutex<Vec<String>>>,
    activity: ActivityLog,
    diag: DiscoveryDiagHandle,
) {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("discovery: failed to build HTTP client");

    info!(
        lenses = ?config.lenses,
        min_liquidity = config.min_liquidity,
        min_volume    = config.min_volume,
        max_markets   = config.max_markets,
        "Market discovery starting"
    );
    log_push(&activity, EntryKind::Engine, format!(
        "Discovery: lenses={:?} min_liquidity=${:.0} min_volume=${:.0}",
        config.lenses, config.min_liquidity, config.min_volume
    ));

    loop {
        match discover_markets(&client, &config).await {
            Ok(markets) => {
                let count = markets.len();
                debug!(count, "Discovery: found markets");

                // Update subscription list — add new tokens, stay within max_markets
                let mut subs = subscription_list.lock().unwrap();
                let existing: HashSet<String> = subs.iter().cloned().collect();
                let mut added = 0usize;

                for m in &markets {
                    if subs.len() >= config.max_markets { break; }
                    if existing.contains(&m.token_id) { continue; }
                    subs.push(m.token_id.clone());
                    added += 1;
                    debug!(token_id = %m.token_id, question = %m.question, "Discovery: added market");
                }
                drop(subs);

                if added > 0 {
                    info!(added, total = count, "Discovery: subscribed to new markets");
                    log_push(&activity, EntryKind::Engine, format!(
                        "Discovery: +{added} new markets ({count} total found)"
                    ));
                }

                {
                    let mut d = diag.lock().unwrap();
                    d.total_refreshes += 1;
                    d.discovered_count = count;
                }
            }
            Err(e) => {
                warn!(error = %e, "Discovery: refresh failed");
                let mut d = diag.lock().unwrap();
                d.last_error = Some(e.to_string());
            }
        }

        tokio::time::sleep(Duration::from_secs(config.refresh_secs)).await;
    }
}

// ─── Discovery Logic ──────────────────────────────────────────────────────────

async fn discover_markets(
    client: &Client,
    config: &DiscoveryConfig,
) -> anyhow::Result<Vec<DiscoveredMarket>> {
    let mut all_markets: Vec<DiscoveredMarket> = Vec::new();
    let mut seen_tokens: HashSet<String> = HashSet::new();

    for lens in &config.lenses {
        let markets = fetch_lens(client, lens, config).await?;
        for m in markets {
            if seen_tokens.insert(m.token_id.clone()) {
                all_markets.push(m);
            }
        }
    }

    // Sort by volume_24h descending, cap at max_markets
    all_markets.sort_by(|a, b| b.volume_24h.partial_cmp(&a.volume_24h).unwrap_or(std::cmp::Ordering::Equal));
    all_markets.truncate(config.max_markets);
    Ok(all_markets)
}

async fn fetch_lens(
    client: &Client,
    lens: &str,
    config: &DiscoveryConfig,
) -> anyhow::Result<Vec<DiscoveredMarket>> {
    let tag_slug = match lens {
        "sports"    => Some("sports"),
        "crypto"    => Some("crypto"),
        "politics"  => Some("politics"),
        "geo"       => Some("geopolitics"),
        _           => None,
    };

    let mut url = format!(
        "{GAMMA_API}/markets?active=true&closed=false\
         &order=volume24hr&ascending=false&limit={}\
         &liquidity_num_min={}&volume_num_min={}",
        config.max_markets * 2,
        config.min_liquidity,
        config.min_volume,
    );
    if let Some(tag) = tag_slug {
        url.push_str(&format!("&tag_slug={tag}"));
    }

    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;
    let arr = resp.as_array()
        .or_else(|| resp["markets"].as_array())
        .cloned()
        .unwrap_or_default();

    let markets: Vec<DiscoveredMarket> = arr.iter().filter_map(|m| {
        // Extract token IDs from `tokens` array or `clob_token_ids`
        let token_id = extract_first_token_id(m)?;
        let slug = m["slug"].as_str()?.to_string();
        let question = m["question"].as_str().unwrap_or(&slug).to_string();

        let liquidity = m["liquidity"].as_f64()
            .or_else(|| m["liquidity"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0.0);
        let volume_24h = m["volume24hr"].as_f64()
            .or_else(|| m["volume24hr"].as_str().and_then(|s| s.parse().ok()))
            .or_else(|| m["volume"].as_f64())
            .unwrap_or(0.0);

        Some(DiscoveredMarket {
            token_id,
            slug,
            question,
            liquidity,
            volume_24h,
            lens: lens.to_string(),
        })
    }).collect();

    Ok(markets)
}

fn extract_first_token_id(m: &serde_json::Value) -> Option<String> {
    // Try `tokens` array first (Gamma API format)
    if let Some(tokens) = m["tokens"].as_array() {
        if let Some(t) = tokens.first() {
            if let Some(id) = t["token_id"].as_str() {
                return Some(id.to_string());
            }
        }
    }
    // Try `clobTokenIds` array
    if let Some(ids) = m["clobTokenIds"].as_array() {
        if let Some(id) = ids.first().and_then(|v| v.as_str()) {
            return Some(id.to_string());
        }
    }
    // Try direct `conditionId` (fallback)
    m["conditionId"].as_str().map(|s| s.to_string())
}
