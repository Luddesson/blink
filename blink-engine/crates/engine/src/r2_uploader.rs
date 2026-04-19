//! Cloudflare R2 log & portfolio uploader.
//!
//! **Latency class: COLD PATH. Never call from the signal → order hot path.**
//!
//! Uploads engine artifacts to a Cloudflare R2 bucket using the S3-compatible
//! API with AWS SigV4 authentication.
//!
//! # Activation
//!
//! Set all four env vars to enable:
//! ```text
//! R2_ACCESS_KEY_ID=...
//! R2_SECRET_ACCESS_KEY=...
//! R2_ACCOUNT_ID=...
//! R2_BUCKET=blink-logs
//! ```
//!
//! # Upload schedule
//!
//! | Artifact | Path in bucket | Frequency |
//! |----------|---------------|-----------|
//! | `paper_portfolio_state.json` | `portfolio/YYYY-MM-DD.json` | Every 15 min |
//! | `engine-stdout.log` | `logs/engine-YYYYMMDD-HH.log` | Every 1 hour |

use std::time::Duration;

use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

type HmacSha256 = Hmac<Sha256>;

// ─── Config ──────────────────────────────────────────────────────────────────

struct R2Config {
    access_key_id: String,
    secret_access_key: String,
    account_id: String,
    bucket: String,
}

impl R2Config {
    fn from_env() -> Option<Self> {
        Some(Self {
            access_key_id: std::env::var("R2_ACCESS_KEY_ID").ok()?,
            secret_access_key: std::env::var("R2_SECRET_ACCESS_KEY").ok()?,
            account_id: std::env::var("R2_ACCOUNT_ID").ok()?,
            bucket: std::env::var("R2_BUCKET").unwrap_or_else(|_| "blink-logs".to_string()),
        })
    }

    fn endpoint(&self) -> String {
        format!("https://{}.r2.cloudflarestorage.com", self.account_id)
    }
}

// ─── SigV4 helpers ───────────────────────────────────────────────────────────

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    to_hex(&h.finalize())
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ─── Signing ─────────────────────────────────────────────────────────────────

/// Returns the signed headers required for a PUT request to R2.
///
/// Headers returned: `x-amz-date`, `x-amz-content-sha256`, `authorization`.
fn sign_put(
    cfg: &R2Config,
    key: &str,
    body: &[u8],
    now: &chrono::DateTime<chrono::Utc>,
) -> Vec<(String, String)> {
    let date_str = now.format("%Y%m%d").to_string();
    let datetime_str = now.format("%Y%m%dT%H%M%SZ").to_string();
    let host = format!("{}.r2.cloudflarestorage.com", cfg.account_id);
    let region = "auto";
    let service = "s3";

    let payload_hash = sha256_hex(body);
    let canonical_uri = format!("/{}/{}", cfg.bucket, key);

    // Canonical headers (must be sorted alphabetically by name)
    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{datetime_str}\n");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";

    let canonical_request =
        format!("PUT\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

    let credential_scope = format!("{date_str}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{datetime_str}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    // Derive signing key: 4-step HMAC chain
    let k_date = hmac_sha256(
        format!("AWS4{}", cfg.secret_access_key).as_bytes(),
        date_str.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");

    let signature = to_hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        cfg.access_key_id, credential_scope, signed_headers, signature
    );

    vec![
        ("x-amz-date".to_string(), datetime_str),
        ("x-amz-content-sha256".to_string(), payload_hash),
        ("authorization".to_string(), auth),
    ]
}

// ─── Upload ───────────────────────────────────────────────────────────────────

async fn upload_bytes(client: &Client, cfg: &R2Config, key: &str, body: Vec<u8>) {
    let now = chrono::Utc::now();
    let headers = sign_put(cfg, key, &body, &now);
    let url = format!("{}/{}/{}", cfg.endpoint(), cfg.bucket, key);

    let mut req = client
        .put(&url)
        .header("content-type", "application/octet-stream")
        .body(body);

    for (k, v) in headers {
        req = req.header(&k, &v);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            info!(key, "R2 upload OK ({})", resp.status());
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!(key, %status, "R2 upload failed: {body}");
        }
        Err(e) => {
            warn!(key, "R2 upload error: {e}");
        }
    }
}

// ─── Upload tasks ─────────────────────────────────────────────────────────────

async fn upload_portfolio(client: &Client, cfg: &R2Config) {
    let portfolio_path = std::env::var("PAPER_STATE_PATH")
        .unwrap_or_else(|_| "logs/paper_portfolio_state.json".to_string());

    match tokio::fs::read(&portfolio_path).await {
        Ok(bytes) => {
            let date_str = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let key = format!("portfolio/{date_str}.json");
            upload_bytes(client, cfg, &key, bytes).await;
        }
        Err(e) => {
            // File may not exist yet — that's OK
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("R2: could not read portfolio file: {e}");
            }
        }
    }
}

async fn upload_engine_log(client: &Client, cfg: &R2Config) {
    let log_path = "logs/engine-stdout.log";

    match tokio::fs::read(log_path).await {
        Ok(bytes) => {
            let now = chrono::Utc::now();
            let key = format!("logs/engine-{}.log", now.format("%Y%m%d-%H"));
            upload_bytes(client, cfg, &key, bytes).await;
        }
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("R2: could not read engine log: {e}");
            }
        }
    }
}

// ─── Entry point ──────────────────────────────────────────────────────────────

/// Spawns the R2 uploader background task if all required env vars are set.
///
/// Returns immediately if any required env var is missing — never panics.
pub fn start_r2_uploader() {
    let Some(cfg) = R2Config::from_env() else {
        info!("R2_ACCESS_KEY_ID not set — Cloudflare R2 uploader disabled");
        return;
    };

    info!(
        bucket = cfg.bucket.as_str(),
        account = cfg.account_id.as_str(),
        "Cloudflare R2 uploader enabled"
    );

    tokio::spawn(async move {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");

        // Upload immediately on start
        upload_portfolio(&client, &cfg).await;

        let mut portfolio_interval = tokio::time::interval(Duration::from_secs(15 * 60));
        let mut log_interval = tokio::time::interval(Duration::from_secs(60 * 60));
        portfolio_interval.tick().await; // consume first tick (already ran above)
        log_interval.tick().await; // consume first tick

        loop {
            tokio::select! {
                _ = portfolio_interval.tick() => {
                    upload_portfolio(&client, &cfg).await;
                }
                _ = log_interval.tick() => {
                    upload_engine_log(&client, &cfg).await;
                }
            }
        }
    });
}
