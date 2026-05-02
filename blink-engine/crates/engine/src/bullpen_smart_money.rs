// bullpen_smart_money.rs — Smart money intelligence layer.
//
// Three layers:
//   1. TraderProfileCache — cached wallet profiling with trust scores
//   2. ConvergenceDetector — detects multiple whales converging on same market
//   3. SmartMoneyFeed — high-P&L trade feed monitor
//
// All run on cold-path timers (30-60s), never on the hot signal path.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::bullpen_bridge::{BullpenBridge, SmartMoneyEntry, TraderProfile};

// ─── Configuration ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SmartMoneyConfig {
    pub enabled: bool,
    pub poll_secs: u64,
    pub profile_cache_ttl_secs: u64,
    pub convergence_threshold: usize,
    pub min_trust_score: f64,
    pub signal_type: String,
    pub feed_min_pnl: f64,
    pub auto_track_top_n: usize,
}

impl SmartMoneyConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("BULLPEN_SM_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            poll_secs: std::env::var("BULLPEN_SM_POLL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            profile_cache_ttl_secs: std::env::var("BULLPEN_SM_PROFILE_CACHE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            convergence_threshold: std::env::var("BULLPEN_SM_CONVERGENCE_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            min_trust_score: std::env::var("BULLPEN_SM_MIN_WIN_RATE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.60),
            signal_type: std::env::var("BULLPEN_SM_SIGNAL_TYPE")
                .unwrap_or_else(|_| "aggregated".into()),
            feed_min_pnl: std::env::var("BULLPEN_SM_FEED_MIN_PNL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5000.0),
            auto_track_top_n: std::env::var("BULLPEN_SM_AUTO_TRACK_TOP_N")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
        }
    }
}

// ─── Layer 1: Trader Profile Cache ────────────────────────────────────────

/// Cached trader profile with trust scoring.
#[derive(Debug, Clone)]
pub struct CachedProfile {
    pub profile: TraderProfile,
    pub trust_score: f64,
    pub fetched_at: Instant,
}

pub struct TraderProfileCache {
    bridge: Arc<BullpenBridge>,
    profiles: Arc<RwLock<HashMap<String, CachedProfile>>>,
    cache_ttl: Duration,
}

impl TraderProfileCache {
    pub fn new(bridge: Arc<BullpenBridge>, cache_ttl_secs: u64) -> Self {
        Self {
            bridge,
            profiles: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Duration::from_secs(cache_ttl_secs),
        }
    }

    /// Get or fetch a trader profile. Returns cached version if fresh.
    pub async fn get_or_fetch(&self, address: &str) -> Option<CachedProfile> {
        // Check cache
        {
            let profiles = self.profiles.read().await;
            if let Some(cached) = profiles.get(address) {
                if cached.fetched_at.elapsed() < self.cache_ttl {
                    return Some(cached.clone());
                }
            }
        }

        // Fetch from Bullpen
        let json_result = self.bridge.trader_profile(address).await;
        let profile = match json_result {
            Ok(generic) => match serde_json::from_value::<TraderProfile>(generic.0) {
                Ok(p) => p,
                Err(e) => {
                    debug!("Failed to parse trader profile for {address}: {e}");
                    return None;
                }
            },
            Err(e) => {
                debug!("Failed to fetch trader profile for {address}: {e}");
                return None;
            }
        };

        let trust_score = compute_trust_score(&profile);
        let cached = CachedProfile {
            profile,
            trust_score,
            fetched_at: Instant::now(),
        };

        // Update cache
        {
            let mut profiles = self.profiles.write().await;
            profiles.insert(address.to_string(), cached.clone());
        }

        Some(cached)
    }

    /// Get trust score for a wallet (returns default 0.3 if not cached).
    pub async fn trust_score(&self, address: &str) -> f64 {
        let profiles = self.profiles.read().await;
        profiles.get(address).map(|c| c.trust_score).unwrap_or(0.3)
    }

    /// Number of cached profiles.
    pub async fn cached_count(&self) -> usize {
        self.profiles.read().await.len()
    }

    /// Pre-warm cache with a list of wallet addresses.
    pub async fn prewarm(&self, wallets: &[String]) {
        let futs: Vec<_> = wallets.iter().map(|addr| self.get_or_fetch(addr)).collect();
        let results = futures_util::future::join_all(futs).await;
        let success = results.iter().filter(|r| r.is_some()).count();
        info!(
            total = wallets.len(),
            success, "Pre-warmed trader profile cache"
        );
    }
}

/// Trust score: win_rate × volume_weight × trade_count_factor
/// Higher = more trustworthy wallet.
fn compute_trust_score(profile: &TraderProfile) -> f64 {
    let win_rate = profile.win_rate.unwrap_or(0.5).clamp(0.0, 1.0);
    let volume = profile.volume_total.unwrap_or(0.0).max(1.0);
    let volume_factor = (volume.log10() / 7.0).clamp(0.0, 1.0); // $10M = 1.0
    let trades = profile.total_trades.unwrap_or(0) as f64;
    let trades_factor = (trades / 1000.0).clamp(0.0, 1.0);

    win_rate * 0.5 + volume_factor * 0.3 + trades_factor * 0.2
}

// ─── Layer 2: Convergence Detection ───────────────────────────────────────

/// When multiple trusted wallets enter the same market, that's a convergence signal.
#[derive(Debug, Clone)]
pub struct ConvergenceSignal {
    pub market: String,
    pub wallets: Vec<ConvergentWallet>,
    pub convergence_score: f64,
    pub net_direction: String, // "buy" or "sell"
    pub total_usd: f64,
    pub detected_at: Instant,
}

#[derive(Debug, Clone)]
pub struct ConvergentWallet {
    pub address: String,
    pub trust_score: f64,
    pub side: String,
    pub amount_usd: f64,
}

/// Internal: timestamped smart money activity per market.
#[derive(Debug, Clone)]
struct TimedEntry {
    wallet: String,
    side: String,
    amount_usd: f64,
    trust_score: f64,
    detected_at: Instant,
}

pub struct ConvergenceStore {
    market_activity: HashMap<String, Vec<TimedEntry>>,
    pub active_signals: Vec<ConvergenceSignal>,
    lookback: Duration,
}

impl ConvergenceStore {
    pub fn new(lookback_secs: u64) -> Self {
        Self {
            market_activity: HashMap::new(),
            active_signals: Vec::new(),
            lookback: Duration::from_secs(lookback_secs),
        }
    }

    /// Get active convergence signal for a market (by market name/slug).
    pub fn get_convergence(&self, market: &str) -> Option<&ConvergenceSignal> {
        self.active_signals.iter().find(|s| s.market == market)
    }

    /// Max convergence_score across all active signals for a market string match.
    pub fn convergence_boost(&self, search: &str) -> f64 {
        self.active_signals
            .iter()
            .filter(|s| s.market.contains(search))
            .map(|s| s.convergence_score * 0.05) // Max 5% sizing boost
            .fold(0.0_f64, f64::max)
    }

    pub fn active_count(&self) -> usize {
        self.active_signals.len()
    }

    /// Summary for TUI/logging.
    pub fn summary(&self) -> ConvergenceSummary {
        ConvergenceSummary {
            active_signals: self.active_signals.len(),
            tracked_markets: self.market_activity.len(),
            top_signal: self
                .active_signals
                .iter()
                .max_by(|a, b| {
                    a.convergence_score
                        .partial_cmp(&b.convergence_score)
                        .unwrap()
                })
                .cloned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConvergenceSummary {
    pub active_signals: usize,
    pub tracked_markets: usize,
    pub top_signal: Option<ConvergenceSignal>,
}

// ─── Smart Money Monitor ──────────────────────────────────────────────────

/// Combined smart money monitor: profiles + convergence + feed.
pub struct SmartMoneyMonitor {
    bridge: Arc<BullpenBridge>,
    profile_cache: Arc<TraderProfileCache>,
    convergence_store: Arc<RwLock<ConvergenceStore>>,
    config: SmartMoneyConfig,
}

impl SmartMoneyMonitor {
    pub fn new(bridge: Arc<BullpenBridge>, config: SmartMoneyConfig) -> Self {
        let profile_cache = Arc::new(TraderProfileCache::new(
            Arc::clone(&bridge),
            config.profile_cache_ttl_secs,
        ));
        let convergence_store = Arc::new(RwLock::new(ConvergenceStore::new(
            config.poll_secs * 10, // lookback = 10 poll intervals
        )));
        Self {
            bridge,
            profile_cache,
            convergence_store,
            config,
        }
    }

    /// Shared handle to convergence store (pass to paper_engine).
    pub fn convergence_store(&self) -> Arc<RwLock<ConvergenceStore>> {
        Arc::clone(&self.convergence_store)
    }

    /// Shared handle to profile cache.
    pub fn profile_cache(&self) -> Arc<TraderProfileCache> {
        Arc::clone(&self.profile_cache)
    }

    /// Background loop — poll smart money feed, detect convergence.
    pub async fn run(self, shutdown: Arc<AtomicBool>) {
        if !self.config.enabled {
            info!("Bullpen smart money monitor disabled");
            return;
        }

        let interval = Duration::from_secs(self.config.poll_secs);
        let mut ticker = tokio::time::interval(interval);

        loop {
            ticker.tick().await;
            if shutdown.load(Ordering::Relaxed) {
                info!("Smart money monitor shutting down");
                break;
            }
            if let Err(e) = self.poll_and_detect().await {
                debug!("Smart money poll failed: {e}");
            }
        }
    }

    async fn poll_and_detect(&self) -> anyhow::Result<()> {
        // Fetch smart money data
        let json = self.bridge.smart_money(&self.config.signal_type).await?;
        let entries: Vec<SmartMoneyEntry> = match serde_json::from_value(json.0.clone()) {
            Ok(v) => v,
            Err(_) => {
                // Try wrapping: Bullpen may return a single object or nested structure
                if let Some(arr) = json.0.as_array() {
                    arr.iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect()
                } else {
                    vec![]
                }
            }
        };

        if entries.is_empty() {
            return Ok(());
        }

        let mut store = self.convergence_store.write().await;
        let now = Instant::now();

        // Add entries with trust scores
        for entry in &entries {
            let wallet = match &entry.wallet {
                Some(w) => w.clone(),
                None => continue,
            };
            let market = entry.market.clone().unwrap_or_default();
            if market.is_empty() {
                continue;
            }

            let trust = self.profile_cache.trust_score(&wallet).await;
            if trust < self.config.min_trust_score {
                continue;
            }

            let timed = TimedEntry {
                wallet,
                side: entry.action.clone().unwrap_or_else(|| "buy".into()),
                amount_usd: entry.amount_usd.unwrap_or(0.0),
                trust_score: trust,
                detected_at: now,
            };

            store.market_activity.entry(market).or_default().push(timed);
        }

        // Prune old entries outside lookback window
        let lookback = store.lookback;
        for entries in store.market_activity.values_mut() {
            entries.retain(|e| e.detected_at.elapsed() < lookback);
        }
        store.market_activity.retain(|_, v| !v.is_empty());

        // Detect convergence
        let mut signals = Vec::new();
        for (market, entries) in &store.market_activity {
            let unique_wallets: HashSet<&str> = entries.iter().map(|e| e.wallet.as_str()).collect();

            if unique_wallets.len() >= self.config.convergence_threshold {
                let wallets: Vec<ConvergentWallet> = entries
                    .iter()
                    .map(|e| ConvergentWallet {
                        address: e.wallet.clone(),
                        trust_score: e.trust_score,
                        side: e.side.clone(),
                        amount_usd: e.amount_usd,
                    })
                    .collect();

                let buy_weight: f64 = wallets
                    .iter()
                    .filter(|w| w.side == "buy")
                    .map(|w| w.trust_score * w.amount_usd.max(1.0))
                    .sum();
                let sell_weight: f64 = wallets
                    .iter()
                    .filter(|w| w.side == "sell")
                    .map(|w| w.trust_score * w.amount_usd.max(1.0))
                    .sum();

                let net_direction = if buy_weight >= sell_weight {
                    "buy"
                } else {
                    "sell"
                };
                let total_usd: f64 = wallets.iter().map(|w| w.amount_usd).sum();
                let avg_trust: f64 =
                    wallets.iter().map(|w| w.trust_score).sum::<f64>() / wallets.len() as f64;
                let convergence_score =
                    (avg_trust * (unique_wallets.len() as f64 / 10.0).min(1.0)).min(1.0);

                info!(
                    market = %market,
                    wallets = unique_wallets.len(),
                    score = format!("{:.2}", convergence_score),
                    direction = net_direction,
                    total_usd = format!("${:.0}", total_usd),
                    "🐋 Whale convergence detected"
                );

                signals.push(ConvergenceSignal {
                    market: market.clone(),
                    wallets,
                    convergence_score,
                    net_direction: net_direction.to_string(),
                    total_usd,
                    detected_at: now,
                });
            }
        }

        store.active_signals = signals;
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_score_high_performer() {
        let profile = TraderProfile {
            address: Some("0xabc".into()),
            volume_total: Some(5_000_000.0), // $5M
            win_rate: Some(0.85),
            total_trades: Some(500),
            avg_trade_size: Some(10_000.0),
            specialization: Some("sports".into()),
            pnl_total: Some(200_000.0),
        };
        let score = compute_trust_score(&profile);
        assert!(score > 0.6, "score={score}");
    }

    #[test]
    fn trust_score_low_performer() {
        let profile = TraderProfile {
            address: Some("0xdef".into()),
            volume_total: Some(100.0),
            win_rate: Some(0.45),
            total_trades: Some(5),
            avg_trade_size: None,
            specialization: None,
            pnl_total: Some(-50.0),
        };
        let score = compute_trust_score(&profile);
        assert!(score < 0.4, "score={score}");
    }

    #[test]
    fn trust_score_empty_profile() {
        let profile = TraderProfile {
            address: None,
            volume_total: None,
            win_rate: None,
            total_trades: None,
            avg_trade_size: None,
            specialization: None,
            pnl_total: None,
        };
        let score = compute_trust_score(&profile);
        // Default win_rate 0.5 * 0.5 + log10(1)/7 * 0.3 + 0 * 0.2
        assert!(score > 0.2 && score < 0.3, "score={score}");
    }

    #[test]
    fn convergence_store_basics() {
        let mut store = ConvergenceStore::new(600);
        assert_eq!(store.active_count(), 0);
        assert!(store.get_convergence("test").is_none());
        assert!((store.convergence_boost("test") - 0.0).abs() < f64::EPSILON);

        store.active_signals.push(ConvergenceSignal {
            market: "test-market".into(),
            wallets: vec![],
            convergence_score: 0.8,
            net_direction: "buy".into(),
            total_usd: 50_000.0,
            detected_at: Instant::now(),
        });

        assert_eq!(store.active_count(), 1);
        assert!(store.get_convergence("test-market").is_some());
        let boost = store.convergence_boost("test-market");
        assert!((boost - 0.04).abs() < f64::EPSILON, "boost={boost}"); // 0.8 * 0.05
    }

    #[test]
    fn config_defaults() {
        let config = SmartMoneyConfig::from_env();
        assert!(!config.enabled);
        assert_eq!(config.poll_secs, 60);
        assert_eq!(config.convergence_threshold, 3);
        assert!((config.min_trust_score - 0.60).abs() < f64::EPSILON);
    }

    #[test]
    fn convergence_summary() {
        let store = ConvergenceStore::new(600);
        let summary = store.summary();
        assert_eq!(summary.active_signals, 0);
        assert_eq!(summary.tracked_markets, 0);
        assert!(summary.top_signal.is_none());
    }
}
