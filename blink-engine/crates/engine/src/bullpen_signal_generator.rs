//! bullpen_signal_generator.rs — Generates RN1Signals from Bullpen smart-money convergence data.
//!
//! Reads the DiscoveryStore (markets resolving within N hours) and the ConvergenceStore
//! (multi-wallet smart-money signals) every `interval_secs` and synthesises `RN1Signal`
//! values that flow through the normal signal → risk → order pipeline.
//!
//! Signal-hierarchy per market:
//!   1. Smart money (primary): ConvergenceStore has a direction → emit RN1Signal directly.
//!   2. No SM direction: market appears in `GET /api/bullpen/short_markets` for the AI sidecar.
//!
//! **Latency class: COLD PATH.  Never call from the signal → order hot path.**

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::bullpen_discovery::DiscoveryStore;
use crate::bullpen_smart_money::ConvergenceStore;
use crate::order_book::OrderBookStore;
use crate::types::{OrderSide, RN1Signal};

// ─── Configuration ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SignalGenConfig {
    pub enabled: bool,
    /// How often to run the generation loop (seconds).
    pub interval_secs: u64,
    /// Only generate signals for markets resolving within this many hours.
    pub max_resolve_hours: u64,
    /// Minimum convergence_score needed to emit a signal (0.0–1.0).
    pub min_convergence_score: f64,
    /// Minimum synthetic RN1 notional (USD) when `total_usd` from SM is very small.
    pub min_synthetic_notional_usd: f64,
}

