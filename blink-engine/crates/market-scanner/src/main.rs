//! Market scanner — discover and select Polymarket CLOB markets.
//!
//! Queries the Polymarket Gamma REST API for all active markets, filters for
//! sports-related events using keyword tag matching, and ranks results by 24-hour
//! trading volume.
//!
//! # Usage
//! ```bash
//! cargo run -p market-scanner
//! ```
//!
//! The tool prints two sorted tables:
//! 1. Top sports markets (up to 20) with YES token IDs
//! 2. Top 20 general markets across all categories
//!
//! It then offers to auto-update the `MARKETS=` line in your `.env` file with the
//! top sports token IDs, or the top general markets if no sports markets are found.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};

const BASE_URL: &str = "https://gamma-api.polymarket.com";
const TOP_SPORTS_FOR_ENV: usize = 20;

const SPORTS_KEYWORDS: &[&str] = &[
    "soccer",
    "football",
    "basketball",
    "nba",
    "nfl",
    "tennis",
    "baseball",
    "mlb",
    "cricket",
    "esports",
    "mma",
    "ufc",
    "boxing",
    "hockey",
    "nhl",
    "golf",
    "rugby",
];

// ─── Serde helpers ───────────────────────────────────────────────────────────

fn de_str_or_f64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StrOrF64 {
        F(f64),
        S(String),
    }
    let v: Option<StrOrF64> = Option::deserialize(d)?;
    Ok(v.and_then(|x| match x {
        StrOrF64::F(f) => Some(f),
        StrOrF64::S(s) => s.parse().ok(),
    }))
}

// ─── API Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Tag {
    id: String,
    label: Option<String>,
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GammaEvent {
    id: Option<String>,
    title: Option<String>,
    #[serde(rename = "volume24hr", deserialize_with = "de_str_or_f64", default)]
    volume_24hr: Option<f64>,
    #[serde(default)]
    markets: Vec<GammaMarket>,
    tags: Option<Vec<EventTag>>,
}

#[derive(Debug, Deserialize)]
struct EventTag {
    id: String,
    label: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct GammaMarket {
    #[serde(rename = "clobTokenIds")]
    clob_token_ids: Option<String>,
    outcomes: Option<String>,
    #[serde(rename = "orderPriceMinTickSize")]
    tick_size: Option<f64>,
    #[serde(deserialize_with = "de_str_or_f64", default)]
    volume: Option<f64>,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    closed: bool,
    #[serde(rename = "negRisk", default)]
    neg_risk: bool,
}

impl GammaMarket {
    fn token_ids(&self) -> Vec<String> {
        self.clob_token_ids
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default()
    }

    fn outcome_labels(&self) -> Vec<String> {
        self.outcomes
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default()
    }

