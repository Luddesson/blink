//! `blink order` — place and manage orders via the Blink engine or CLOB API.
//!
//! In paper mode the engine simulates orders; in live mode orders go to the CLOB.
//! Interactive confirmation is required unless `--yes` is passed.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use comfy_table::{Table, Cell, Color, Attribute, ContentArrangement};

use crate::{client::CliContext, output::format_pnl, OutputFormat};

#[derive(Args)]
pub struct OrderArgs {
    #[command(subcommand)]
    pub sub: OrderCmd,
}

#[derive(Subcommand)]
pub enum OrderCmd {
    /// Market-buy shares on an outcome.
    Buy {
        /// Market slug or token ID.
        market: String,
        /// Outcome label (e.g. "Yes" or "No").
        outcome: String,
        /// Amount in USD to spend.
        amount_usd: f64,
        /// Skip confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Market-sell shares on an outcome.
    Sell {
        /// Market slug or token ID.
        market: String,
        /// Outcome label.
        outcome: String,
        /// Number of shares to sell.
        shares: f64,
        /// Skip confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Place a limit buy order.
    LimitBuy {
        market: String,
        outcome: String,
        #[arg(long)] price: f64,
        #[arg(long)] shares: f64,
        /// Order expiration: gtc | gtd | fok | fak.
        #[arg(long, default_value = "gtc")] expiration: String,
        /// Reject if the order would fill immediately (maker-only).
        #[arg(long)] post_only: bool,
        #[arg(long)] yes: bool,
    },
    /// Place a limit sell order.
    LimitSell {
        market: String,
        outcome: String,
        #[arg(long)] price: f64,
        #[arg(long)] shares: f64,
        #[arg(long, default_value = "gtc")] expiration: String,
        #[arg(long)] post_only: bool,
        #[arg(long)] yes: bool,
    },
    /// List open orders.
    List,
    /// Cancel a specific order by ID.
    Cancel {
        order_id: String,
    },
    /// Cancel all open orders.
    CancelAll {
        #[arg(long)] yes: bool,
    },
}

pub async fn run(ctx: CliContext, args: OrderArgs) -> Result<()> {
    match args.sub {
        OrderCmd::Buy { market, outcome, amount_usd, yes } =>
            buy(ctx, market, outcome, amount_usd, yes).await,
        OrderCmd::Sell { market, outcome, shares, yes } =>
            sell(ctx, market, outcome, shares, yes).await,
        OrderCmd::LimitBuy { market, outcome, price, shares, expiration, post_only, yes } =>
            limit_order(ctx, "buy", market, outcome, price, shares, expiration, post_only, yes).await,
        OrderCmd::LimitSell { market, outcome, price, shares, expiration, post_only, yes } =>
            limit_order(ctx, "sell", market, outcome, price, shares, expiration, post_only, yes).await,
        OrderCmd::List       => list_orders(ctx).await,
        OrderCmd::Cancel { order_id } => cancel_order(ctx, order_id).await,
        OrderCmd::CancelAll { yes }   => cancel_all(ctx, yes).await,
    }
}

// ── Buy ──────────────────────────────────────────────────────────────────────

async fn buy(ctx: CliContext, market: String, outcome: String, amount_usd: f64, yes: bool) -> Result<()> {
    // Fetch current price for preview
    let price = fetch_market_price(&ctx, &market, &outcome).await;

    println!("\n  {} {} on {}", "Buy".green().bold(), outcome.bold(), market.bold());
    if let Some(p) = price {
        let est_shares = amount_usd / p;
        println!("  Price: {:.3}  |  Amount: ${:.2}  |  Est. shares: {:.2}  |  Potential: ${:.2}",
            p, amount_usd, est_shares, est_shares);
    } else {
        println!("  Amount: ${:.2}", amount_usd);
    }

    if !yes && !confirm("Confirm order? [y/N]: ")? {
        println!("{}", "Order cancelled.".dimmed());
        return Ok(());
    }

    let body = serde_json::json!({
        "action": "buy",
        "market": market,
        "outcome": outcome,
        "amount_usd": amount_usd,
    });
    let resp = ctx.engine_post("/api/positions/buy", body).await;
    handle_order_response(resp, &ctx.output)
}

// ── Sell ─────────────────────────────────────────────────────────────────────

async fn sell(ctx: CliContext, market: String, outcome: String, shares: f64, yes: bool) -> Result<()> {
    let price = fetch_market_price(&ctx, &market, &outcome).await;

    println!("\n  {} {} on {}", "Sell".red().bold(), outcome.bold(), market.bold());
    if let Some(p) = price {
        println!("  Price: {:.3}  |  Shares: {:.2}  |  Est. proceeds: ${:.2}", p, shares, shares * p);
    } else {
        println!("  Shares: {:.2}", shares);
    }

    if !yes && !confirm("Confirm order? [y/N]: ")? {
        println!("{}", "Order cancelled.".dimmed());
        return Ok(());
    }

    let body = serde_json::json!({
        "action": "sell",
        "market": market,
        "outcome": outcome,
        "shares": shares,
    });
    let resp = ctx.engine_post("/api/positions/sell", body).await;
    handle_order_response(resp, &ctx.output)
}

// ── Limit Order ───────────────────────────────────────────────────────────────

async fn limit_order(
    ctx: CliContext,
    side: &str,
    market: String,
    outcome: String,
    price: f64,
    shares: f64,
    expiration: String,
    post_only: bool,
    yes: bool,
) -> Result<()> {
    let notional = price * shares;
    println!(
        "\n  Limit {} {} on {} — price: {:.4}  shares: {:.2}  notional: ${:.2}  exp: {}{}",
        if side == "buy" { "Buy".green().bold().to_string() } else { "Sell".red().bold().to_string() },
        outcome.bold(), market.bold(),
        price, shares, notional, expiration,
        if post_only { "  [post-only]" } else { "" }
    );

    if !yes && !confirm("Confirm limit order? [y/N]: ")? {
        println!("{}", "Order cancelled.".dimmed());
        return Ok(());
    }

    let body = serde_json::json!({
        "action": format!("limit_{side}"),
        "market": market,
        "outcome": outcome,
        "price": price,
        "shares": shares,
        "expiration": expiration,
        "post_only": post_only,
    });
    let resp = ctx.engine_post("/api/positions/limit", body).await;
    handle_order_response(resp, &ctx.output)
}

// ── List Orders ───────────────────────────────────────────────────────────────

async fn list_orders(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_get("/api/portfolio").await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data["open_positions"])?);
        return Ok(());
    }

    let positions = match data["open_positions"].as_array() {
        Some(p) if !p.is_empty() => p,
        _ => {
            println!("{}", "  No open positions / orders.".dimmed());
            return Ok(());
        }
    };

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("ID").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Market").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Side").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Entry").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Shares").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Unrealised P&L").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    for pos in positions {
        let id      = pos["id"].as_u64().map(|v| v.to_string()).unwrap_or_else(|| "—".to_string());
        let title   = pos["market_title"].as_str()
            .or_else(|| pos["token_id"].as_str()).unwrap_or("—");
        let side    = pos["side"].as_str().unwrap_or("—");
        let entry   = pos["entry_price"].as_f64().unwrap_or(0.0);
        let shares  = pos["shares"].as_f64().unwrap_or(0.0);
        let pnl     = pos["unrealized_pnl"].as_f64().unwrap_or(0.0);
        let t_short = if title.len() > 40 { format!("{}…", &title[..39]) } else { title.to_string() };
        table.add_row(vec![
            id, t_short, side.to_string(),
            format!("{:.3}", entry), format!("{:.2}", shares),
            format_pnl(pnl),
        ]);
    }
    println!("\n{table}\n");
    Ok(())
}

