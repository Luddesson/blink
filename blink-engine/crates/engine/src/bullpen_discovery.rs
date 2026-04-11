// bullpen_discovery.rs — Multi-lens discovery scheduler with fusion scoring.
//
// Fuses Blink's Gamma API discovery with Bullpen CLI's discovery lenses
// to produce enriched market data with composite viability scores.
// Runs on a cold-path timer (default 5min), never on the hot signal path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::bullpen_bridge::{BullpenBridge, DiscoverEvent, DiscoverResponse};

// ─── Configuration ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DiscoverySchedulerConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub lenses: Vec<String>,
    pub limit_per_lens: usize,
    pub prewarm_on_startup: bool,
    pub prewarm_cache: bool,
    pub stale_prune_secs: u64,
}

impl DiscoverySchedulerConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("BULLPEN_DISCOVER_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            interval_secs: std::env::var("BULLPEN_DISCOVER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            lenses: std::env::var("BULLPEN_DISCOVER_LENSES")
                .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_else(|_| vec!["sports".into(), "crypto".into(), "traders".into()]),
            limit_per_lens: std::env::var("BULLPEN_DISCOVER_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50),
            prewarm_on_startup: std::env::var("BULLPEN_DISCOVER_PREWARM")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            prewarm_cache: std::env::var("BULLPEN_DISCOVER_PREWARM")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            stale_prune_secs: 3600,
        }
    }
}

// ─── Enriched Market Data ─────────────────────────────────────────────────

/// Fused discovery data per token_id, combining multi-lens signals.
#[derive(Debug, Clone)]
pub struct EnrichedMarket {
    pub token_id: String,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub category: Option<String>,
    pub volume: Option<f64>,
    pub liquidity: Option<f64>,

    /// Which Bullpen lenses found this market (e.g., ["sports", "traders"])
    pub discovery_lenses: Vec<String>,
    /// Best volume rank across all lenses (lower = higher volume)
    pub best_volume_rank: Option<usize>,
    /// Found in the "traders" lens → smart money is interested
    pub smart_money_interest: bool,
    /// Found in the "flow" lens
    pub flow_detected: bool,

    /// Composite viability score: 0.0–1.0
    pub viability_score: f64,
    /// Additional conviction boost for sizing: 0.0–0.05
    pub conviction_boost: f64,

    pub first_seen: Instant,
    pub last_seen: Instant,
    pub seen_count: u32,
}

// ─── Discovery Store ──────────────────────────────────────────────────────

/// In-memory store of enriched markets keyed by token_id.
pub struct DiscoveryStore {
    markets: HashMap<String, EnrichedMarket>,
    slug_to_tokens: HashMap<String, Vec<String>>,
    pub last_scan: Option<Instant>,
    pub scan_count: u64,
}

impl DiscoveryStore {
    pub fn new() -> Self {
        Self {
            markets: HashMap::new(),
            slug_to_tokens: HashMap::new(),
            last_scan: None,
            scan_count: 0,
        }
    }

    /// Look up enrichment data for a token_id.
    pub fn get(&self, token_id: &str) -> Option<&EnrichedMarket> {
        self.markets.get(token_id)
    }

    /// Get conviction_boost for a token_id (0.0 if unknown).
    pub fn conviction_boost(&self, token_id: &str) -> f64 {
        self.markets
            .get(token_id)
            .map(|m| m.conviction_boost)
            .unwrap_or(0.0)
    }

    /// All discovered token_ids.
    pub fn token_ids(&self) -> Vec<String> {
        self.markets.keys().cloned().collect()
    }

    /// Number of tracked markets.
    pub fn len(&self) -> usize {
        self.markets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.markets.is_empty()
    }

