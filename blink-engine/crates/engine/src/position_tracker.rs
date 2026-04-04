use crate::types::{OrderSide, Signal};
use std::collections::{HashMap, VecDeque};

/// Tracks RN1's positions to detect synthetic hedges
#[derive(Debug, Clone)]
pub struct PositionTracker {
    /// Map of market_id -> recent positions
    positions: HashMap<String, VecDeque<TrackedPosition>>,
    /// Maximum history to keep per market
    max_history: usize,
}

#[derive(Debug, Clone)]
pub struct TrackedPosition {
    pub market_id: String,
    pub token_id: String,
    pub side: OrderSide,
    pub size_usdc: f64,
    pub price: f64,
    pub timestamp: i64,
}

impl PositionTracker {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            max_history: 10, // Keep last 10 trades per market
        }
    }

    pub fn with_capacity(capacity: usize, max_history: usize) -> Self {
        Self {
            positions: HashMap::with_capacity(capacity),
            max_history,
        }
    }

    /// Record a new RN1 trade signal
    pub fn record_trade(&mut self, signal: &Signal) {
        let pos = TrackedPosition {
            market_id: signal.market_id.clone(),
            token_id: signal.token_id.clone(),
            side: signal.side,
            size_usdc: signal.size,
            price: signal.price,
            timestamp: chrono::Utc::now().timestamp(),
        };

        let entry = self
            .positions
            .entry(signal.market_id.clone())
            .or_default();

        entry.push_back(pos);

        // Trim to max history
        while entry.len() > self.max_history {
            entry.pop_front();
        }
    }

    /// Check if this signal is likely a synthetic hedge
    /// Returns true if RN1 recently took opposite side in same market
    pub fn is_hedge(&self, signal: &Signal) -> bool {
        if let Some(positions) = self.positions.get(&signal.market_id) {
            // Check last 5 positions (most recent first)
            for pos in positions.iter().rev().take(5) {
                // Must be opposite side
                if pos.side == signal.side {
                    continue;
                }

                // Check if sizes are similar (within 30% tolerance)
                let size_ratio = (pos.size_usdc - signal.size).abs() / pos.size_usdc;
                if size_ratio <= 0.30 {
                    // Check if recent (within 24 hours)
                    let age_seconds = chrono::Utc::now().timestamp() - pos.timestamp;
                    if age_seconds <= 24 * 3600 {
                        tracing::warn!(
                            "🔍 HEDGE DETECTED: market {} - Previous {} ${:.0} @ {:.3}, Now {} ${:.0} @ {:.3} ({} hours ago)",
                            signal.market_id,
                            pos.side,
                            pos.size_usdc,
                            pos.price,
                            signal.side,
                            signal.size,
                            signal.price,
                            age_seconds / 3600
                        );
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Get all positions for a market
    pub fn get_positions(&self, market_id: &str) -> Option<&VecDeque<TrackedPosition>> {
        self.positions.get(market_id)
    }

    /// Get most recent position for a market
    pub fn get_latest_position(&self, market_id: &str) -> Option<&TrackedPosition> {
        self.positions.get(market_id)?.back()
    }

    /// Clear old positions (older than cutoff_hours)
    pub fn cleanup_old_positions(&mut self, cutoff_hours: i64) {
        let cutoff_ts = chrono::Utc::now().timestamp() - (cutoff_hours * 3600);

        for positions in self.positions.values_mut() {
            positions.retain(|pos| pos.timestamp >= cutoff_ts);
        }

        // Remove empty entries
        self.positions.retain(|_, positions| !positions.is_empty());
    }

    /// Get statistics
    pub fn stats(&self) -> PositionTrackerStats {
        let total_markets = self.positions.len();
        let total_positions: usize = self.positions.values().map(|v| v.len()).sum();

        PositionTrackerStats {
            total_markets,
            total_positions,
        }
    }
}

impl Default for PositionTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct PositionTrackerStats {
    pub total_markets: usize,
    pub total_positions: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hedge_detection_opposite_side() {
        let mut tracker = PositionTracker::new();

        // Record first trade: Buy YES
        let signal1 = Signal {
            market_id: "test-market".to_string(),
            token_id: "token-123".to_string(),
            side: OrderSide::Buy,
            size: 50_000.0,
            price: 0.55,
        };
        tracker.record_trade(&signal1);

        // Second trade: Buy NO (opposite) with similar size
        let signal2 = Signal {
            market_id: "test-market".to_string(),
            token_id: "token-456".to_string(),
            side: OrderSide::Sell, // Opposite
            size: 48_000.0,        // Within 30%
            price: 0.45,
        };

        assert!(tracker.is_hedge(&signal2), "Should detect hedge");
    }

    #[test]
    fn test_no_hedge_same_side() {
        let mut tracker = PositionTracker::new();

        let signal1 = Signal {
            market_id: "test-market".to_string(),
            token_id: "token-123".to_string(),
            side: OrderSide::Buy,
            size: 50_000.0,
            price: 0.55,
        };
        tracker.record_trade(&signal1);

        // Same side - not a hedge
        let signal2 = Signal {
            market_id: "test-market".to_string(),
            token_id: "token-123".to_string(),
            side: OrderSide::Buy,
            size: 48_000.0,
            price: 0.56,
        };

        assert!(
            !tracker.is_hedge(&signal2),
            "Should not detect hedge (same side)"
        );
    }

    #[test]
    fn test_no_hedge_different_size() {
        let mut tracker = PositionTracker::new();

        let signal1 = Signal {
            market_id: "test-market".to_string(),
            token_id: "token-123".to_string(),
            side: OrderSide::Buy,
            size: 50_000.0,
            price: 0.55,
        };
        tracker.record_trade(&signal1);

        // Opposite side but very different size
        let signal2 = Signal {
            market_id: "test-market".to_string(),
            token_id: "token-456".to_string(),
            side: OrderSide::Sell,
            size: 10_000.0, // Too different (>30%)
            price: 0.45,
        };

        assert!(
            !tracker.is_hedge(&signal2),
            "Should not detect hedge (size mismatch)"
        );
    }
}