// ── Cancel ────────────────────────────────────────────────────────────────────

async fn cancel_order(ctx: CliContext, order_id: String) -> Result<()> {
    let body = serde_json::json!({"order_id": order_id});
    let resp = ctx.engine_post("/api/orders/cancel", body).await;
    handle_order_response(resp, &ctx.output)
}

async fn cancel_all(ctx: CliContext, yes: bool) -> Result<()> {
    if !yes && !confirm("Cancel ALL open orders? [y/N]: ")? {
        println!("{}", "Cancelled.".dimmed());
        return Ok(());
    }
    let resp = ctx.engine_post("/api/orders/cancel-all", serde_json::json!({})).await;
    handle_order_response(resp, &ctx.output)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn fetch_market_price(ctx: &CliContext, token_id: &str, _outcome: &str) -> Option<f64> {
    let resp = ctx.clob_get(&format!("/midpoint?token_id={token_id}")).await.ok()?;
    resp["mid"].as_str()?.parse::<f64>().ok()
}

fn handle_order_response(resp: Result<serde_json::Value>, fmt: &OutputFormat) -> Result<()> {
    match resp {
        Ok(v) => {
            println!("{}", "✓ Order submitted.".green());
            if matches!(fmt, OutputFormat::Json) {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else if let Some(id) = v["order_id"].as_str() {
                println!("  Order ID: {id}");
            }
        }
        Err(e) => {
            println!("{} {e}", "✗ Order failed:".red());
        }
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    use std::io::{self, Write};
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}
