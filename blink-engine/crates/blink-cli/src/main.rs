//! Blink CLI — interactive terminal tool for the Blink trading engine.
//!
//! Connects to:
//!   - The running Blink engine REST API (`BLINK_HOST`, default http://localhost:3030)
//!   - Polymarket CLOB REST API (https://clob.polymarket.com)
//!   - Polymarket Gamma API (https://gamma-api.polymarket.com)

mod commands;
mod client;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};
use dotenvy::dotenv;

#[derive(Parser)]
#[command(
    name    = "blink",
    version = env!("CARGO_PKG_VERSION"),
    about   = "Blink Engine CLI — trade, monitor, and control from your terminal",
    long_about = None
)]
struct Cli {
    /// Blink engine base URL (overrides BLINK_HOST env var).
    #[arg(long, env = "BLINK_HOST", default_value = "http://localhost:3030", global = true)]
    host: String,

    /// Output format.
    #[arg(long, value_enum, default_value = "table", global = true)]
    output: OutputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    Table,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// Portfolio positions, balances, and P&L.
    Portfolio(commands::portfolio::PortfolioArgs),

    /// Discover, search, and inspect Polymarket markets.
    Market(commands::market::MarketArgs),

    /// Place and manage orders (buy, sell, limit, cancel).
    Order(commands::order::OrderArgs),

    /// Engine control — status, pause, resume.
    Engine(commands::engine::EngineArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv();
    let cli = Cli::parse();
    let ctx = client::CliContext::new(cli.host, cli.output);

    match cli.command {
        Commands::Portfolio(args) => commands::portfolio::run(ctx, args).await,
        Commands::Market(args)    => commands::market::run(ctx, args).await,
        Commands::Order(args)     => commands::order::run(ctx, args).await,
        Commands::Engine(args)    => commands::engine::run(ctx, args).await,
    }
}
