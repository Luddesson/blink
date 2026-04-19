//! Runtime configuration loaded from environment variables (`.env` file).
//!
//! Load order: `.env` file (via [`dotenvy`]) → process environment.  
//! Call [`Config::from_env`] once at startup and wrap the result in
//! [`std::sync::Arc`] for cheap sharing across tasks.

use anyhow::{Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use std::fmt;

pub const CANONICAL_LIVE_PROFILE: &str = "canonical-v1";

/// Full runtime configuration for the Blink Engine.
#[derive(Clone)]
pub struct Config {
    /// Base URL of the Polymarket CLOB REST API.
    /// Default: `https://clob.polymarket.com`
    pub clob_host: String,

    /// WebSocket feed URL.
    /// Default: `wss://ws-subscriptions-clob.polymarket.com/ws/market`
    pub ws_url: String,

    /// Lowercase hex wallet address of the RN1 target we are tracking.
    pub rn1_wallet: String,

    /// List of Polymarket token IDs to subscribe to.
    pub markets: Vec<String>,

    /// Tracing log level directive, e.g. `"info"` or `"engine=debug"`.
    #[allow(dead_code)]
    pub log_level: String,

    /// Minimum delay between force-reconnect triggers in the WS task.
    pub ws_reconnect_debounce_ms: u64,

    /// Max raw payload preview length logged on WS parse failures.
    pub ws_parse_error_preview_chars: usize,

    /// How often the web UI WebSocket broadcasts state snapshots (seconds).
    /// Default: 10. Lower = faster UI updates, more CPU.
    pub ws_broadcast_interval_secs: u64,

    /// Rolling window size for the latency tracker (number of samples).
    /// Default: 2000.
    pub latency_window_size: usize,

    // ── Live-trading credentials ────────────────────────────────────────────
    /// When `true` the engine will submit real orders via the CLOB REST API.
    /// Defaults to `false` (paper/dry-run mode). Set `LIVE_TRADING=true` to
    /// enable. **All credential fields below must be set when this is `true`.**
    pub live_trading: bool,

    /// Hex-encoded private key for EIP-712 order signing.
    /// Loaded from `SIGNER_PRIVATE_KEY`. Empty string in paper-trading mode.
    pub signer_private_key: String,

    /// Polymarket funder (proxy-wallet) address used as the order `maker`.
    /// Loaded from `POLYMARKET_FUNDER_ADDRESS`.
    pub funder_address: String,

    /// Polymarket L2 API key.
    /// Loaded from `POLYMARKET_API_KEY`.
    pub api_key: String,

    /// Polymarket L2 API secret (base64-encoded, decoded before HMAC use).
    /// Loaded from `POLYMARKET_API_SECRET`.
    pub api_secret: String,

    /// Polymarket L2 API passphrase.
    /// Loaded from `POLYMARKET_API_PASSPHRASE`.
    pub api_passphrase: String,

    /// EIP-712 signature type for CLOB orders (0 = EOA, others per account model).
    pub polymarket_signature_type: u8,
    /// Optional explicit order nonce used in signatures.
    pub polymarket_order_nonce: u64,
    /// Optional explicit order expiration epoch seconds (0 = GTC/no-expiry semantics).
    pub polymarket_order_expiration: u64,
    /// Canonical live profile contract identifier.
    pub live_profile: String,
    /// Canary rollout stage (1, 2, 3).
    pub live_rollout_stage: u8,
    /// Stage guardrail: max notional per accepted live order.
    pub live_canary_max_order_usdc: f64,
    /// Stage guardrail: accepted live orders allowed per process session.
    pub live_canary_max_orders_per_session: usize,
    /// Stage guardrail: only allow trading inside UTC window.
    pub live_canary_daytime_only: bool,
    pub live_canary_start_hour_utc: u8,
    pub live_canary_end_hour_utc: u8,
    /// Stage guardrail: stop accepting new orders after this many submit failures in a row.
    pub live_canary_max_reject_streak: usize,
    /// Optional allowlist of token IDs allowed during canary stages.
    pub live_canary_allowed_markets: Vec<String>,

    // ── Alpha / AI autonomous trading ──────────────────────────────────────
    /// When `true`, the engine accepts alpha signals from the Python sidecar.
    /// Default: `false`.
    pub alpha_enabled: bool,
    /// URL of the Python alpha sidecar (for health checks).
    /// Default: `http://127.0.0.1:7879`.
    pub alpha_sidecar_url: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("clob_host", &self.clob_host)
            .field("ws_url", &self.ws_url)
            .field("rn1_wallet", &self.rn1_wallet)
            .field("markets", &self.markets)
            .field("log_level", &self.log_level)
            .field("ws_reconnect_debounce_ms", &self.ws_reconnect_debounce_ms)
            .field("ws_parse_error_preview_chars", &self.ws_parse_error_preview_chars)
            .field("live_trading", &self.live_trading)
            .field("signer_private_key", &"[REDACTED]")
            .field("funder_address", &self.funder_address)
            .field("api_key", &"[REDACTED]")
            .field("api_secret", &"[REDACTED]")
            .field("api_passphrase", &"[REDACTED]")
            .field("polymarket_signature_type", &self.polymarket_signature_type)
            .field("polymarket_order_nonce", &self.polymarket_order_nonce)
            .field("polymarket_order_expiration", &self.polymarket_order_expiration)
            .field("live_profile", &self.live_profile)
            .field("live_rollout_stage", &self.live_rollout_stage)
            .field("live_canary_max_order_usdc", &self.live_canary_max_order_usdc)
            .field("live_canary_max_orders_per_session", &self.live_canary_max_orders_per_session)
            .field("live_canary_daytime_only", &self.live_canary_daytime_only)
            .field("live_canary_start_hour_utc", &self.live_canary_start_hour_utc)
            .field("live_canary_end_hour_utc", &self.live_canary_end_hour_utc)
            .field("live_canary_max_reject_streak", &self.live_canary_max_reject_streak)
            .field("live_canary_allowed_markets", &self.live_canary_allowed_markets)
            .field("alpha_enabled", &self.alpha_enabled)
            .field("alpha_sidecar_url", &self.alpha_sidecar_url)
            .finish()
    }
}

