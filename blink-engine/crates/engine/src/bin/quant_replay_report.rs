use anyhow::{Context, Result};
use engine::quant_data::{
    latest_replay_timestamp_ms, latest_replay_timestamp_ms_including_ticks,
    load_replay_events_from_postgres, replay_warehouse_stats, PostgresReplayLoadConfig,
    TickEventMode,
};
use engine::quant_report::{run_signal_taker_replay, SignalTakerReplayConfig};
use postgres_native_tls::MakeTlsConnector;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    dotenvy::from_filename(".env.live").ok();

    let postgres_url = std::env::var("POSTGRES_URL")
        .context("POSTGRES_URL is required for quant replay reports")?;
    let client = connect_postgres(&postgres_url).await?;
    let tick_mode = match std::env::var("QUANT_REPLAY_TICK_MODE")
        .unwrap_or_else(|_| "ignore".to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        "book_delta" | "delta" => TickEventMode::AsBookDelta,
        "trade" | "trade_proxy" => TickEventMode::AsTradeProxy,
        _ => TickEventMode::Ignore,
    };

    let end_ms = match env_i64("QUANT_REPLAY_END_MS") {
        Some(end_ms) => end_ms,
        None => {
            let latest = if tick_mode == TickEventMode::Ignore {
                latest_replay_timestamp_ms(&client).await?
            } else {
                latest_replay_timestamp_ms_including_ticks(&client).await?
            };
            match latest {
                Some(end_ms) => end_ms,
                None => {
                    let stats = replay_warehouse_stats(&client).await?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "no_replay_data",
                            "reason": "no timestamps found in replay warehouse tables",
                            "warehouse": stats
                        }))?
                    );
                    return Ok(());
                }
            }
        }
    };
    let mut load_config =
        PostgresReplayLoadConfig::last_24h_ending_at(end_ms, std::env::var("RN1_WALLET").ok());
    if let Some(start_ms) = env_i64("QUANT_REPLAY_START_MS") {
        load_config.start_ms = start_ms;
    }
    load_config.tick_mode = tick_mode;

    let events = load_replay_events_from_postgres(&client, &load_config).await?;
    let report = run_signal_taker_replay(
        events,
        SignalTakerReplayConfig {
            starting_cash_usdc: env_f64("QUANT_REPLAY_STARTING_USDC").unwrap_or(100.0),
            max_order_usdc: env_f64("QUANT_REPLAY_MAX_ORDER_USDC").unwrap_or(2.0),
            min_signal_notional_usdc: env_f64("QUANT_REPLAY_MIN_SIGNAL_NOTIONAL_USDC")
                .unwrap_or(1.0),
            taker_latency_ms: env_i64("QUANT_REPLAY_TAKER_LATENCY_MS").unwrap_or(150),
            taker_slippage_bps: env_u64("QUANT_REPLAY_TAKER_SLIPPAGE_BPS").unwrap_or(0),
        },
    );

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn connect_postgres(url: &str) -> Result<tokio_postgres::Client> {
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .context("failed to build TLS connector")?;
    let connector = MakeTlsConnector::new(connector);
    let (client, connection) = tokio_postgres::connect(url, connector)
        .await
        .context("failed to connect to Postgres")?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("quant replay Postgres connection error: {error}");
        }
    });
    Ok(client)
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_i64(name: &str) -> Option<i64> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}
