//! MEV-Share sandwich evasion and deadline enforcement.
//!
//! [`MevShield`] wraps outgoing transactions with timing-based staleness
//! checks and sandwich-attack detection before they are sent to the
//! [`crate::tx_router::TxRouter`].
//!
//! # Sandwich detection
//!
//! If the same token appears in more than 2 pending submissions within the
//! last 500 ms, the shield flags a sandwich risk and skips submission.
//!
//! # Staleness
//!
//! Bundles older than ~5 seconds (2 Polygon blocks at ~2.1 s each) are
//! considered stale and are rejected.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Maximum age before a bundle is considered stale (~2 Polygon blocks).
const BUNDLE_MAX_AGE: Duration = Duration::from_secs(5);

/// Sliding window for sandwich detection.
const SANDWICH_WINDOW: Duration = Duration::from_millis(500);

/// Threshold: if a token has more than this many pending txs in the window,
/// flag as sandwich risk.
const SANDWICH_TX_THRESHOLD: usize = 2;

/// Default deadline offset added to current timestamp (seconds).
/// Polygon blocks are ~2.1 s; 12 s ≈ ~6 blocks of slack.
const DEADLINE_OFFSET_SECS: u64 = 12;

// ─── MevShield ───────────────────────────────────────────────────────────────

/// MEV protection layer that screens outgoing transactions.
pub struct MevShield {
    /// Recent pending submissions keyed by token ID, storing timestamps.
    recent_submissions: HashMap<String, Vec<Instant>>,
}

impl MevShield {
    /// Creates a new shield instance.
    pub fn new() -> Self {
        info!("MevShield initialised — sandwich detection + deadline enforcement active");
        Self {
            recent_submissions: HashMap::new(),
        }
    }

    /// Check whether a bundle created at `submitted_at` is stale.
    ///
    /// Returns `true` if the bundle is older than [`BUNDLE_MAX_AGE`] (~5 s).
    pub fn is_bundle_stale(&self, submitted_at: Instant) -> bool {
        let stale = submitted_at.elapsed() > BUNDLE_MAX_AGE;
        if stale {
            warn!(
                age_ms = submitted_at.elapsed().as_millis() as u64,
                "bundle is stale (>{} ms) — skipping submission",
                BUNDLE_MAX_AGE.as_millis()
            );
        }
        stale
    }

    /// Calculate a block.timestamp deadline for a transaction.
    ///
    /// Returns `current_timestamp + 12 seconds`.
    pub fn calculate_deadline(&self) -> u64 {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let deadline = now_secs + DEADLINE_OFFSET_SECS;
        debug!(now_secs, deadline, "calculated tx deadline");
        deadline
    }

    /// Check if submitting a transaction for `token_id` would be a sandwich
    /// risk.
    ///
    /// Returns `true` if it is safe to proceed, `false` if the submission
    /// should be skipped due to sandwich risk.
    pub fn check_sandwich_risk(&mut self, token_id: &str) -> bool {
        let now = Instant::now();

        // Prune stale entries outside the sliding window.
        let timestamps = self
            .recent_submissions
            .entry(token_id.to_string())
            .or_default();
        timestamps.retain(|ts| now.duration_since(*ts) < SANDWICH_WINDOW);

        // Check threshold before recording this submission.
        if timestamps.len() >= SANDWICH_TX_THRESHOLD {
            warn!(
                token_id,
                pending_count = timestamps.len(),
                "🥪 SANDWICH RISK: {} pending txs for token in last {}ms — skipping submission",
                timestamps.len(),
                SANDWICH_WINDOW.as_millis()
            );
            return false;
        }

        // Record this submission.
        timestamps.push(now);
        debug!(
            token_id,
            pending = timestamps.len(),
            "MevShield: submission recorded, no sandwich risk"
        );
        true
    }

    /// Full pre-submission screening.
    ///
    /// Runs staleness check + sandwich detection. Returns `true` if the
    /// transaction is safe to submit.
    pub fn screen_submission(&mut self, token_id: &str, submitted_at: Instant) -> bool {
        // 1. Staleness check.
        if self.is_bundle_stale(submitted_at) {
            return false;
        }

        // 2. Sandwich detection.
        if !self.check_sandwich_risk(token_id) {
            return false;
        }

        true
    }

