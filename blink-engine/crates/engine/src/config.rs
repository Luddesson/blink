//! Configuration management for the Blink Engine.

use anyhow::{Context, Result};
use std::str::FromStr;

use crate::strategy::StrategyMode;

/// The canonical profile contract address for live trading.
pub const CANONICAL_LIVE_PROFILE: &str = "canonical-v1";

#[derive(Debug, Clone)]
pub struct Config {
    pub clob_host: String,
    pub ws_url: String,
    pub rn1_wallet: String,
    pub markets: Vec<String>,
    pub log_level: String,
    pub ws_reconnect_debounce_ms: u64,
    pub ws_parse_error_preview_chars: usize,
    pub ws_broadcast_interval_secs: u64,
    pub latency_window_size: usize,

    // Live trading credentials
    pub live_trading: bool,
    pub signer_private_key: String,
    pub funder_address: String,
    pub api_key: String,
    pub api_secret: String,
    pub api_passphrase: String,
    pub polymarket_signature_type: u8,
    pub polymarket_order_nonce: u64,
    pub polymarket_order_expiration: u64,

    pub live_profile: String,
    pub live_rollout_stage: u8,
    pub live_canary_max_order_usdc: f64,
    pub live_canary_max_orders_per_session: usize,
    pub live_canary_daytime_only: bool,
    pub live_canary_start_hour_utc: u8,
    pub live_canary_end_hour_utc: u8,
    pub live_canary_max_reject_streak: usize,
    pub live_canary_max_loss_streak: usize,
    pub live_canary_allowed_markets: Vec<String>,

    pub alpha_enabled: bool,
    pub alpha_sidecar_url: String,

    pub strategy_mode: StrategyMode,
    pub strategy_mode_explicit_env: bool,
    pub strategy_runtime_switch: bool,
    pub strategy_live_switch_allowed: bool,
    pub strategy_switch_cooldown_secs: u64,
    pub strategy_require_reason: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let clob_host =
            std::env::var("CLOB_HOST").unwrap_or_else(|_| "https://clob.polymarket.com".into());
        let ws_url = std::env::var("WS_URL")
            .unwrap_or_else(|_| "wss://ws-subscriptions-clob.polymarket.com/ws/market".into());
        let rn1_wallet_raw =
            std::env::var("RN1_WALLET").context("RN1_WALLET environment variable not set")?;
        let markets_str = std::env::var("MARKETS").unwrap_or_default();
        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".into());
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
            .unwrap_or(30);
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

        // Prefer explicit env overrides first so operators can retarget the
        // live funder without rewriting the encrypted keystore.
        let env_funder_address = std::env::var("POLYMARKET_FUNDER_ADDRESS").ok();
        let env_signer_private_key = std::env::var("SIGNER_PRIVATE_KEY").ok();
        let env_api_key = std::env::var("POLYMARKET_API_KEY").ok();
        let env_api_secret = std::env::var("POLYMARKET_API_SECRET").ok();
        let env_api_passphrase = std::env::var("POLYMARKET_API_PASSPHRASE").ok();

        // Try encrypted keystore first, but let explicit env vars override any
        // field that is present. This keeps keystore convenience without
        // forcing a re-encrypt for simple address changes.
        if let Ok(ks_path) = std::env::var("KEYSTORE_PATH") {
            let passphrase = std::env::var("KEYSTORE_PASSPHRASE")
                .context("KEYSTORE_PATH set but KEYSTORE_PASSPHRASE missing")?;
            let secrets =
                tee_vault::keystore::decrypt_keystore(std::path::Path::new(&ks_path), &passphrase)
                    .with_context(|| format!("decrypt keystore: {ks_path}"))?;
            tracing::info!(path = %ks_path, "loaded credentials from encrypted keystore");
            signer_private_key =
                env_signer_private_key.unwrap_or_else(|| secrets.signer_private_key.clone());
            funder_address = env_funder_address.unwrap_or_else(|| secrets.funder_address.clone());
            api_key = env_api_key.unwrap_or_else(|| secrets.api_key.clone());
            api_secret = env_api_secret.unwrap_or_else(|| secrets.api_secret.clone());
            api_passphrase = env_api_passphrase.unwrap_or_else(|| secrets.api_passphrase.clone());
            // secrets is zeroized on drop here
        } else {
            signer_private_key = env_signer_private_key.unwrap_or_default();
            funder_address = env_funder_address.unwrap_or_default();
            api_key = env_api_key.unwrap_or_default();
            api_secret = env_api_secret.unwrap_or_default();
            api_passphrase = env_api_passphrase.unwrap_or_default();
        }

