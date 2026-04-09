//! `blink market` — discover, search, and inspect Polymarket markets.
//!
//! Data sources:
//!   - Polymarket Gamma API  (discovery, event details, price history)
//!   - Polymarket CLOB API   (real-time prices, order books, recent trades)

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use comfy_table::{Table, Cell, Color, Attribute, ContentArrangement};
use indicatif::{ProgressBar, ProgressStyle};

use crate::{client::CliContext, OutputFormat};

#[derive(Args)]
pub struct MarketArgs {
    #[command(subcommand)]
    pub sub: MarketCmd,
}

#[derive(Subcommand)]
pub enum MarketCmd {
    /// Discover trending prediction markets.
    Discover {
        /// Filter lens: all | sports | crypto | politics | geo | ending-soon.
        #[arg(default_value = "all")]
        lens: String,

        /// Text search filter.
        #[arg(long)]
        search: Option<String>,

        /// Filter by minimum 24h volume (USD).
        #[arg(long)]
        min_volume: Option<f64>,

        /// Filter by minimum liquidity (USD).
        #[arg(long)]
        min_liquidity: Option<f64>,

        /// Sort: volume | volume_24h | liquidity | newest | ending-soon.
        #[arg(long, default_value = "volume_24h")]
        sort: String,

        /// Max results (default 20).
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Show real-time bid/ask/mid/spread for a market token.
    Price {
        /// Token ID (CLOB) or market slug.
        token_id: String,
    },

    /// Order book snapshot for a market token.
    Book {
        /// Token ID.
        token_id: String,
    },

    /// Recent trades on a market.
    Trades {
        /// Token ID.
        token_id: String,

        /// Max results (default 20).
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Price history for a market token (from CLOB).
    History {
        /// Token ID.
        token_id: String,

        /// Interval: 1m | 1h | 6h | 1d | 1w.
        #[arg(long, default_value = "1d")]
        interval: String,

        /// Fidelity (number of data points).
        #[arg(long, default_value = "50")]
        fidelity: u32,
    },

    /// Search markets and trader profiles.
    Search {
        /// Search query.
        query: String,

        /// Result type: market | user (default both).
        #[arg(long)]
        r#type: Option<String>,

        /// Max results.
        #[arg(long, default_value = "10")]
        limit: usize,
    },
}

pub async fn run(ctx: CliContext, args: MarketArgs) -> Result<()> {
    match args.sub {
        MarketCmd::Discover { lens, search, min_volume, min_liquidity, sort, limit } =>
            discover(ctx, lens, search, min_volume, min_liquidity, sort, limit).await,
        MarketCmd::Price { token_id }        => price(ctx, token_id).await,
        MarketCmd::Book  { token_id }        => book(ctx, token_id).await,
        MarketCmd::Trades { token_id, limit } => trades(ctx, token_id, limit).await,
        MarketCmd::History { token_id, interval, fidelity } =>
            history(ctx, token_id, interval, fidelity).await,
        MarketCmd::Search { query, r#type, limit } => search(ctx, query, r#type, limit).await,
    }
}

// ── Discover ─────────────────────────────────────────────────────────────────

async fn discover(
    ctx: CliContext,
    lens: String,
    search: Option<String>,
    min_volume: Option<f64>,
    min_liquidity: Option<f64>,
    sort: String,
    limit: usize,
) -> Result<()> {
    let pb = spinner("Fetching markets…");

    // Map lens to Gamma API tag filter.
    let tag_slug: Option<&str> = match lens.as_str() {
        "sports"      => Some("sports"),
        "crypto"      => Some("crypto"),
        "politics"    => Some("politics"),
        "geo"         => Some("geopolitics"),
        "ending-soon" => None, // handled by sort
        _             => None,
    };

    let sort_param = match sort.as_str() {
        "volume"      => "volume",
        "liquidity"   => "liquidity",
        "newest"      => "startDate",
        "ending-soon" => "endDate",
        _             => "volume24hr",
    };

    let mut qs = format!(
        "/markets?active=true&closed=false&order={sort_param}&ascending=false&limit={limit}"
    );
    if let Some(ref q) = search       { qs.push_str(&format!("&slug_contains={}", urlenc(q))); }
    if let Some(tag)   = tag_slug     { qs.push_str(&format!("&tag_slug={tag}")); }
    if let Some(v)     = min_volume   { qs.push_str(&format!("&volume_num_min={v}")); }
    if let Some(l)     = min_liquidity{ qs.push_str(&format!("&liquidity_num_min={l}")); }

    let data = ctx.gamma_get(&qs).await?;
    pb.finish_and_clear();

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let markets = data.as_array()
        .or_else(|| data["markets"].as_array())
        .map(|v| v.as_slice())
        .unwrap_or_default();

    if markets.is_empty() {
        println!("{}", "No markets found.".dimmed());
        return Ok(());
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Market").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Yes").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("No").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Vol 24h").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Liquidity").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Ends").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    for m in markets {
        let question  = m["question"].as_str().unwrap_or(m["slug"].as_str().unwrap_or("—"));
        let yes_price = m["bestAsk"].as_f64()
            .or_else(|| m["outcomePrices"].as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok())))
            .map(|v| format!("{:.0}¢", v * 100.0))
            .unwrap_or_else(|| "—".to_string());
        let no_price = m["bestBid"].as_f64()
            .or_else(|| m["outcomePrices"].as_array()
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok())))
            .map(|v| format!("{:.0}¢", v * 100.0))
            .unwrap_or_else(|| "—".to_string());
        let vol24 = m["volume24hr"].as_f64()
            .or_else(|| m["volume"].as_f64())
            .map(|v| format_usd(v))
            .unwrap_or_else(|| "—".to_string());
        let liq = m["liquidity"].as_f64()
            .map(format_usd)
            .unwrap_or_else(|| "—".to_string());
        let ends = m["endDate"].as_str()
            .map(|s| s.get(..10).unwrap_or(s).to_string())
            .unwrap_or_else(|| "—".to_string());

        let label = if question.len() > 55 { format!("{}…", &question[..54]) } else { question.to_string() };
        table.add_row(vec![label, yes_price, no_price, vol24, liq, ends]);
    }
    println!("\n{table}\n");
    Ok(())
}

