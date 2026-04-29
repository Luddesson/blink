//! Postgres LISTEN/NOTIFY listener for remote engine commands.
//!
//! Connects to `POSTGRES_URL`, listens on `blink_command_channel`, and
//! logs incoming commands.

use anyhow::Result;
use futures_util::StreamExt;
use postgres_native_tls::MakeTlsConnector;
use serde::Deserialize;
use tokio_postgres::AsyncMessage;
use tracing::{error, info, warn};

#[derive(Deserialize, Debug)]
pub struct EngineCommand {
    pub id: i64,
    pub command_type: String,
    pub payload: serde_json::Value,
}

/// Spawns a task that listens for Postgres notifications on `blink_command_channel`.
pub async fn run_command_listener(url: String) -> Result<()> {
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()?;
    let connector = MakeTlsConnector::new(connector);

    let (client, mut connection) = tokio_postgres::connect(&url, connector).await?;

    // The connection object handles the actual communication. We turn it into a
    // stream of asynchronous messages (like Notifications) and process them.
    tokio::spawn(async move {
        let mut stream = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(AsyncMessage::Notification(notification)) => {
                    let payload = notification.payload();
                    match serde_json::from_str::<EngineCommand>(payload) {
                        Ok(cmd) => {
                            info!(
                                "Received remote command: {} (id: {})",
                                cmd.command_type, cmd.id
                            );
                        }
                        Err(e) => {
                            warn!("Failed to parse command payload: {}. Raw: {}", e, payload);
                        }
                    }
                }
                Ok(AsyncMessage::Notice(notice)) => {
                    info!("Postgres notice: {}", notice.message());
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Postgres command listener connection error: {}", e);
                    break;
                }
            }
        }
    });

    // Register interest in the channel.
    client.execute("LISTEN blink_command_channel", &[]).await?;
    info!("Postgres LISTEN blink_command_channel active");

    // Keep the task alive to maintain the connection and client.
    futures_util::future::pending::<()>().await;

    Ok(())
}
