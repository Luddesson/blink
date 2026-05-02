//! Game-start watcher — monitors active markets for the pre-game to in-play transition.
//!
//! For each market token in our watch list, this task periodically polls the CLOB
//! GET /price endpoint. If a market suddenly reports a price that is unavailable
//! (the market has gone in-play or is paused), it fires a `GameStartSignal` that
//! the engine uses to flush all open orders for that market.
//!
//! Polling interval: 500ms (configurable via GAME_WATCHER_INTERVAL_MS env var).
//! Detection: if GET /price returns an error OR both sides return zero/unparseable,
//! treat as game-start.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::clob_client::ClobClient;
use crate::config::Config;
use crate::types::OrderSide;

// ─── GameStartSignal ─────────────────────────────────────────────────────────

/// Emitted when a watched market is detected as going in-play or paused.
#[derive(Debug, Clone)]
pub struct GameStartSignal {
    pub token_id: String,
    pub detected_at: Instant,
}

// ─── GameStartWatcher ────────────────────────────────────────────────────────

/// Polls CLOB prices for each watched market and fires a [`GameStartSignal`]
/// the first time a market looks in-play.
pub struct GameStartWatcher {
    token_ids: Vec<String>,
    clob_client: Arc<ClobClient>,
    tx: broadcast::Sender<GameStartSignal>,
    interval_ms: u64,
}

impl GameStartWatcher {
    /// Create a watcher from the engine config.
    ///
    /// `tx` is the broadcast sender — callers subscribe to it to receive signals.
    pub fn new(
        config: &Config,
        clob_client: Arc<ClobClient>,
        tx: broadcast::Sender<GameStartSignal>,
    ) -> Self {
        let interval_ms = std::env::var("GAME_WATCHER_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500_u64);

        Self {
            token_ids: config.markets.clone(),
            clob_client,
            tx,
            interval_ms,
        }
    }

    /// Runs the poll loop forever (intended to be `tokio::spawn`-ed).
    ///
    /// Fires at most one [`GameStartSignal`] per market over the lifetime of
    /// this task.  Stops gracefully when all receivers are dropped (the
    /// broadcast channel closes).
    pub async fn run(self) {
        if self.token_ids.is_empty() {
            info!("GameStartWatcher: no markets to watch — task exiting");
            return;
        }

        let mut fired: HashSet<String> = HashSet::new();
        let interval = Duration::from_millis(self.interval_ms);

        info!(
            markets = self.token_ids.len(),
            interval_ms = self.interval_ms,
            "GameStartWatcher started"
        );

        loop {
            tokio::time::sleep(interval).await;

            // If all receivers have been dropped the channel is closed; stop
            // polling to avoid burning CPU with orphaned work.
            if self.tx.receiver_count() == 0 {
                info!("GameStartWatcher: no receivers — shutting down");
                break;
            }

            for token_id in &self.token_ids {
                if fired.contains(token_id) {
                    continue;
                }

                let buy_result = self.clob_client.get_price(token_id, OrderSide::Buy).await;
                let sell_result = self.clob_client.get_price(token_id, OrderSide::Sell).await;

                let game_started = match (&buy_result, &sell_result) {
                    // Any HTTP/network error → treat as market paused / in-play.
                    (Err(_), _) | (_, Err(_)) => true,
                    // Both sides returned successfully; check if prices are zero.
                    (Ok(buy_str), Ok(sell_str)) => {
                        let buy: f64 = buy_str.parse().unwrap_or(0.0);
                        let sell: f64 = sell_str.parse().unwrap_or(0.0);
                        buy == 0.0 && sell == 0.0
                    }
                };

                if game_started {
                    warn!(
                        token_id = %token_id,
                        "GAME START detected for token {token_id} — firing order wipe"
                    );
                    fired.insert(token_id.clone());
                    let signal = GameStartSignal {
                        token_id: token_id.clone(),
                        detected_at: Instant::now(),
                    };
                    // A send error just means no active receivers; log and move on.
                    if let Err(e) = self.tx.send(signal) {
                        warn!(token_id = %token_id, "GameStartSignal send failed: {e}");
                    }
                }
            }
        }
    }
}
