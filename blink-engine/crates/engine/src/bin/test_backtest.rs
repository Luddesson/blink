use engine::backtest_engine::{load_ticks_csv, BacktestConfig, BacktestEngine};

fn main() -> anyhow::Result<()> {
    let csv_path = "/root/backtest_data.csv";
    let rn1_wallet = "0xrn1wallet".to_string();

    let ticks = load_ticks_csv(csv_path)?;

    // Use very loose parameters to ensure some trades happen
    let cfg = BacktestConfig {
        rn1_wallet,
        starting_usdc: 1000.0,
        size_multiplier: 0.10, // 10%
        drift_threshold: 0.05, // 5%
        fill_window_ms: 1000,
        slippage_bps: 0,
    };

    let mut engine = BacktestEngine::new(cfg, ticks);
    let results = engine.run();

    println!("Total Trades: {}", results.total_trades);
    println!("Total Signals: {}", engine.portfolio.total_signals);
    println!("Skipped Orders: {}", engine.portfolio.skipped_orders);
    println!("Aborted Orders: {}", engine.portfolio.aborted_orders);
    println!("Filled Orders: {}", engine.portfolio.filled_orders);
    println!("Final Return%: {:.4}%", results.total_return_pct);

    println!("\nTrade Details:");
    println!(
        "{:<10} {:<10} {:<10} {:<10} {:<10}",
        "Side", "Entry", "Exit", "PnL", "Reason"
    );
    for trade in &engine.portfolio.closed_trades {
        println!(
            "{:<10?} {:<10.4} {:<10.4} {:<10.4} {:<10}",
            trade.side, trade.entry_price, trade.exit_price, trade.realized_pnl, trade.reason
        );
    }

    Ok(())
}
