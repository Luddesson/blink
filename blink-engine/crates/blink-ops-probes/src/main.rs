//! `blink-probe` — operator probes for Polymarket HFT rebuild.
//!
//! Subcommands (see plan §5 risks):
//!   * `ratelimit`  — R-2: probe Polymarket CLOB POST /order rate limit.
//!   * `cloudflare` — R-7: measure Cloudflare submit-path latency.
//!   * `mempool`    — R-8: observe Polygon pending-tx firehose.
//!
//! These tools are intended to be run by the operator against **live**
//! infrastructure. Nothing here should ever be invoked from CI.

use clap::{Parser, Subcommand};

mod report;
mod ratelimit;
mod cloudflare;
mod mempool;

#[derive(Parser, Debug)]
#[command(
    name = "blink-probe",
    version,
    about = "Operator probes for Polymarket HFT rebuild (R-2 / R-7 / R-8).",
    long_about = "Measurement tools the operator runs against real infrastructure to \
                  unblock risk decisions documented in the build plan. \
                  Never run these from CI or against live Polymarket without \
                  the explicit acknowledgement flag."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// R-2: Probe Polymarket CLOB POST /order rate limit.
    Ratelimit(ratelimit::Args),
    /// R-7: Measure Cloudflare submit-path latency + anycast region.
    Cloudflare(cloudflare::Args),
    /// R-8: Observe Polygon mempool and inclusion latency.
    Mempool(mempool::Args),
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    // Install default crypto provider for rustls 0.23.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ratelimit(a) => ratelimit::run(a).await,
        Cmd::Cloudflare(a) => cloudflare::run(a).await,
        Cmd::Mempool(a) => mempool::run(a).await,
    }
}
