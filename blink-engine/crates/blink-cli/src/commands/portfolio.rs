//! `blink portfolio` — positions, balances, and P&L from the engine REST API.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use comfy_table::{Table, Cell, Color, Attribute, ContentArrangement};

use crate::{client::CliContext, output::{format_pnl, pct}, OutputFormat};

#[derive(Args)]
pub struct PortfolioArgs {
    #[command(subcommand)]
    pub sub: PortfolioCmd,
}

#[derive(Subcommand)]
pub enum PortfolioCmd {
    /// Open positions with unrealised P&L.
    Positions,
    /// Cash balance, NAV, and summary stats.
    Balances,
    /// Closed trade history and realised P&L.
    Pnl {
        /// Number of closed trades to show (default 20).
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

pub async fn run(ctx: CliContext, args: PortfolioArgs) -> Result<()> {
    match args.sub {
        PortfolioCmd::Positions => show_positions(ctx).await,
        PortfolioCmd::Balances  => show_balances(ctx).await,
        PortfolioCmd::Pnl { limit } => show_pnl(ctx, limit).await,
    }
}

// ── Positions ────────────────────────────────────────────────────────────────

async fn show_positions(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_get("/api/portfolio").await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let nav    = data["nav_usdc"].as_f64().unwrap_or(0.0);
    let cash   = data["cash_usdc"].as_f64().unwrap_or(0.0);
    let unr    = data["unrealized_pnl_usdc"].as_f64().unwrap_or(0.0);
    let wr     = data["win_rate_pct"].as_f64().unwrap_or(0.0);

    println!(
        "\n  NAV: {}  |  Cash: ${:.2}  |  Unrealised P&L: {}  |  Win rate: {}",
        format!("${:.2}", nav).bold(),
        cash,
        format_pnl(unr),
        pct(wr),
    );

    let positions = match data["open_positions"].as_array() {
        Some(p) if !p.is_empty() => p,
        _ => {
            println!("{}", "  No open positions.".dimmed());
            return Ok(());
        }
    };

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Market").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Side").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Shares").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Entry").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Current").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("P&L").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("P&L %").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    for pos in positions {
        let title   = pos["market_title"].as_str()
            .or_else(|| pos["token_id"].as_str()).unwrap_or("—");
        let outcome = pos["market_outcome"].as_str().unwrap_or("");
        let label   = if outcome.is_empty() { title.to_string() } else { format!("{title} [{outcome}]") };
        let side    = pos["side"].as_str().unwrap_or("—");
        let shares  = pos["shares"].as_f64().unwrap_or(0.0);
        let entry   = pos["entry_price"].as_f64().unwrap_or(0.0);
        let current = pos["current_price"].as_f64().unwrap_or(0.0);
        let value   = shares * current;
        let pnl     = pos["unrealized_pnl"].as_f64().unwrap_or(0.0);
        let pnl_pct = pos["unrealized_pnl_pct"].as_f64().unwrap_or(0.0);

        table.add_row(vec![
            label,
            side.to_string(),
            format!("{:.2}", shares),
            format!("{:.3}", entry),
            format!("{:.3}", current),
            format!("${:.2}", value),
            format_pnl(pnl),
            pct(pnl_pct),
        ]);
    }
    println!("{table}\n");
    Ok(())
}

// ── Balances ─────────────────────────────────────────────────────────────────

async fn show_balances(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_get("/api/portfolio").await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Metric").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    let pairs = [
        ("NAV (USDC)",           data["nav_usdc"].as_f64().map(|v| format!("${:.4}", v))),
        ("Cash (USDC)",          data["cash_usdc"].as_f64().map(|v| format!("${:.4}", v))),
        ("Invested (USDC)",      data["invested_usdc"].as_f64().map(|v| format!("${:.4}", v))),
        ("Unrealised P&L",       data["unrealized_pnl_usdc"].as_f64().map(format_pnl)),
        ("Realised P&L",         data["realized_pnl_usdc"].as_f64().map(format_pnl)),
        ("Fees paid (USDC)",     data["fees_paid_usdc"].as_f64().map(|v| format!("${:.4}", v))),
        ("Fill rate",            data["fill_rate_pct"].as_f64().map(|v| pct(v))),
        ("Win rate",             data["win_rate_pct"].as_f64().map(|v| pct(v))),
        ("Avg slippage (bps)",   data["avg_slippage_bps"].as_f64().map(|v| format!("{:.1}", v))),
        ("Total signals",        data["total_signals"].as_u64().map(|v| v.to_string())),
        ("Filled orders",        data["filled_orders"].as_u64().map(|v| v.to_string())),
        ("Uptime (s)",           data["uptime_secs"].as_u64().map(|v| v.to_string())),
    ];

    for (k, v) in &pairs {
        table.add_row(vec![k.to_string(), v.clone().unwrap_or_else(|| "—".to_string())]);
    }
    println!("\n{table}\n");
    Ok(())
}

// ── P&L history ──────────────────────────────────────────────────────────────

async fn show_pnl(ctx: CliContext, limit: usize) -> Result<()> {
    let data = ctx.engine_get(&format!("/api/history?per_page={limit}&page=1")).await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let trades = match data["trades"].as_array().or_else(|| data.as_array()) {
        Some(t) => t,
        None => {
            println!("{}", "No closed trades yet.".dimmed());
            return Ok(());
        }
    };

    if trades.is_empty() {
        println!("{}", "No closed trades yet.".dimmed());
        return Ok(());
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Market").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Side").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Entry").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Exit").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Shares").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Realised P&L").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Fees").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Reason").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    for t in trades.iter().take(limit) {
        let title  = t["market_title"].as_str()
            .or_else(|| t["token_id"].as_str()).unwrap_or("—");
        let side   = t["side"].as_str().unwrap_or("—");
        let entry  = t["entry_price"].as_f64().unwrap_or(0.0);
        let exit   = t["exit_price"].as_f64().unwrap_or(0.0);
        let shares = t["shares"].as_f64().unwrap_or(0.0);
        let pnl    = t["realized_pnl"].as_f64().unwrap_or(0.0);
        let fees   = t["fees_paid_usdc"].as_f64().unwrap_or(0.0);
        let reason = t["reason"].as_str().unwrap_or("—");

        table.add_row(vec![
            title.to_string(),
            side.to_string(),
            format!("{:.3}", entry),
            format!("{:.3}", exit),
            format!("{:.2}", shares),
            format_pnl(pnl),
            format!("${:.4}", fees),
            reason.to_string(),
        ]);
    }
    println!("\n{table}\n");
    Ok(())
}
