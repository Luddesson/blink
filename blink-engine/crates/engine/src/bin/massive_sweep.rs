use engine::backtest_engine::{load_ticks_csv, run_parameter_sweep, BacktestConfig, SweepAxes};
use std::env;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <csv_path> [rn1_wallet]", args[0]);
        std::process::exit(1);
    }
    let csv_path = &args[1];
    let rn1_wallet = args.get(2).cloned().unwrap_or_default();

    println!("Loading ticks from: {}", csv_path);
    let ticks = load_ticks_csv(csv_path)?;
    println!("Loaded {} ticks", ticks.len());

    let base_cfg = BacktestConfig {
        rn1_wallet,
        starting_usdc: 1000.0,
        ..BacktestConfig::default()
    };

    let axes = SweepAxes {
        size_multiplier: vec![0.01, 0.02, 0.05, 0.10],
        slippage_bps: vec![5, 10, 20, 50],
        drift_threshold: vec![0.01, 0.015, 0.02, 0.05],
        fill_window_ms: vec![1000, 3000, 5000, 10000],
    };

    println!("Starting parameter sweep...");
    let results = run_parameter_sweep(base_cfg, ticks, axes);

    println!("\nTop 10 Results (Sorted by Sharpe):");
    println!(
        "{:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10}",
        "SizeMult", "SlipBps", "DriftThr", "FillWin", "Return%", "Sharpe", "Trades"
    );
    for row in results.iter().take(10) {
        println!(
            "{:<10.2} {:<10} {:<10.3} {:<10} {:<10.2} {:<10.4} {:<10}",
            row.size_multiplier,
            row.slippage_bps,
            row.drift_threshold,
            row.fill_window_ms,
            row.total_return_pct,
            row.sharpe_ratio,
            row.total_trades
        );
    }

    Ok(())
}