    /// Diagnostics summary for TUI / logging.
    pub fn summary(&self) -> DiscoverySummary {
        let smart_money_count = self
            .markets
            .values()
            .filter(|m| m.smart_money_interest)
            .count();
        let avg_viability = if self.markets.is_empty() {
            0.0
        } else {
            self.markets.values().map(|m| m.viability_score).sum::<f64>()
                / self.markets.len() as f64
        };
        DiscoverySummary {
            total_markets: self.markets.len(),
            smart_money_markets: smart_money_count,
            avg_viability,
            scan_count: self.scan_count,
            last_scan_ago_secs: self.last_scan.map(|t| t.elapsed().as_secs()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoverySummary {
    pub total_markets: usize,
    pub smart_money_markets: usize,
    pub avg_viability: f64,
    pub scan_count: u64,
    pub last_scan_ago_secs: Option<u64>,
}

// ─── Scheduler ────────────────────────────────────────────────────────────

pub struct DiscoveryScheduler {
    bridge: Arc<BullpenBridge>,
    store: Arc<RwLock<DiscoveryStore>>,
    config: DiscoverySchedulerConfig,
}

impl DiscoveryScheduler {
    pub fn new(
        bridge: Arc<BullpenBridge>,
        store: Arc<RwLock<DiscoveryStore>>,
        config: DiscoverySchedulerConfig,
    ) -> Self {
        Self {
            bridge,
            store,
            config,
        }
    }

    /// Shared handle to the discovery store (pass to paper_engine for lookups).
    pub fn store(&self) -> Arc<RwLock<DiscoveryStore>> {
        self.store.clone()
    }

    /// Background loop — spawn via `tokio::spawn(scheduler.run(shutdown))`.
    pub async fn run(self, shutdown: Arc<AtomicBool>) {
        if !self.config.enabled {
            info!("Bullpen discovery scheduler disabled");
            return;
        }

        let interval = Duration::from_secs(self.config.interval_secs);
        let mut ticker = tokio::time::interval(interval);

        if self.config.prewarm_on_startup {
            info!("Bullpen discovery: pre-warming on startup");
            if let Err(e) = self.full_scan().await {
                warn!("Discovery pre-warm failed: {e}");
            }
        }

        loop {
            ticker.tick().await;
            if shutdown.load(Ordering::Relaxed) {
                info!("Bullpen discovery scheduler shutting down");
                break;
            }
            if let Err(e) = self.full_scan().await {
                warn!("Discovery scan failed: {e}");
            }
        }
    }

    /// Execute a full multi-lens discovery scan.
    async fn full_scan(&self) -> anyhow::Result<()> {
        let lenses = self.config.lenses.clone();
        let _limit = self.config.limit_per_lens;

        // Parallel lens scans
        let futures: Vec<_> = lenses
            .iter()
            .map(|lens| {
                let bridge = self.bridge.clone();
                let lens = lens.clone();
                async move {
                    let result = bridge.discover_markets(&lens).await;
                    (lens, result)
                }
            })
            .collect();

        let results = futures_util::future::join_all(futures).await;

        let mut lens_results: Vec<(String, DiscoverResponse)> = Vec::new();
        for (lens, result) in results {
            match result {
                Ok(resp) => {
                    let event_count = resp.events.len();
                    info!(lens = %lens, events = event_count, "Bullpen discover lens complete");
                    lens_results.push((lens, resp));
                }
                Err(e) => {
                    warn!(lens = %lens, error = %e, "Bullpen discover lens failed");
                }
            }
        }

        self.fuse_results(&lens_results).await;
        Ok(())
    }

    /// Merge multi-lens results into unified EnrichedMarket entries.
    async fn fuse_results(&self, lens_results: &[(String, DiscoverResponse)]) {
        let mut store = self.store.write().await;
        let now = Instant::now();

        for (lens, response) in lens_results {
            for (event_rank, event) in response.events.iter().enumerate() {
                let slug = event.slug.clone();
                let title = event.title.clone();
                let category = event.category.clone();

                // Collect token_ids from event markets
                let token_ids: Vec<String> = event
                    .markets
                    .iter()
                    .filter_map(|m| m.token_id.clone())
                    .collect();

                if let Some(ref s) = slug {
                    store.slug_to_tokens.insert(s.clone(), token_ids.clone());
                }

                for token_id in &token_ids {
                    let entry = store
                        .markets
                        .entry(token_id.clone())
                        .or_insert_with(|| EnrichedMarket {
                            token_id: token_id.clone(),
                            slug: slug.clone(),
                            title: title.clone(),
                            category: category.clone(),
                            volume: Self::event_total_volume(event),
                            liquidity: Self::event_total_liquidity(event),
                            discovery_lenses: vec![],
                            best_volume_rank: None,
                            smart_money_interest: false,
                            flow_detected: false,
                            viability_score: 0.0,
                            conviction_boost: 0.0,
                            first_seen: now,
                            last_seen: now,
                            seen_count: 0,
                        });

                    // Merge lens data
                    if !entry.discovery_lenses.contains(lens) {
                        entry.discovery_lenses.push(lens.clone());
                    }
                    entry.last_seen = now;
                    entry.seen_count += 1;

                    match lens.as_str() {
                        "traders" | "walletscope" => entry.smart_money_interest = true,
                        "flow" => entry.flow_detected = true,
                        _ => {}
                    }

                    if entry.best_volume_rank.is_none()
                        || event_rank < entry.best_volume_rank.unwrap()
                    {
                        entry.best_volume_rank = Some(event_rank);
                    }
                }
            }
        }

        // Recompute scores for all entries
        for entry in store.markets.values_mut() {
            entry.viability_score = compute_viability_score(entry);
            entry.conviction_boost = compute_conviction_boost(entry);
        }

        // Prune stale entries
        let stale_cutoff = Duration::from_secs(self.config.stale_prune_secs);
        store.markets.retain(|_, v| v.last_seen.elapsed() < stale_cutoff);

        store.last_scan = Some(now);
        store.scan_count += 1;

        info!(
            total_markets = store.markets.len(),
            scan_count = store.scan_count,
            "Discovery store updated"
        );
    }

    fn event_total_volume(event: &DiscoverEvent) -> Option<f64> {
        let sum: f64 = event
            .markets
            .iter()
            .filter_map(|m| m.volume)
            .sum();
        if sum > 0.0 {
            Some(sum)
        } else {
            None
        }
    }

    fn event_total_liquidity(event: &DiscoverEvent) -> Option<f64> {
        let sum: f64 = event
            .markets
            .iter()
            .filter_map(|m| m.liquidity)
            .sum();
        if sum > 0.0 {
            Some(sum)
        } else {
            None
        }
    }
}

// ─── Scoring Functions ────────────────────────────────────────────────────

/// Composite viability score: 0.0–1.0
/// Weights: lenses (0.30) + smart_money (0.25) + volume_rank (0.20) +
///          flow (0.15) + recency (0.10)
fn compute_viability_score(market: &EnrichedMarket) -> f64 {
    let lens_score = (market.discovery_lenses.len() as f64 / 4.0).min(1.0) * 0.30;
    let sm_score = if market.smart_money_interest {
        0.25
    } else {
        0.0
    };
    let rank_score = market
        .best_volume_rank
        .map(|r| (1.0 - (r as f64 / 50.0).min(1.0)) * 0.20)
        .unwrap_or(0.0);
    let flow_score = if market.flow_detected { 0.15 } else { 0.0 };
    let recency = (1.0 - (market.last_seen.elapsed().as_secs() as f64 / 3600.0).min(1.0)) * 0.10;

    lens_score + sm_score + rank_score + flow_score + recency
}

/// Conviction boost from discovery data: 0.0–0.05
/// Added to base conviction_multiplier() result in paper_engine.
fn compute_conviction_boost(market: &EnrichedMarket) -> f64 {
    let mut boost: f64 = 0.0;

    if market.smart_money_interest {
        boost += 0.02;
    }
    if market.discovery_lenses.len() >= 3 {
        boost += 0.015;
    }
    if market.flow_detected {
        boost += 0.01;
    }
    if market.seen_count >= 3 {
        boost += 0.005;
    }

    boost.min(0.05)
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_market(lenses: &[&str], sm: bool, flow: bool, rank: Option<usize>) -> EnrichedMarket {
        EnrichedMarket {
            token_id: "tok_1".into(),
            slug: Some("test-market".into()),
            title: Some("Test Market".into()),
            category: Some("sports".into()),
            volume: Some(100_000.0),
            liquidity: Some(50_000.0),
            discovery_lenses: lenses.iter().map(|s| s.to_string()).collect(),
            best_volume_rank: rank,
            smart_money_interest: sm,
            flow_detected: flow,
            viability_score: 0.0,
            conviction_boost: 0.0,
            first_seen: Instant::now(),
            last_seen: Instant::now(),
            seen_count: 1,
        }
    }

    #[test]
    fn viability_empty_market() {
        let m = make_market(&[], false, false, None);
        let score = compute_viability_score(&m);
        // Only recency component (fresh = ~0.10)
        assert!(score > 0.0 && score < 0.15, "score={score}");
    }

    #[test]
    fn viability_high_quality_market() {
        let m = make_market(&["sports", "traders", "flow", "crypto"], true, true, Some(0));
        let score = compute_viability_score(&m);
        // All factors contribute → close to 1.0
        assert!(score > 0.85, "score={score}");
    }

    #[test]
    fn conviction_boost_caps_at_005() {
        let mut m = make_market(&["a", "b", "c", "d"], true, true, Some(0));
        m.seen_count = 10;
        let boost = compute_conviction_boost(&m);
        assert!((boost - 0.05).abs() < f64::EPSILON, "boost={boost}");
    }

    #[test]
    fn conviction_boost_zero_for_bare_market() {
        let m = make_market(&["sports"], false, false, Some(30));
        let boost = compute_conviction_boost(&m);
        assert!((boost - 0.0).abs() < f64::EPSILON, "boost={boost}");
    }

    #[test]
    fn store_operations() {
        let mut store = DiscoveryStore::new();
        assert!(store.is_empty());
        assert_eq!(store.conviction_boost("unknown"), 0.0);

        store.markets.insert(
            "tok_1".into(),
            {
                let mut m = make_market(&["traders"], true, false, Some(5));
                m.conviction_boost = 0.02;
                m
            },
        );

        assert_eq!(store.len(), 1);
        assert!((store.conviction_boost("tok_1") - 0.02).abs() < f64::EPSILON);
        assert!(store.get("tok_1").is_some());

        let summary = store.summary();
        assert_eq!(summary.total_markets, 1);
        assert_eq!(summary.smart_money_markets, 1);
    }

    #[test]
    fn config_defaults() {
        let config = DiscoverySchedulerConfig::from_env();
        assert!(!config.enabled);
        assert_eq!(config.interval_secs, 300);
        assert_eq!(config.limit_per_lens, 50);
        assert_eq!(config.lenses.len(), 3);
    }
}