impl Config {
    /// Loads configuration from environment variables.
    ///
    /// Expects the following variables to be set (typically via a `.env` file
    /// loaded by [`dotenvy`] before this call):
    ///
    /// | Variable     | Required | Description                              |
    /// |--------------|----------|------------------------------------------|
    /// | `CLOB_HOST`  | ✓        | CLOB REST API base URL                   |
    /// | `WS_URL`     | ✓        | WebSocket feed URL                       |
    /// | `RN1_WALLET` | ✓        | Wallet address to sniff                  |
    /// | `MARKETS`    | ✓        | Comma-separated token IDs                |
    /// | `LOG_LEVEL`  | ✗        | Log filter (default: `"info"`)           |
    /// | `WS_RECONNECT_DEBOUNCE_MS` | ✗ | Debounce for forced WS reconnects (default: `1500`) |
    /// | `WS_PARSE_ERROR_PREVIEW_CHARS` | ✗ | Raw payload preview chars on parse fail (default: `120`) |
    ///
    /// # Errors
    /// Returns an error if any required variable is missing or empty.
    #[tracing::instrument(name = "config::from_env")]
    pub fn from_env() -> Result<Self> {
        let clob_host = std::env::var("CLOB_HOST")
            .context("CLOB_HOST environment variable not set")?;

        let ws_url = std::env::var("WS_URL")
            .context("WS_URL environment variable not set")?;

        let rn1_wallet_raw = std::env::var("RN1_WALLET")
            .context("RN1_WALLET environment variable not set")?;

        let markets_str = std::env::var("MARKETS")
            .unwrap_or_default();

        let log_level = std::env::var("LOG_LEVEL")
            .unwrap_or_else(|_| "info".to_string());
        let ws_reconnect_debounce_ms = std::env::var("WS_RECONNECT_DEBOUNCE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1500);
        let ws_parse_error_preview_chars = std::env::var("WS_PARSE_ERROR_PREVIEW_CHARS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(120);
        let ws_broadcast_interval_secs = std::env::var("WS_BROADCAST_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1)
            .clamp(1, 60);
        let latency_window_size = std::env::var("LATENCY_WINDOW_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(2_000)
            .clamp(100, 100_000);

        // ── Live-trading credentials (optional; validated below) ────────────
        let live_trading = std::env::var("LIVE_TRADING")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        let signer_private_key;
        let funder_address;
        let api_key;
        let api_secret;
        let api_passphrase;

        // Try encrypted keystore first, fall back to env vars
        if let Ok(ks_path) = std::env::var("KEYSTORE_PATH") {
            let passphrase = std::env::var("KEYSTORE_PASSPHRASE")
                .context("KEYSTORE_PATH set but KEYSTORE_PASSPHRASE missing")?;
            let secrets = tee_vault::keystore::decrypt_keystore(
                std::path::Path::new(&ks_path),
                &passphrase,
            )
            .with_context(|| format!("decrypt keystore: {ks_path}"))?;
            tracing::info!(path = %ks_path, "loaded credentials from encrypted keystore");
            signer_private_key = secrets.signer_private_key.clone();
            funder_address     = secrets.funder_address.clone();
            api_key            = secrets.api_key.clone();
            api_secret         = secrets.api_secret.clone();
            api_passphrase     = secrets.api_passphrase.clone();
            // secrets is zeroized on drop here
        } else {
            signer_private_key = std::env::var("SIGNER_PRIVATE_KEY").unwrap_or_default();
            funder_address     = std::env::var("POLYMARKET_FUNDER_ADDRESS").unwrap_or_default();
            api_key            = std::env::var("POLYMARKET_API_KEY").unwrap_or_default();
            api_secret         = std::env::var("POLYMARKET_API_SECRET").unwrap_or_default();
            api_passphrase     = std::env::var("POLYMARKET_API_PASSPHRASE").unwrap_or_default();
        }
        let polymarket_signature_type = std::env::var("POLYMARKET_SIGNATURE_TYPE")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(0);
        let polymarket_order_nonce = std::env::var("POLYMARKET_ORDER_NONCE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let polymarket_order_expiration = std::env::var("POLYMARKET_ORDER_EXPIRATION")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let live_profile = std::env::var("BLINK_LIVE_PROFILE")
            .unwrap_or_else(|_| CANONICAL_LIVE_PROFILE.to_string());
        let live_rollout_stage = std::env::var("LIVE_ROLLOUT_STAGE")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(3);
        let live_canary_max_order_usdc = std::env::var("LIVE_CANARY_MAX_ORDER_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(20.0);
        let live_canary_max_orders_per_session = std::env::var("LIVE_CANARY_MAX_ORDERS_PER_SESSION")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let live_canary_daytime_only = std::env::var("LIVE_CANARY_DAYTIME_ONLY")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let live_canary_start_hour_utc = std::env::var("LIVE_CANARY_START_HOUR_UTC")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(8);
        let live_canary_end_hour_utc = std::env::var("LIVE_CANARY_END_HOUR_UTC")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(22);
        let live_canary_max_reject_streak = std::env::var("LIVE_CANARY_MAX_REJECT_STREAK")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3);
        let live_canary_allowed_markets = std::env::var("LIVE_CANARY_ALLOWED_MARKETS")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // When live trading is requested every credential must be present.
        if live_trading {
            for (name, val) in [
                ("SIGNER_PRIVATE_KEY", &signer_private_key),
                ("POLYMARKET_FUNDER_ADDRESS", &funder_address),
                ("POLYMARKET_API_KEY", &api_key),
                ("POLYMARKET_API_SECRET", &api_secret),
                ("POLYMARKET_API_PASSPHRASE", &api_passphrase),
            ] {
                anyhow::ensure!(!val.is_empty(), "LIVE_TRADING=true but {name} is not set");
            }
            anyhow::ensure!(
                funder_address.starts_with("0x") && funder_address.len() == 42,
                "LIVE_TRADING=true but POLYMARKET_FUNDER_ADDRESS must be 0x-prefixed 20-byte address"
            );
            anyhow::ensure!(
                polymarket_signature_type <= 2,
                "LIVE_TRADING=true but POLYMARKET_SIGNATURE_TYPE must be in [0,2]"
            );
        }

        let markets: Vec<String> = markets_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .collect();

        // In paper/copytrade mode, markets are discovered dynamically from RN1 signals.
        // Only enforce non-empty markets for live trading.
        if markets.is_empty() && live_trading {
            anyhow::bail!("MARKETS must contain at least one token ID for live trading");
        }

        Ok(Config {
            clob_host,
            ws_url,
            // Normalise to lowercase so comparisons are always case-insensitive.
            rn1_wallet: rn1_wallet_raw.to_lowercase(),
            markets,
            log_level,
            ws_reconnect_debounce_ms,
            ws_parse_error_preview_chars,
            ws_broadcast_interval_secs,
            latency_window_size,
            live_trading,
            signer_private_key,
            funder_address,
            api_key,
            api_secret,
            api_passphrase,
            polymarket_signature_type,
            polymarket_order_nonce,
            polymarket_order_expiration,
            live_profile,
            live_rollout_stage,
            live_canary_max_order_usdc,
            live_canary_max_orders_per_session,
            live_canary_daytime_only,
            live_canary_start_hour_utc,
            live_canary_end_hour_utc,
            live_canary_max_reject_streak,
            live_canary_allowed_markets,

            alpha_enabled: std::env::var("ALPHA_ENABLED")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false),
            alpha_sidecar_url: std::env::var("ALPHA_SIDECAR_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:7879".to_string()),
        })
    }

    /// Validates the canonical live profile contract.
    pub fn validate_live_profile_contract(&self) -> Result<()> {
        if !self.live_trading {
            return Ok(());
        }

        anyhow::ensure!(
            self.live_profile == CANONICAL_LIVE_PROFILE,
            "LIVE_TRADING=true requires BLINK_LIVE_PROFILE={CANONICAL_LIVE_PROFILE}"
        );
        anyhow::ensure!(
            (1..=3).contains(&self.live_rollout_stage),
            "LIVE_ROLLOUT_STAGE must be one of 1,2,3"
        );
        anyhow::ensure!(
            self.polymarket_signature_type <= 2,
            "POLYMARKET_SIGNATURE_TYPE must be in [0,2]"
        );
        anyhow::ensure!(
            self.funder_address.starts_with("0x") && self.funder_address.len() == 42,
            "POLYMARKET_FUNDER_ADDRESS must be 0x-prefixed 20-byte address"
        );
        anyhow::ensure!(
            self.ws_reconnect_debounce_ms >= 500,
            "WS_RECONNECT_DEBOUNCE_MS must be >= 500 for live profile stability"
        );
        anyhow::ensure!(
            !self.markets.is_empty(),
            "MARKETS must contain at least one token ID for live profile"
        );
        anyhow::ensure!(
            self.live_canary_max_order_usdc > 0.0,
            "LIVE_CANARY_MAX_ORDER_USDC must be > 0"
        );
        anyhow::ensure!(
            self.live_canary_start_hour_utc < 24 && self.live_canary_end_hour_utc < 24,
            "LIVE_CANARY_START_HOUR_UTC and LIVE_CANARY_END_HOUR_UTC must be in [0,23]"
        );
        anyhow::ensure!(
            self.live_canary_max_reject_streak > 0,
            "LIVE_CANARY_MAX_REJECT_STREAK must be > 0"
        );
        anyhow::ensure!(
            self.signer_private_key.starts_with("0x") && self.signer_private_key.len() == 66,
            "SIGNER_PRIVATE_KEY must be a 0x-prefixed 32-byte hex string (66 chars total)"
        );
        anyhow::ensure!(
            self.rn1_wallet.starts_with("0x") && self.rn1_wallet.len() == 42,
            "RN1_WALLET must be a 0x-prefixed 20-byte hex address (42 chars total)"
        );
        if self.polymarket_order_expiration != 0 {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            anyhow::ensure!(
                self.polymarket_order_expiration > now,
                "POLYMARKET_ORDER_EXPIRATION must be 0 or a future unix timestamp"
            );
        }
        let trading_enabled = std::env::var("TRADING_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        anyhow::ensure!(
            trading_enabled,
            "LIVE_TRADING=true requires TRADING_ENABLED=true in canonical live profile"
        );
        Ok(())
    }

    /// Validates the minimum configuration required for paper-trading mode.
    ///
    /// Returns all errors at once so the operator can fix everything in one go.
    pub fn validate_for_paper_trading(&self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();

        if self.rn1_wallet.is_empty() {
            errors.push("RN1_WALLET is not set — needed to identify the whale to track".into());
        } else if !self.rn1_wallet.starts_with("0x") || self.rn1_wallet.len() != 42 {
            errors.push(format!(
                "RN1_WALLET '{}' is not a valid 0x-prefixed 20-byte address (42 chars)",
                self.rn1_wallet
            ));
        }

        if self.clob_host.is_empty() {
            errors.push("CLOB_HOST is not set".into());
        }
        if self.ws_url.is_empty() {
            errors.push("WS_URL is not set".into());
        }

        if !errors.is_empty() {
            anyhow::bail!(
                "Paper-trading configuration errors:\n{}",
                errors.iter().map(|e| format!("  • {e}")).collect::<Vec<_>>().join("\n")
            );
        }
        Ok(())
    }
}