// ── Price ────────────────────────────────────────────────────────────────────

async fn price(ctx: CliContext, token_id: String) -> Result<()> {
    let pb = spinner("Fetching prices…");

    // Fetch buy + sell prices from CLOB
    let buy_resp  = ctx.clob_get(&format!("/price?token_id={token_id}&side=BUY")).await;
    let sell_resp = ctx.clob_get(&format!("/price?token_id={token_id}&side=SELL")).await;
    let mid_resp  = ctx.clob_get(&format!("/midpoint?token_id={token_id}")).await;

    pb.finish_and_clear();

    if matches!(ctx.output, OutputFormat::Json) {
        let combined = serde_json::json!({
            "token_id": token_id,
            "buy":  buy_resp.as_ref().map(|v| v["price"].as_str()).ok().flatten(),
            "sell": sell_resp.as_ref().map(|v| v["price"].as_str()).ok().flatten(),
            "mid":  mid_resp.as_ref().map(|v| v["mid"].as_str()).ok().flatten(),
        });
        println!("{}", serde_json::to_string_pretty(&combined)?);
        return Ok(());
    }

    let ask = buy_resp.as_ref().ok()
        .and_then(|v| v["price"].as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let bid = sell_resp.as_ref().ok()
        .and_then(|v| v["price"].as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let mid = mid_resp.as_ref().ok()
        .and_then(|v| v["mid"].as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let spread = match (bid, ask) {
        (Some(b), Some(a)) => Some(a - b),
        _ => None,
    };

    println!("\n  Token: {}\n", token_id.bold());
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Metric").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);
    let rows = [
        ("Midpoint", mid.map(|v| format!("{:.4} ({:.0}¢)", v, v * 100.0))),
        ("Ask (Buy)",  ask.map(|v| format!("{:.4} ({:.0}¢)", v, v * 100.0))),
        ("Bid (Sell)", bid.map(|v| format!("{:.4} ({:.0}¢)", v, v * 100.0))),
        ("Spread",     spread.map(|v| format!("{:.4} ({:.2}¢)", v, v * 100.0))),
    ];
    for (k, v) in &rows {
        table.add_row(vec![k.to_string(), v.clone().unwrap_or_else(|| "—".to_string())]);
    }
    println!("{table}\n");
    Ok(())
}

// ── Order Book ───────────────────────────────────────────────────────────────

async fn book(ctx: CliContext, token_id: String) -> Result<()> {
    let pb = spinner("Fetching order book…");
    let data = ctx.clob_get(&format!("/order-book/{token_id}")).await?;
    pb.finish_and_clear();

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    println!("\n  Order Book: {}\n", token_id.bold());

    let bids = data["bids"].as_array().map(|v| v.as_slice()).unwrap_or_default();
    let asks = data["asks"].as_array().map(|v| v.as_slice()).unwrap_or_default();

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Bid Price").add_attribute(Attribute::Bold).fg(Color::Green),
        Cell::new("Bid Size").add_attribute(Attribute::Bold).fg(Color::Green),
        Cell::new("Ask Price").add_attribute(Attribute::Bold).fg(Color::Red),
        Cell::new("Ask Size").add_attribute(Attribute::Bold).fg(Color::Red),
    ]);

    let depth = bids.len().max(asks.len()).min(10);
    for i in 0..depth {
        let bid_p = bids.get(i).and_then(|b| b["price"].as_str()).unwrap_or("—");
        let bid_s = bids.get(i).and_then(|b| b["size"].as_str()).unwrap_or("—");
        let ask_p = asks.get(i).and_then(|a| a["price"].as_str()).unwrap_or("—");
        let ask_s = asks.get(i).and_then(|a| a["size"].as_str()).unwrap_or("—");
        table.add_row(vec![bid_p, bid_s, ask_p, ask_s]);
    }
    println!("{table}\n");
    Ok(())
}

// ── Trades ───────────────────────────────────────────────────────────────────

async fn trades(ctx: CliContext, token_id: String, limit: usize) -> Result<()> {
    let pb = spinner("Fetching recent trades…");
    let data = ctx.clob_get(&format!("/trades?token_id={token_id}&limit={limit}")).await?;
    pb.finish_and_clear();

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let trades_arr = data.as_array()
        .or_else(|| data["data"].as_array())
        .map(|v| v.as_slice())
        .unwrap_or_default();

    if trades_arr.is_empty() {
        println!("{}", "No recent trades found.".dimmed());
        return Ok(());
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Time").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Side").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Price").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Size").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    for t in trades_arr.iter().take(limit) {
        let ts   = t["created_at"].as_str().or_else(|| t["timestamp"].as_str()).unwrap_or("—");
        let side = t["side"].as_str().unwrap_or("—");
        let price = t["price"].as_str().unwrap_or("—");
        let size  = t["size"].as_str().unwrap_or("—");
        table.add_row(vec![ts, side, price, size]);
    }
    println!("\n{table}\n");
    Ok(())
}