        let polymarket_signature_type_raw = std::env::var("POLYMARKET_SIGNATURE_TYPE").ok();
        let polymarket_signature_type = polymarket_signature_type_raw
            .as_deref()
            .map(|v| v.parse::<u8>())
            .transpose()
            .context("POLYMARKET_SIGNATURE_TYPE must be an integer in [0,2]")?
            .unwrap_or(0);
        let polymarket_order_nonce = std::env::var("POLYMARKET_ORDER_NONCE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let polymarket_order_expiration = std::env::var("POLYMARKET_ORDER_EXPIRATION")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        let live_profile = std::env::var("BLINK_LIVE_PROFILE").unwrap_or_else(|_| "paper".into());
        let live_rollout_stage = std::env::var("LIVE_ROLLOUT_STAGE")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(1);
        let live_canary_max_order_usdc = std::env::var("LIVE_CANARY_MAX_ORDER_USDC")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(5.0);
        let live_canary_max_orders_per_session =
            std::env::var("LIVE_CANARY_MAX_ORDERS_PER_SESSION")
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
        let live_canary_max_loss_streak = std::env::var("LIVE_CANARY_MAX_LOSS_STREAK")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let live_canary_allowed_markets = std::env::var("LIVE_CANARY_ALLOWED_MARKETS")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let strategy_mode_raw = std::env::var("BLINK_STRATEGY_MODE").ok();
        let strategy_mode_explicit_env = strategy_mode_raw.is_some();
        let strategy_mode = strategy_mode_raw
            .as_deref()
            .map(StrategyMode::from_str)
            .transpose()
            .map_err(|e| anyhow::anyhow!("BLINK_STRATEGY_MODE: {e}"))?
            .unwrap_or_default();
        let strategy_runtime_switch = std::env::var("BLINK_STRATEGY_RUNTIME_SWITCH")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let strategy_live_switch_allowed = std::env::var("BLINK_STRATEGY_LIVE_SWITCH_ALLOWED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let strategy_switch_cooldown_secs = std::env::var("BLINK_STRATEGY_SWITCH_COOLDOWN_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);
        let strategy_require_reason = std::env::var("BLINK_STRATEGY_REQUIRE_REASON")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);

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
                polymarket_signature_type_raw.is_some(),
                "LIVE_TRADING=true requires explicit POLYMARKET_SIGNATURE_TYPE (0=EOA, 1=proxy/safe, 2=browser wallet)"
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
        // We allow this in live mode too for parity with paper execution logic.
        if markets.is_empty() && live_trading {
            tracing::warn!(
                "MARKETS is empty: starting in dynamic discovery mode (shadowing signals)"
            );
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
            live_canary_max_loss_streak,
            live_canary_allowed_markets,

            alpha_enabled: std::env::var("ALPHA_ENABLED")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false),
            alpha_sidecar_url: std::env::var("ALPHA_SIDECAR_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:7879".to_string()),
            strategy_mode,
            strategy_mode_explicit_env,
            strategy_runtime_switch,
            strategy_live_switch_allowed,
            strategy_switch_cooldown_secs,
            strategy_require_reason,
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
            self.live_canary_max_loss_streak > 0,
            "LIVE_CANARY_MAX_LOSS_STREAK must be > 0"
        );
        Ok(())
    }

    pub fn validate_for_paper_trading(&self) -> Result<()> {
        anyhow::ensure!(
            !self.rn1_wallet.is_empty(),
            "PAPER_TRADING requires RN1_WALLET to be set"
        );
        Ok(())
    }
}