    /// Return YES token ID (first token whose outcome label is "yes", or the first token).
    fn yes_token(&self) -> Option<String> {
        let ids = self.token_ids();
        let labels = self.outcome_labels();
        labels
            .iter()
            .zip(ids.iter())
            .find(|(l, _)| l.to_lowercase() == "yes")
            .map(|(_, id)| id.clone())
            .or_else(|| ids.into_iter().next())
    }
}

// ─── Display helpers ─────────────────────────────────────────────────────────

fn fmt_dollars(value: f64) -> String {
    let rounded = value as u64;
    let s = rounded.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

fn trunc(s: &str, len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > len {
        format!("{}…", chars[..len].iter().collect::<String>())
    } else {
        s.to_string()
    }
}

// ─── API calls ───────────────────────────────────────────────────────────────

fn fetch_tags(client: &reqwest::blocking::Client) -> Result<Vec<Tag>> {
    let url = format!("{BASE_URL}/tags");
    let text = client.get(&url).send().context("GET /tags")?.text()?;
    serde_json::from_str(&text).context("parse /tags")
}

fn fetch_events_for_tag(
    client: &reqwest::blocking::Client,
    tag_id: &str,
) -> Result<Vec<GammaEvent>> {
    let url = format!("{BASE_URL}/events?active=true&closed=false&tag_id={tag_id}&limit=20");
    let text = client
        .get(&url)
        .send()
        .context("GET /events?tag_id")?
        .text()?;
    serde_json::from_str(&text).context("parse /events?tag_id")
}

fn fetch_top_events(client: &reqwest::blocking::Client) -> Result<Vec<GammaEvent>> {
    let url = format!("{BASE_URL}/events?active=true&closed=false&limit=50");
    let text = client.get(&url).send().context("GET /events")?.text()?;
    serde_json::from_str(&text).context("parse /events")
}

// ─── Helper: find sports label for an event ──────────────────────────────────

fn sport_for_event(event: &GammaEvent, sport_tag_map: &HashMap<String, String>) -> String {
    event
        .tags
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find_map(|t| sport_tag_map.get(&t.id).cloned())
        .unwrap_or_default()
}

#[allow(dead_code)]
fn is_sports_event(event: &GammaEvent, sport_ids: &HashSet<String>) -> bool {
    event
        .tags
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .any(|t| sport_ids.contains(&t.id))
}

// ─── Row types ───────────────────────────────────────────────────────────────

struct SportsRow {
    volume: f64,
    event_title: String,
    yes_token: String,
    tick: f64,
    neg_risk: bool,
    sport: String,
}

struct AllRow {
    volume: f64,
    event_title: String,
    yes_token: String,
    tick: f64,
    tag: String,
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    let client = reqwest::blocking::Client::builder()
        .user_agent("blink-market-scanner/0.2")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let border = "═".repeat(44);

    // ── Step 1: Fetch all tags and filter for sports ──────────────────────────
    println!("Fetching tags from Gamma API…");
    let all_tags = fetch_tags(&client).unwrap_or_else(|e| {
        eprintln!("Warning: could not fetch /tags: {e}");
        vec![]
    });

    let sport_ids: HashSet<String> = all_tags
        .iter()
        .filter(|t| {
            let label = t.label.as_deref().unwrap_or("").to_lowercase();
            let slug = t.slug.as_deref().unwrap_or("").to_lowercase();
            SPORTS_KEYWORDS
                .iter()
                .any(|kw| label.contains(kw) || slug.contains(kw))
        })
        .map(|t| t.id.clone())
        .collect();

    // Map: tag_id → label (for sports tags only)
    let sport_tag_map: HashMap<String, String> = all_tags
        .iter()
        .filter(|t| sport_ids.contains(&t.id))
        .map(|t| {
            (
                t.id.clone(),
                t.label.clone().unwrap_or_else(|| t.id.clone()),
            )
        })
        .collect();

    let sport_names: Vec<&str> = all_tags
        .iter()
        .filter(|t| sport_ids.contains(&t.id))
        .filter_map(|t| t.label.as_deref())
        .collect();

    println!(
        "Found {} sport tag(s): {}",
        sport_ids.len(),
        sport_names.join(", ")
    );

    // ── Step 2: Fetch events for each sport tag ───────────────────────────────
    let mut sports_events: Vec<GammaEvent> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    for tid in &sport_ids {
        let label = sport_tag_map.get(tid).map(|s| s.as_str()).unwrap_or(tid);
        println!("  Fetching events for '{label}' (id={tid})…");
        match fetch_events_for_tag(&client, tid) {
            Ok(evts) => {
                for evt in evts {
                    let id = evt.id.clone().unwrap_or_default();
                    if seen_ids.insert(id) {
                        sports_events.push(evt);
                    }
                }
            }
            Err(e) => eprintln!("  Warning: failed tag {tid}: {e}"),
        }
    }

    // ── Step 3: Fetch top-50 events (all categories) ──────────────────────────
    println!("Fetching top-50 all-category events…");
    let top_events = fetch_top_events(&client).unwrap_or_else(|e| {
        eprintln!("Warning: could not fetch top events: {e}");
        vec![]
    });

    println!();

    // ── Build sports rows ─────────────────────────────────────────────────────
    let mut sports_rows: Vec<SportsRow> = Vec::new();
    for event in &sports_events {
        let event_vol = event.volume_24hr.unwrap_or(0.0);
        let sport = sport_for_event(event, &sport_tag_map);
        for market in &event.markets {
            if !market.active || market.closed {
                continue;
            }
            let Some(yes_token) = market.yes_token() else {
                continue;
            };
            sports_rows.push(SportsRow {
                volume: market.volume.unwrap_or(event_vol),
                event_title: event.title.clone().unwrap_or_default(),
                yes_token,
                tick: market.tick_size.unwrap_or(0.01),
                neg_risk: market.neg_risk,
                sport: sport.clone(),
            });
        }
    }
    sports_rows.sort_by(|a, b| {
        b.volume
            .partial_cmp(&a.volume)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // ── Build all-markets rows (top_events) ───────────────────────────────────
    let mut all_rows: Vec<AllRow> = Vec::new();
    for event in &top_events {
        let event_vol = event.volume_24hr.unwrap_or(0.0);
        let tag_label = event
            .tags
            .as_deref()
            .unwrap_or(&[])
            .first()
            .and_then(|t| t.label.as_deref())
            .unwrap_or("—")
            .to_string();
        for market in &event.markets {
            if !market.active || market.closed {
                continue;
            }
            let Some(yes_token) = market.yes_token() else {
                continue;
            };
            all_rows.push(AllRow {
                volume: market.volume.unwrap_or(event_vol),
                event_title: event.title.clone().unwrap_or_default(),
                yes_token,
                tick: market.tick_size.unwrap_or(0.01),
                tag: tag_label.clone(),
            });
        }
    }
    all_rows.sort_by(|a, b| {
        b.volume
            .partial_cmp(&a.volume)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // ── Section 1: Top Sports Markets ─────────────────────────────────────────
    println!("{border}");
    println!("  🏆 TOP SPORTS MARKETS (by 24h volume)");
    println!("{border}");
    if sports_rows.is_empty() {
        println!("  ⚠️  No active sports markets found via tag filtering.");
        println!("      Sports tags returned no active markets right now.");
        println!("      Consider checking top markets below for manual selection.");
    } else {
        println!(
            " {:<3} {:<22} {:>12}  {:<10} {:>5}",
            "#", "Token ID", "Volume 24h", "Sport", "Tick"
        );
        println!(" {}", "─".repeat(58));
        for (i, row) in sports_rows.iter().enumerate().take(20) {
            let neg = if row.neg_risk { " ⚠️ negRisk" } else { "" };
            println!(
                " {:<3} {:<22} {:>12}  {:<10} {:>5}{neg}",
                i + 1,
                trunc(&row.yes_token, 20),
                format!("${}", fmt_dollars(row.volume)),
                trunc(&row.sport, 10),
                format!("{:.2}", row.tick),
            );
            println!("     └─ {}", trunc(&row.event_title, 55));
        }
    }
    println!();

    // ── Section 2: All Top Markets ────────────────────────────────────────────
    println!("{border}");
    println!("  📊 ALL TOP MARKETS (including non-sports)");
    println!("{border}");
    if all_rows.is_empty() {
        println!("  No markets found.");
    } else {
        println!(
            " {:<3} {:<22} {:>12}  {:<14} {:>5}",
            "#", "Token ID", "Volume 24h", "Tag", "Tick"
        );
        println!(" {}", "─".repeat(62));
        for (i, row) in all_rows.iter().enumerate().take(20) {
            println!(
                " {:<3} {:<22} {:>12}  {:<14} {:>5}",
                i + 1,
                trunc(&row.yes_token, 20),
                format!("${}", fmt_dollars(row.volume)),
                trunc(&row.tag, 14),
                format!("{:.2}", row.tick),
            );
            println!("     └─ {}", trunc(&row.event_title, 58));
        }
    }
    println!();

    // ── Section 3: Suggested MARKETS= line ───────────────────────────────────
    let (env_line, label) = if !sports_rows.is_empty() {
        let tokens: Vec<&str> = sports_rows
            .iter()
            .take(TOP_SPORTS_FOR_ENV)
            .map(|r| r.yes_token.as_str())
            .collect();
        (tokens.join(","), "sports only")
    } else {
        eprintln!("⚠️  No sports markets found — falling back to all top markets.");
        let tokens: Vec<&str> = all_rows
            .iter()
            .take(TOP_SPORTS_FOR_ENV)
            .map(|r| r.yes_token.as_str())
            .collect();
        (tokens.join(","), "⚠️  all markets (no sports found)")
    };

    println!("{border}");
    println!("  🔧 SUGGESTED .env MARKETS= ({label})");
    println!("{border}");
    if env_line.is_empty() {
        println!("  (no token IDs available)");
    } else {
        println!("MARKETS={env_line}");
    }
    println!();

    // ── Interactive .env update ───────────────────────────────────────────────
    if env_line.is_empty() {
        println!("Nothing to write — exiting.");
        return Ok(());
    }

    print!("Update .env? (y/N): ");
    io::stdout().flush()?;
    let answer = io::stdin()
        .lock()
        .lines()
        .next()
        .and_then(|l| l.ok())
        .unwrap_or_default();

    if answer.trim().eq_ignore_ascii_case("y") {
        // Resolve .env relative to the current working directory so this
        // binary works regardless of where the workspace is checked out.
        let env_path = std::env::current_dir()
            .context("cannot determine current directory")?
            .join(".env");
        let content = std::fs::read_to_string(&env_path)
            .with_context(|| format!("read .env at {}", env_path.display()))?;
        let updated: String = if content.contains("MARKETS=") {
            content
                .lines()
                .map(|l| {
                    if l.starts_with("MARKETS=") {
                        format!("MARKETS={env_line}")
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            format!("{content}\nMARKETS={env_line}\n")
        };
        std::fs::write(&env_path, updated)
            .with_context(|| format!("write .env at {}", env_path.display()))?;
        println!(
            "✅ .env updated with {} sports token ID(s).",
            env_line.split(',').count()
        );
    } else {
        println!("No changes made.");
    }

    Ok(())
}