// ── Price History ─────────────────────────────────────────────────────────────

async fn history(ctx: CliContext, token_id: String, interval: String, fidelity: u32) -> Result<()> {
    let pb = spinner("Fetching price history…");
    let data = ctx.clob_get(
        &format!("/prices-history?market={token_id}&interval={interval}&fidelity={fidelity}")
    ).await?;
    pb.finish_and_clear();

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let history_arr = data["history"].as_array()
        .or_else(|| data.as_array())
        .map(|v| v.as_slice())
        .unwrap_or_default();

    if history_arr.is_empty() {
        println!("{}", "No price history available.".dimmed());
        return Ok(());
    }

    // Simple ASCII sparkline.
    let prices: Vec<f64> = history_arr.iter()
        .filter_map(|h| h["p"].as_f64().or_else(|| h["price"].as_f64()))
        .collect();

    if prices.is_empty() {
        println!("{}", "No price data in history response.".dimmed());
        return Ok(());
    }

    let min = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = (max - min).max(1e-9);

    let bars = "▁▂▃▄▅▆▇█";
    let spark: String = prices.iter().map(|p| {
        let idx = ((p - min) / range * 7.0).round() as usize;
        bars.chars().nth(idx.min(7)).unwrap_or('▁')
    }).collect();

    println!("\n  {} — Price history ({}, {} pts)", token_id.bold(), interval, prices.len());
    println!("  Min: {:.4}  Max: {:.4}  Last: {:.4}", min, max, prices.last().unwrap());
    println!("\n  {spark}\n");
    Ok(())
}

// ── Search ───────────────────────────────────────────────────────────────────

async fn search(ctx: CliContext, query: String, filter_type: Option<String>, limit: usize) -> Result<()> {
    let pb = spinner("Searching…");
    let qs = format!("/markets?slug_contains={}&limit={limit}", urlenc(&query));
    let data = ctx.gamma_get(&qs).await?;
    pb.finish_and_clear();

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    // Skip user results unless explicitly requested — gamma returns market objects
    if matches!(filter_type.as_deref(), Some("user")) {
        println!("{}", "User search not yet supported via Gamma API.".yellow());
        return Ok(());
    }

    let markets = data.as_array()
        .or_else(|| data["markets"].as_array())
        .map(|v| v.as_slice())
        .unwrap_or_default();

    if markets.is_empty() {
        println!("{}", "No markets found.".dimmed());
        return Ok(());
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Slug").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Question").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Status").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    for m in markets.iter().take(limit) {
        let slug     = m["slug"].as_str().unwrap_or("—");
        let question = m["question"].as_str().unwrap_or("—");
        let q_short  = if question.len() > 60 { format!("{}…", &question[..59]) } else { question.to_string() };
        let active   = m["active"].as_bool().unwrap_or(false);
        let closed   = m["closed"].as_bool().unwrap_or(false);
        let status   = if closed { "closed" } else if active { "active" } else { "inactive" };
        table.add_row(vec![slug, &q_short, status]);
    }
    println!("\n{table}\n");
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner} {msg}")
            .unwrap()
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

fn format_usd(v: f64) -> String {
    if v >= 1_000_000.0 { format!("${:.1}M", v / 1_000_000.0) }
    else if v >= 1_000.0 { format!("${:.1}K", v / 1_000.0) }
    else { format!("${:.0}", v) }
}

fn urlenc(s: &str) -> String {
    s.replace(' ', "%20")
     .replace('&', "%26")
     .replace('"', "%22")
}
