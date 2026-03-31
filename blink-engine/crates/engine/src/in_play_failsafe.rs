//! In-play failsafe — 3-second price-drift detection with cancel.
//!
//! During the 3-second countdown before submitting a live order, this module
//! polls the CLOB `/price` endpoint every 100 ms.  If the implied probability
//! shifts by more than `PRICE_DRIFT_ABORT_BPS` (default 150 = 1.5%), the
//! order is aborted immediately.
//!
//! This protects against submitting stale orders into a rapidly moving
//! in-play market.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::clob_client::ClobClient;
use crate::types::OrderSide;

// ─── Configuration ──────────────────────────────────────────────────────────

/// Default drift threshold in basis points (150 = 1.5%).
const DEFAULT_DRIFT_ABORT_BPS: u32 = 150;

/// Default poll interval during the countdown.
const POLL_INTERVAL_MS: u64 = 100;

/// Default countdown duration.
const COUNTDOWN_SECS: u64 = 3;

/// Result of running the in-play failsafe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailsafeResult {
    /// Price remained stable — safe to proceed.
    Stable,
    /// Price drifted beyond the threshold — order must be cancelled.
    DriftAbort { drift_bps: u32 },
    /// Could not fetch the price (network error etc.) — conservative abort.
    FetchError,
}

/// Configuration for the in-play failsafe, loaded from environment.
#[derive(Debug, Clone)]
pub struct InPlayFailsafeConfig {
    /// Maximum allowed price drift in basis points before aborting.
    pub drift_abort_bps: u32,
    /// How often to poll the price endpoint.
    pub poll_interval: Duration,
    /// Total countdown duration.
    pub countdown: Duration,
}

impl InPlayFailsafeConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let drift_abort_bps = std::env::var("PRICE_DRIFT_ABORT_BPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_DRIFT_ABORT_BPS);

        let poll_interval_ms = std::env::var("FAILSAFE_POLL_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(POLL_INTERVAL_MS);

        let countdown_secs = std::env::var("FAILSAFE_COUNTDOWN_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(COUNTDOWN_SECS);

        Self {
            drift_abort_bps,
            poll_interval: Duration::from_millis(poll_interval_ms),
            countdown: Duration::from_secs(countdown_secs),
        }
    }
}

impl Default for InPlayFailsafeConfig {
    fn default() -> Self {
        Self {
            drift_abort_bps: DEFAULT_DRIFT_ABORT_BPS,
            poll_interval: Duration::from_millis(POLL_INTERVAL_MS),
            countdown: Duration::from_secs(COUNTDOWN_SECS),
        }
    }
}

// ─── Failsafe runner ────────────────────────────────────────────────────────

/// Runs the in-play failsafe countdown for a given token.
///
/// Polls the CLOB `/price` endpoint every `config.poll_interval` for the
/// duration of `config.countdown`.  If the price drifts by more than
/// `config.drift_abort_bps` from `anchor_price`, returns
/// [`FailsafeResult::DriftAbort`].
///
/// `anchor_price` is the price (as a float, e.g. 0.65) at the moment the
/// order signal was detected.
pub async fn run_in_play_failsafe(
    clob: &Arc<ClobClient>,
    token_id: &str,
    side: OrderSide,
    anchor_price: f64,
    config: &InPlayFailsafeConfig,
) -> FailsafeResult {
    let start = Instant::now();
    let threshold = config.drift_abort_bps as f64 / 10_000.0;
    let mut polls: u32 = 0;

    info!(
        token_id,
        anchor_price,
        drift_abort_bps = config.drift_abort_bps,
        countdown_ms = config.countdown.as_millis(),
        "In-play failsafe countdown started"
    );

    while start.elapsed() < config.countdown {
        tokio::time::sleep(config.poll_interval).await;
        polls += 1;

        let price_result = clob.get_price(token_id, side).await;
        match price_result {
            Ok(price_str) => {
                let current: f64 = match price_str.parse() {
                    Ok(p) => p,
                    Err(_) => {
                        warn!(
                            token_id,
                            price_str,
                            "failsafe: unparseable price — aborting"
                        );
                        return FailsafeResult::FetchError;
                    }
                };

                if anchor_price == 0.0 {
                    continue;
                }

                let drift = (current - anchor_price).abs() / anchor_price;
                let drift_bps = (drift * 10_000.0) as u32;

                if drift > threshold {
                    warn!(
                        token_id,
                        anchor_price,
                        current,
                        drift_bps,
                        polls,
                        "🚨 IN-PLAY FAILSAFE: price drift exceeded threshold — ABORTING"
                    );
                    return FailsafeResult::DriftAbort { drift_bps };
                }
            }
            Err(e) => {
                warn!(
                    token_id,
                    error = %e,
                    polls,
                    "failsafe: price fetch failed — conservative abort"
                );
                return FailsafeResult::FetchError;
            }
        }
    }

    info!(
        token_id,
        polls,
        elapsed_ms = start.elapsed().as_millis(),
        "In-play failsafe passed — price stable"
    );
    FailsafeResult::Stable
}

/// Synchronous (non-network) drift check against a locally available price.
///
/// Returns `true` if the price has drifted beyond the threshold.
pub fn check_drift(anchor_price: f64, current_price: f64, drift_abort_bps: u32) -> bool {
    if anchor_price == 0.0 {
        return false;
    }
    let drift = (current_price - anchor_price).abs() / anchor_price;
    drift > (drift_abort_bps as f64 / 10_000.0)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_drift_within_threshold() {
        assert!(!check_drift(0.65, 0.655, 150)); // ~0.77% < 1.5%
    }

    #[test]
    fn check_drift_exceeds_threshold() {
        assert!(check_drift(0.65, 0.67, 150)); // ~3.08% > 1.5%
    }

    #[test]
    fn check_drift_exact_threshold() {
        // 1.5% of 0.65 = 0.00975 → 0.65 + 0.00975 = 0.65975
        // Just above: 0.6598 should trigger
        assert!(check_drift(0.65, 0.6598, 150));
    }

    #[test]
    fn check_drift_negative_direction() {
        assert!(check_drift(0.65, 0.63, 150)); // ~3.08% drift downward
    }

    #[test]
    fn check_drift_zero_anchor() {
        assert!(!check_drift(0.0, 0.5, 150));
    }

    #[test]
    fn failsafe_config_defaults() {
        let cfg = InPlayFailsafeConfig::default();
        assert_eq!(cfg.drift_abort_bps, 150);
        assert_eq!(cfg.poll_interval, Duration::from_millis(100));
        assert_eq!(cfg.countdown, Duration::from_secs(3));
    }
}

// ─── Property-based tests ───────────────────────────────────────────────────

#[cfg(test)]
mod proptest_failsafe {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(5_000))]

        /// Invariant: if |current - anchor| / anchor > threshold, check_drift returns true.
        #[test]
        fn drift_detection_correct(
            anchor in 0.01f64..1.0f64,
            offset_bps in 0u32..500u32,
            threshold_bps in 1u32..300u32,
        ) {
            let offset = anchor * (offset_bps as f64 / 10_000.0);
            let current = anchor + offset;
            let result = check_drift(anchor, current, threshold_bps);

            if offset_bps > threshold_bps {
                prop_assert!(result, "drift {offset_bps}bps should exceed threshold {threshold_bps}bps");
            }
            // Note: at exactly the threshold, floating point rounding can go either way.
        }
    }
}