    /// Build MEV-Share hints for a private transaction.
    ///
    /// Returns a minimal hint payload that reveals only the contract address
    /// (required for MEV-Share matchmaking) but hides calldata and value.
    pub fn build_mev_share_hints(&self, contract_address: &str) -> MevShareHints {
        MevShareHints {
            contract_address: contract_address.to_string(),
            function_selector: None,
            logs: false,
            calldata: false,
            tx_hash: true,
        }
    }
}

impl Default for MevShield {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal MEV-Share hint set for private transaction submission.
#[derive(Debug, Clone)]
pub struct MevShareHints {
    /// Target contract address (publicly visible).
    pub contract_address: String,
    /// Optional 4-byte function selector (hidden by default).
    pub function_selector: Option<String>,
    /// Whether to reveal log topics (hidden by default).
    pub logs: bool,
    /// Whether to reveal calldata (hidden by default).
    pub calldata: bool,
    /// Whether to reveal the tx hash (visible by default).
    pub tx_hash: bool,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn fresh_bundle_is_not_stale() {
        let shield = MevShield::new();
        let now = Instant::now();
        assert!(!shield.is_bundle_stale(now));
    }

    #[test]
    fn old_bundle_is_stale() {
        let shield = MevShield::new();
        let old = Instant::now() - Duration::from_secs(10);
        assert!(shield.is_bundle_stale(old));
    }

    #[test]
    fn deadline_is_in_the_future() {
        let shield = MevShield::new();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let deadline = shield.calculate_deadline();
        assert!(deadline >= now_secs + DEADLINE_OFFSET_SECS);
        assert!(deadline <= now_secs + DEADLINE_OFFSET_SECS + 1);
    }

    #[test]
    fn first_two_submissions_pass_sandwich_check() {
        let mut shield = MevShield::new();
        let token = "token_abc";

        assert!(shield.check_sandwich_risk(token)); // 1st — ok
        assert!(shield.check_sandwich_risk(token)); // 2nd — ok
    }

    #[test]
    fn third_submission_triggers_sandwich_risk() {
        let mut shield = MevShield::new();
        let token = "token_abc";

        assert!(shield.check_sandwich_risk(token)); // 1st
        assert!(shield.check_sandwich_risk(token)); // 2nd
        assert!(!shield.check_sandwich_risk(token)); // 3rd — blocked
    }

    #[test]
    fn sandwich_window_expires() {
        let mut shield = MevShield::new();
        let token = "token_abc";

        assert!(shield.check_sandwich_risk(token));
        assert!(shield.check_sandwich_risk(token));

        // Sleep past the sandwich window so entries expire.
        thread::sleep(Duration::from_millis(600));

        // Should be safe again after window expiry.
        assert!(shield.check_sandwich_risk(token));
    }

    #[test]
    fn different_tokens_are_independent() {
        let mut shield = MevShield::new();

        assert!(shield.check_sandwich_risk("token_a"));
        assert!(shield.check_sandwich_risk("token_a"));
        // token_a is at threshold, but token_b is fresh.
        assert!(shield.check_sandwich_risk("token_b"));
        assert!(shield.check_sandwich_risk("token_b"));
    }

    #[test]
    fn screen_submission_blocks_stale_bundle() {
        let mut shield = MevShield::new();
        let old = Instant::now() - Duration::from_secs(10);
        assert!(!shield.screen_submission("token_x", old));
    }

    #[test]
    fn screen_submission_passes_fresh_bundle() {
        let mut shield = MevShield::new();
        let now = Instant::now();
        assert!(shield.screen_submission("token_x", now));
    }

    #[test]
    fn mev_share_hints_hide_calldata_by_default() {
        let shield = MevShield::new();
        let hints = shield.build_mev_share_hints("0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E");

        assert!(!hints.calldata);
        assert!(!hints.logs);
        assert!(hints.tx_hash);
        assert!(hints.function_selector.is_none());
    }
}
