//! `Rn1RestSource` — legacy-compatible poller for the Polymarket public
//! data API (`https://data-api.polymarket.com/activity?user=...`).
//!
//! This source is a drop-in replacement for the shape of
//! `engine::rn1_poller` but emits typed [`RawEvent`]s into the ingress
//! ring instead of the old `crossbeam-channel` `RN1Signal` stream. It is
//! **scheduled for retirement** once `MempoolSource` has shadow-mode
//! parity (plan §3 Phase 2, `p2-retire`). Treat it as legacy-path only.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use blink_rings::Producer;
use blink_timestamps::Timestamp;
use blink_types::{PriceTicks, RawEvent, Side, SizeU, SourceKind, wall_clock_ns, OnChainAnchor, EventId};
use serde::Deserialize;

use crate::{Source, SourceCounters, ShutdownToken, try_push};

/// Configuration for [`Rn1RestSource`].
#[derive(Debug, Clone)]
pub struct Rn1RestConfig {
    /// Wallet to poll (0x-prefixed, mixed-case OK — echoed into the URL).
    pub wallet: String,
    /// Base data API URL (override for tests / staging). Defaults to
    /// `https://data-api.polymarket.com`.
    pub base_url: String,
    /// Interval between polls.
    pub poll_interval: Duration,
    /// `limit` query parameter — entries per poll.
    pub limit: u32,
    /// Per-request HTTP timeout.
    pub request_timeout: Duration,
}

impl Default for Rn1RestConfig {
    fn default() -> Self {
        Self {
            wallet: String::new(),
            base_url: "https://data-api.polymarket.com".to_string(),
            poll_interval: Duration::from_millis(400),
            limit: 20,
            request_timeout: Duration::from_secs(5),
        }
    }
}

/// Polymarket data-api `/activity` entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    /// Transaction hash (0x-prefixed, 66 chars).
    #[serde(default)]
    pub transaction_hash: Option<String>,
    /// Condition id of the market.
    #[serde(default)]
    pub condition_id: Option<String>,
    /// Token id (asset).
    #[serde(default)]
    pub asset: Option<String>,
    /// `"BUY"` / `"SELL"`.
    #[serde(default)]
    pub side: Option<String>,
    /// Execution price, e.g. `0.52`.
    #[serde(default)]
    pub price: Option<f64>,
    /// Token size.
    #[serde(default)]
    pub size: Option<f64>,
    /// Epoch seconds.
    #[serde(default)]
    #[allow(dead_code)]
    pub timestamp: Option<i64>,
    /// e.g. `"TRADE"`.
    #[serde(default, rename = "type")]
    pub entry_type: Option<String>,
}

/// The concrete RN1 REST source.
pub struct Rn1RestSource {
    cfg: Rn1RestConfig,
    counters: Arc<SourceCounters>,
}

impl Rn1RestSource {
    /// Construct.
    pub fn new(cfg: Rn1RestConfig) -> Self {
        Self {
            cfg,
            counters: SourceCounters::new(),
        }
    }
}

impl Source for Rn1RestSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Rn1Rest
    }

    fn stats_handle(&self) -> Arc<SourceCounters> {
        self.counters.clone()
    }

    fn run(self: Box<Self>, mut sink: Producer<RawEvent>, shutdown: ShutdownToken) {
        let _ = blink_timestamps::init_with_policy(blink_timestamps::InitPolicy::AllowFallback);
        let counters = self.counters.clone();
        let cfg = self.cfg.clone();
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                log::error!("rn1-rest: cannot build tokio runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            let client = match reqwest::Client::builder()
                .timeout(cfg.request_timeout)
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    log::error!("rn1-rest: client build failed: {e}");
                    return;
                }
            };
            let url = format!(
                "{}/activity?user={}&limit={}",
                cfg.base_url.trim_end_matches('/'),
                cfg.wallet,
                cfg.limit
            );
            let mut seen: HashSet<String> = HashSet::with_capacity(256);
            let mut first_poll = true;

            while !shutdown.is_cancelled() {
                match fetch_once(&client, &url).await {
                    Ok(entries) => {
                        for ev in parse_activity(&entries, &mut seen, first_poll) {
                            try_push(&mut sink, &counters, ev);
                        }
                        first_poll = false;
                    }
                    Err(e) => {
                        counters
                            .reconnects
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        log::warn!("rn1-rest: poll failed: {e}");
                    }
                }
                tokio::select! {
                    _ = tokio::time::sleep(cfg.poll_interval) => {}
                    _ = shutdown.cancelled() => break,
                }
            }
        });
    }
}

async fn fetch_once(client: &reqwest::Client, url: &str) -> Result<Vec<ActivityEntry>, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice::<Vec<ActivityEntry>>(&body).map_err(|e| e.to_string())
}