impl SignalGenConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("BULLPEN_SIGNAL_GEN_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            interval_secs: std::env::var("BULLPEN_SIGNAL_GEN_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            max_resolve_hours: std::env::var("BULLPEN_DISCOVER_MAX_RESOLVE_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(6),
            min_convergence_score: std::env::var("BULLPEN_SIGNAL_GEN_MIN_CONVERGENCE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.3),
            min_synthetic_notional_usd: std::env::var("BULLPEN_SIGNAL_GEN_MIN_NOTIONAL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(500.0),
        }
    }
}

// ─── Signal Generator ─────────────────────────────────────────────────────────

pub struct BullpenSignalGenerator {
    discovery_store: Arc<RwLock<DiscoveryStore>>,
    convergence_store: Arc<RwLock<ConvergenceStore>>,
    book_store: Arc<OrderBookStore>,
    signal_tx: tokio::sync::mpsc::Sender<RN1Signal>,
    market_subscriptions: Arc<Mutex<Vec<String>>>,
    force_reconnect: Arc<AtomicBool>,
    config: SignalGenConfig,
    /// Tracks order_ids emitted this session to prevent re-emission on every cycle.
    emitted_ids: HashSet<String>,
    scan_cycle: u64,
}

impl BullpenSignalGenerator {
    pub fn new(
        discovery_store: Arc<RwLock<DiscoveryStore>>,
        convergence_store: Arc<RwLock<ConvergenceStore>>,
        book_store: Arc<OrderBookStore>,
        signal_tx: tokio::sync::mpsc::Sender<RN1Signal>,
        market_subscriptions: Arc<Mutex<Vec<String>>>,
        force_reconnect: Arc<AtomicBool>,
        config: SignalGenConfig,
    ) -> Self {
        Self {
            discovery_store,
            convergence_store,
            book_store,
            signal_tx,
            market_subscriptions,
            force_reconnect,
            config,
            emitted_ids: HashSet::with_capacity(512),
            scan_cycle: 0,
        }
    }

    /// Background loop. Spawn with `tokio::spawn(generator.run(shutdown))`.
    pub async fn run(mut self, shutdown: Arc<AtomicBool>) {
        if !self.config.enabled {
            info!("Bullpen signal generator disabled (BULLPEN_SIGNAL_GEN_ENABLED not set)");
            return;
        }

        let interval = std::time::Duration::from_secs(self.config.interval_secs);
        let mut ticker = tokio::time::interval(interval);

        info!(
            interval_secs = self.config.interval_secs,
            max_resolve_hours = self.config.max_resolve_hours,
            min_convergence_score = self.config.min_convergence_score,
            "Bullpen signal generator started"
        );

        loop {
            ticker.tick().await;
            if shutdown.load(Ordering::Relaxed) {
                info!("Bullpen signal generator shutting down");
                break;
            }
            self.run_cycle().await;
        }
    }

    async fn run_cycle(&mut self) {
        self.scan_cycle += 1;

        // Step 1: Collect short-term markets and any token_ids not yet subscribed to WS.
        let (short_markets, new_token_ids) = {
            let store = self.discovery_store.read().await;
            let subs = self.market_subscriptions.lock().unwrap();
            let mut new_ids: Vec<String> = Vec::new();

            let markets: Vec<(String, Option<String>, Option<i64>)> = store
                .short_term_markets(self.config.max_resolve_hours)
                .into_iter()
                .map(|m| {
                    if !subs.contains(&m.token_id) {
                        new_ids.push(m.token_id.clone());
                    }
                    (m.token_id.clone(), m.title.clone(), m.ends_at_ts)
                })
                .collect();

            (markets, new_ids)
        };

        // Step 2: Subscribe any newly discovered token_ids to the WS feed.
        if !new_token_ids.is_empty() {
            self.subscribe_tokens(&new_token_ids);
            info!(
                count = new_token_ids.len(),
                "BullpenSignalGen: subscribing new short-term market tokens to WS"
            );
        }

        if short_markets.is_empty() {
            debug!(
                scan = self.scan_cycle,
                max_hours = self.config.max_resolve_hours,
                "BullpenSignalGen: no markets with known end_date within window"
            );
            return;
        }

        // Step 3: Read convergence signals (snapshot — released before signal send).
        let convergence_signals = {
            let store = self.convergence_store.read().await;
            store.active_signals.clone()
        };

        let mut generated = 0usize;

        for (token_id, title, ends_at_ts) in &short_markets {
            // Try to match a convergence signal by fuzzy title search.
            let conv = title.as_deref().and_then(|t| {
                let tl = t.to_lowercase();
                convergence_signals.iter().find(|s| {
                    let sl = s.market.to_lowercase();
                    // Match if either string contains the other (handles slug vs full title).
                    sl.contains(&tl) || tl.contains(&sl)
                })
            });

            let conv = match conv {
                Some(c) if c.convergence_score >= self.config.min_convergence_score => c,
                _ => continue, // No SM signal strong enough → skip (alpha sidecar covers this)
            };

            let direction = conv.net_direction.to_lowercase();
            // Dedup key includes the scan_cycle so we can re-signal on a new cycle if
            // the direction changes. But within the same scan_cycle we won't repeat.
            let order_id = format!("sm:{}:{}:{}", token_id, direction, self.scan_cycle);
            if self.emitted_ids.contains(&order_id) {
                continue;
            }

            let side = if direction == "buy" {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };

            // Use WS order-book midpoint (×1000 scaled), fall back to 500 (0.50).
            let price_scaled = self.book_store.get_mid_price(token_id).unwrap_or(500);
            let price_f64 = price_scaled as f64 / 1_000.0;

            // Derive synthetic shares from SM total_usd (clamped to min_synthetic_notional).
            let notional = conv.total_usd.max(self.config.min_synthetic_notional_usd);
            let shares = notional / price_f64.max(0.001);
            let size_scaled = (shares * 1_000.0) as u64;

            let signal = RN1Signal {
                token_id: token_id.clone(),
                market_title: title.clone(),
                market_outcome: None,
                side,
                price: price_scaled,
                size: size_scaled,
                order_id: order_id.clone(),
                detected_at: Instant::now(),
                event_start_time: None,
                event_end_time: *ends_at_ts,
                source_wallet: "bullpen_sm".to_string(),
                wallet_weight: conv.convergence_score.clamp(0.0, 1.0),
                signal_source: "rn1".to_string(),
                analysis_id: None,
                intent_id: crate::types::next_intent_id(),
                market_id: None, // TODO: hydrate market_id from bullpen discovery data
                source_order_id: None,
                source_seq: None,
                enqueued_at: Instant::now(),
            };

            match self.signal_tx.send(signal).await {
                Ok(()) => {
                    self.emitted_ids.insert(order_id);
                    generated += 1;
                    info!(
                        token_id = %token_id,
                        direction = %direction,
                        score = conv.convergence_score,
                        price = price_f64,
                        ends_at_ts,
                        "BullpenSignalGen: SM signal generated for short-term market"
                    );
                }
                Err(e) => {
                    warn!(err = %e, "BullpenSignalGen: signal channel closed — stopping");
                    return;
                }
            }
        }

        if generated > 0 || self.scan_cycle.is_multiple_of(10) {
            info!(
                scan = self.scan_cycle,
                short_markets = short_markets.len(),
                convergence_checked = convergence_signals.len(),
                generated,
                "BullpenSignalGen cycle complete"
            );
        }

        // Prune emitted_ids to avoid unbounded growth.
        if self.emitted_ids.len() > 2_000 {
            self.emitted_ids.clear();
        }
    }

    /// Add `token_ids` to the WS subscription list and trigger a WS reconnect.
    fn subscribe_tokens(&self, token_ids: &[String]) {
        let mut subs = self.market_subscriptions.lock().unwrap();
        let mut added = false;
        for id in token_ids {
            if !subs.contains(id) {
                subs.push(id.clone());
                added = true;
            }
        }
        if added {
            self.force_reconnect.store(true, Ordering::Relaxed);
        }
    }
}
