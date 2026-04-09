//! `blink engine` — engine status, pause, and resume.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use comfy_table::{Table, Cell, Color, Attribute, ContentArrangement};

use crate::{client::CliContext, OutputFormat};

#[derive(Args)]
pub struct EngineArgs {
    #[command(subcommand)]
    pub sub: EngineCmd,
}

#[derive(Subcommand)]
pub enum EngineCmd {
    /// Show engine status: WS, trading state, risk, subscriptions.
    Status,
    /// Pause order execution (engine keeps running, no new orders).
    Pause,
    /// Resume order execution.
    Resume,
    /// Show risk manager metrics.
    Risk,
    /// Show latency percentiles.
    Latency,
}

pub async fn run(ctx: CliContext, args: EngineArgs) -> Result<()> {
    match args.sub {
        EngineCmd::Status  => status(ctx).await,
        EngineCmd::Pause   => pause(ctx).await,
        EngineCmd::Resume  => resume(ctx).await,
        EngineCmd::Risk    => risk(ctx).await,
        EngineCmd::Latency => latency(ctx).await,
    }
}

async fn status(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_get("/api/status").await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let mode_data = ctx.engine_get("/api/mode").await.ok();
    let mode = mode_data
        .as_ref()
        .and_then(|v| v["mode"].as_str())
        .unwrap_or("unknown");

    let ws       = data["ws_connected"].as_bool().unwrap_or(false);
    let paused   = data["trading_paused"].as_bool().unwrap_or(false);
    let msgs     = data["messages_total"].as_u64().unwrap_or(0);
    let risk_st  = data["risk_status"].as_str().unwrap_or("—");
    let subs     = data["subscriptions"].as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    println!("\n  Blink Engine Status\n");

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Field").add_attribute(Attribute::Bold).fg(Color::Cyan),
        Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
    ]);

    let ws_str = if ws { "✓ Connected".green().to_string() } else { "✗ Disconnected".red().to_string() };
    let paused_str = if paused { "⏸ Paused".yellow().to_string() } else { "▶ Running".green().to_string() };
    let risk_str = if risk_st == "OK" { risk_st.green().to_string() } else { risk_st.red().to_string() };

    table.add_row(vec!["Mode".to_string(), mode.to_string()]);
    table.add_row(vec!["WebSocket".to_string(), ws_str]);
    table.add_row(vec!["Trading".to_string(), paused_str]);
    table.add_row(vec!["Risk Status".to_string(), risk_str]);
    table.add_row(vec!["Subscriptions".to_string(), subs.to_string()]);
    table.add_row(vec!["Messages (total)".to_string(), msgs.to_string()]);
    println!("{table}\n");

    // List subscriptions if any
    if let Some(subs_arr) = data["subscriptions"].as_array() {
        if !subs_arr.is_empty() {
            println!("  Subscribed tokens:");
            for s in subs_arr {
                if let Some(id) = s.as_str() {
                    println!("    • {id}");
                }
            }
            println!();
        }
    }
    Ok(())
}

async fn pause(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_post("/api/pause", serde_json::json!({"paused": true})).await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;
    println!("{}", "⏸  Engine paused — no new orders will be placed.".yellow());
    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn resume(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_post("/api/pause", serde_json::json!({"paused": false})).await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;
    println!("{}", "▶  Engine resumed — order execution active.".green());
    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn risk(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_get("/api/risk").await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    if let Some(obj) = data.as_object() {
        let mut table = Table::new();
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![
            Cell::new("Metric").add_attribute(Attribute::Bold).fg(Color::Cyan),
            Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
        ]);
        for (k, v) in obj {
            let vs = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null      => "—".to_string(),
                other                        => other.to_string(),
            };
            table.add_row(vec![k.as_str(), vs.as_str()]);
        }
        println!("\n{table}\n");
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn latency(ctx: CliContext) -> Result<()> {
    let data = ctx.engine_get("/api/latency").await
        .map_err(|e| anyhow::anyhow!("Engine unreachable — is Blink running? ({e})"))?;

    if matches!(ctx.output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    if let Some(obj) = data.as_object() {
        let mut table = Table::new();
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![
            Cell::new("Metric").add_attribute(Attribute::Bold).fg(Color::Cyan),
            Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
        ]);
        for (k, v) in obj {
            let vs = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null      => "—".to_string(),
                other                        => other.to_string(),
            };
            table.add_row(vec![k.as_str(), vs.as_str()]);
        }
        println!("\n{table}\n");
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}