/// Pure parse — unit-testable without I/O. Converts a batch of activity
/// entries into `RawEvent`s, using `seen` as the tx-hash dedup set. On
/// `is_first_poll` we seed the dedup set but emit nothing (matches the
/// legacy poller's cold-start behaviour so we don't fire on historical
/// trades).
pub fn parse_activity(
    entries: &[ActivityEntry],
    seen: &mut HashSet<String>,
    is_first_poll: bool,
) -> Vec<RawEvent> {
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let hash = match &e.transaction_hash {
            Some(h) => h.clone(),
            None => continue,
        };
        if e.entry_type.as_deref() != Some("TRADE") {
            continue;
        }
        if !seen.insert(hash.clone()) {
            continue;
        }
        if is_first_poll {
            continue;
        }
        let side = match e.side.as_deref().map(str::to_ascii_uppercase).as_deref() {
            Some("BUY") => Side::Buy,
            Some("SELL") => Side::Sell,
            _ => continue,
        };
        let price_ticks = e.price.map(|p| (p * 1000.0) as u64).unwrap_or(0);
        let size_u = e.size.map(|s| (s * 1000.0) as u64).unwrap_or(0);
        let tx_bytes = parse_tx_hash(&hash);
        let anchor = tx_bytes.map(|h| OnChainAnchor {
            tx_hash: h,
            log_index: u32::MAX,
        });
        out.push(RawEvent {
            event_id: EventId::fetch_next(),
            source: SourceKind::Rn1Rest,
            source_seq: u64::MAX,
            anchor,
            token_id: e.asset.clone().unwrap_or_default(),
            market_id: e.condition_id.clone(),
            side: Some(side),
            price: Some(PriceTicks(price_ticks)),
            size: Some(SizeU(size_u)),
            tsc_in: Timestamp::now(),
            wall_ns: wall_clock_ns(),
            extra: None,
            observe_only: false,
            maker_wallet: None,
        });
    }
    out
}

fn parse_tx_hash(s: &str) -> Option<[u8; 32]> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&stripped[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blink_timestamps::{init_with_policy, InitPolicy};

    fn init_ts() {
        let _ = init_with_policy(InitPolicy::AllowFallback);
    }

    const FIXTURE: &str = r#"[
      {
        "transactionHash": "0x1111111111111111111111111111111111111111111111111111111111111111",
        "conditionId": "0xcond1",
        "asset": "0xtoken1",
        "side": "BUY",
        "price": 0.52,
        "size": 100.0,
        "timestamp": 1700000000,
        "type": "TRADE"
      },
      {
        "transactionHash": "0x2222222222222222222222222222222222222222222222222222222222222222",
        "conditionId": "0xcond1",
        "asset": "0xtoken1",
        "side": "SELL",
        "price": 0.48,
        "size": 50.0,
        "type": "TRADE"
      },
      {
        "transactionHash": "0x3333333333333333333333333333333333333333333333333333333333333333",
        "type": "SPLIT"
      }
    ]"#;

    #[test]
    fn parses_fixture_into_two_raw_events() {
        init_ts();
        let entries: Vec<ActivityEntry> = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(entries.len(), 3);

        // First poll seeds dedup, emits nothing.
        let mut seen = HashSet::new();
        let first = parse_activity(&entries, &mut seen, true);
        assert!(first.is_empty(), "first poll is cold-start seed");
        assert_eq!(seen.len(), 2, "only TRADE entries recorded in seen");

        // New batch → already-seen, no emission.
        let again = parse_activity(&entries, &mut seen, false);
        assert!(again.is_empty());

        // Fresh seen → second non-first poll emits both trades.
        let mut seen2 = HashSet::new();
        let events = parse_activity(&entries, &mut seen2, false);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].source, SourceKind::Rn1Rest);
        assert_eq!(events[0].side, Some(Side::Buy));
        assert_eq!(events[0].price, Some(PriceTicks(520)));
        assert_eq!(events[0].size, Some(SizeU(100_000)));
        assert!(events[0].anchor.is_some());
        assert_eq!(events[0].anchor.unwrap().log_index, u32::MAX);
        assert_eq!(events[1].side, Some(Side::Sell));
        assert_eq!(events[1].price, Some(PriceTicks(480)));
        assert!(!events[0].observe_only);
    }

    #[test]
    fn non_trade_and_unknown_side_are_dropped() {
        init_ts();
        let raw = r#"[
          {"transactionHash":"0xaa00000000000000000000000000000000000000000000000000000000000000","type":"TRADE","side":"WAT","price":0.5,"size":1.0,"asset":"x"}
        ]"#;
        let entries: Vec<ActivityEntry> = serde_json::from_str(raw).unwrap();
        let mut seen = HashSet::new();
        let events = parse_activity(&entries, &mut seen, false);
        assert!(events.is_empty(), "unknown side is skipped");
    }

    #[test]
    fn tx_hash_decode() {
        let ok = parse_tx_hash("0x1111111111111111111111111111111111111111111111111111111111111111");
        assert!(ok.is_some());
        assert!(parse_tx_hash("0xshort").is_none());
    }
}
