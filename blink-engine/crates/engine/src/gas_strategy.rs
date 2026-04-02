//! EIP-1559 dynamic priority fee calculator for Polygon transactions.
//!
//! Computes an optimal priority fee based on expected trade profit, gas
//! limits, and the current base fee from [`crate::gas_oracle::GasOracle`].
//!
//! # Formula
//!
//! ```text
//! Optimal_Priority_Fee = (Expected_Trade_Profit × 0.10) / Gas_Limit
//! ```
//!
//! The result is clamped between a hard floor (30 gwei) and a hard ceiling
//! (500 gwei), then adjusted upward if the oracle-reported base fee suggests
//! network congestion.

use std::sync::Arc;

use tracing::{debug, info};

use crate::gas_oracle::GasOracle;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Hard floor for priority fee (gwei).
const PRIORITY_FEE_FLOOR_GWEI: u64 = 30;

/// Hard ceiling for priority fee (gwei).
const PRIORITY_FEE_CEILING_GWEI: u64 = 500;

/// Fraction of expected profit allocated to gas (10%).
const PROFIT_GAS_FRACTION: f64 = 0.10;

/// Approximate MATIC price in USD for conversion (conservative estimate).
/// In production this would be fetched from a price feed.
const MATIC_PRICE_USD: f64 = 0.50;

/// 1 gwei = 1e-9 MATIC.
const GWEI_TO_MATIC: f64 = 1e-9;

// ─── GasStrategy ─────────────────────────────────────────────────────────────

/// EIP-1559 dynamic fee calculator.
///
/// Combines profit-based fee estimation with live base-fee data from the
/// [`GasOracle`] to produce optimal priority fees that balance inclusion
/// speed against cost.
pub struct GasStrategy {
    oracle: Arc<GasOracle>,
}

impl GasStrategy {
    /// Creates a new strategy backed by the given gas oracle.
    pub fn new(oracle: Arc<GasOracle>) -> Self {
        info!("GasStrategy initialised (floor={PRIORITY_FEE_FLOOR_GWEI}, ceiling={PRIORITY_FEE_CEILING_GWEI})");
        Self { oracle }
    }

    /// Calculate the optimal priority fee in gwei.
    ///
    /// # Arguments
    /// * `expected_profit_usdc` — Expected profit from the trade in USD.
    /// * `gas_limit`            — Gas limit for the transaction.
    ///
    /// # Returns
    /// Priority fee in gwei, clamped to `[30, 500]`.
    pub async fn calc_priority_fee(&self, expected_profit_usdc: f64, gas_limit: u64) -> u64 {
        if gas_limit == 0 {
            debug!("gas_limit is 0 — returning floor fee");
            return PRIORITY_FEE_FLOOR_GWEI;
        }

        // 1. Profit-based component: allocate 10% of profit to gas.
        let gas_budget_usdc = expected_profit_usdc * PROFIT_GAS_FRACTION;

        // Convert USD gas budget → MATIC → gwei.
        let gas_budget_matic = gas_budget_usdc / MATIC_PRICE_USD;
        let gas_budget_gwei = gas_budget_matic / GWEI_TO_MATIC;

        // Divide by gas_limit to get per-unit priority fee.
        let profit_fee_gwei = (gas_budget_gwei / gas_limit as f64) as u64;

        // 2. Base-fee congestion component from the oracle.
        let oracle_fee = self.oracle.suggest_priority_fee_gwei().await;

        // Take the higher of profit-based and oracle-based fees to ensure
        // competitive inclusion during congestion.
        let raw_fee = profit_fee_gwei.max(oracle_fee);

        // 3. Clamp to [floor, ceiling].
        let clamped = raw_fee.clamp(PRIORITY_FEE_FLOOR_GWEI, PRIORITY_FEE_CEILING_GWEI);

        debug!(
            expected_profit_usdc,
            gas_limit,
            profit_fee_gwei,
            oracle_fee,
            clamped,
            "GasStrategy fee calculated"
        );

        clamped
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_oracle() -> Arc<GasOracle> {
        Arc::new(GasOracle::new(None))
    }

    #[tokio::test]
    async fn floor_enforced_on_zero_profit() {
        let strategy = GasStrategy::new(test_oracle());
        let fee = strategy.calc_priority_fee(0.0, 200_000).await;
        assert_eq!(fee, PRIORITY_FEE_FLOOR_GWEI);
    }

    #[tokio::test]
    async fn floor_enforced_on_zero_gas_limit() {
        let strategy = GasStrategy::new(test_oracle());
        let fee = strategy.calc_priority_fee(100.0, 0).await;
        assert_eq!(fee, PRIORITY_FEE_FLOOR_GWEI);
    }

    #[tokio::test]
    async fn ceiling_enforced_on_huge_profit() {
        let strategy = GasStrategy::new(test_oracle());
        // Enormous profit should cap at ceiling.
        let fee = strategy.calc_priority_fee(1_000_000.0, 21_000).await;
        assert_eq!(fee, PRIORITY_FEE_CEILING_GWEI);
    }

    #[tokio::test]
    async fn moderate_profit_gives_reasonable_fee() {
        let strategy = GasStrategy::new(test_oracle());
        let fee = strategy.calc_priority_fee(5.0, 200_000).await;
        assert!(fee >= PRIORITY_FEE_FLOOR_GWEI);
        assert!(fee <= PRIORITY_FEE_CEILING_GWEI);
    }

    #[test]
    fn constants_are_sane() {
        assert!(PRIORITY_FEE_FLOOR_GWEI < PRIORITY_FEE_CEILING_GWEI);
        assert!(PROFIT_GAS_FRACTION > 0.0 && PROFIT_GAS_FRACTION < 1.0);
    }

    #[tokio::test]
    async fn higher_profit_yields_higher_or_equal_fee() {
        let strategy = GasStrategy::new(test_oracle());
        let fee_low = strategy.calc_priority_fee(1.0, 200_000).await;
        let fee_high = strategy.calc_priority_fee(50.0, 200_000).await;
        assert!(fee_high >= fee_low);
    }

    #[tokio::test]
    async fn lower_gas_limit_yields_higher_or_equal_fee() {
        let strategy = GasStrategy::new(test_oracle());
        let fee_high_gas = strategy.calc_priority_fee(10.0, 500_000).await;
        let fee_low_gas = strategy.calc_priority_fee(10.0, 50_000).await;
        assert!(fee_low_gas >= fee_high_gas);
    }
}
