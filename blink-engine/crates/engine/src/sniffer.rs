//! RN1 wallet sniffer.
//!
//! Inspects every incoming [`MarketEvent`] and emits an [`RN1Signal`] whenever
//! an `"order"` event is placed by the tracked wallet address.  
//! Address comparison is always **case-insensitive**.

use std::time::Instant;

use tracing::instrument;

use crate::types::{MarketEvent, OrderEvent, RN1Signal, parse_price};

/// Watches the order event stream for activity from a specific wallet.
pub struct Sniffer {
    /// Lowercase normalised wallet address of the target (RN1).
    rn1_wallet: String,
}

impl Sniffer {
    /// Creates a new sniffer targeting `wallet`.
    ///
    /// The address is normalised to lowercase at construction time so that
    /// hot-path comparisons are a simple equality check.
    pub fn new(wallet: &str) -> Self {
        Self {
            rn1_wallet: wallet.to_lowercase(),
        }
    }

    /// Checks a [`MarketEvent`] for an RN1 order.
    ///
    /// Returns [`Some(RN1Signal)`] if the event is an `"order"` placed by the
    /// tracked wallet; `None` for all other events or non-matching owners.
    #[instrument(skip(self, event), fields(rn1_wallet = %self.rn1_wallet))]
    pub fn check_order_event(&self, event: &MarketEvent) -> Option<RN1Signal> {
        let order = match event {
            MarketEvent::Order(o) => o,
            _ => return None,
        };

        if order.owner.to_lowercase() != self.rn1_wallet {
            return None;
        }

        Some(self.build_signal(order))
    }

    /// Constructs and logs an [`RN1Signal`] from a matching [`OrderEvent`].
    fn build_signal(&self, order: &OrderEvent) -> RN1Signal {
        let price = parse_price(&order.price);
        let size = parse_price(&order.original_size);

        tracing::warn!(
            token_id   = %order.asset_id.as_deref().unwrap_or(&order.market),
            order_id   = %order.order_id,
            owner      = %order.owner,
            side       = %order.side,
            price      = %order.price,
            size       = %order.original_size,
            order_type = %order.order_type,
            "🚨 RN1 order detected"
        );

        RN1Signal {
            token_id:    order.asset_id.clone().unwrap_or_else(|| order.market.clone()),
            market_title: None,
            market_outcome: None,
            side:        order.side,
            price,
            size,
            order_id:    order.order_id.clone(),
            detected_at: Instant::now(),
            event_start_time: None,
            event_end_time: None,
            source_wallet: self.rn1_wallet.clone(),
            wallet_weight: 1.0,
            signal_source: "rn1".to_string(),
            analysis_id: None,
        }
    }

    /// Returns the normalised wallet address this sniffer is watching.
    #[allow(dead_code)]
    #[inline]
    pub fn target_wallet(&self) -> &str {
        &self.rn1_wallet
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OrderEvent, OrderSide};

    fn make_order_event(owner: &str) -> MarketEvent {
        MarketEvent::Order(OrderEvent {
            market:        "token-abc".to_string(),
            asset_id:      None,
            order_id:      "uuid-1".to_string(),
            owner:         owner.to_string(),
            side:          OrderSide::Buy,
            price:         "0.65".to_string(),
            size_matched:  Some("0".to_string()),
            original_size: "50000".to_string(),
            order_type:    "LIMIT".to_string(),
            created_at:    None,
        })
    }

    #[test]
    fn detects_matching_wallet_case_insensitive() {
        let sniffer = Sniffer::new("0xABCDEF");
        let event = make_order_event("0xabcdef");
        assert!(sniffer.check_order_event(&event).is_some());
    }

    #[test]
    fn ignores_non_matching_wallet() {
        let sniffer = Sniffer::new("0xABCDEF");
        let event = make_order_event("0xDEADBEEF");
        assert!(sniffer.check_order_event(&event).is_none());
    }

    #[test]
    fn ignores_non_order_events() {
        use crate::types::{BookEvent, MarketEvent};
        let sniffer = Sniffer::new("0xABCDEF");
        let book = MarketEvent::Book(BookEvent {
            market:    "token-abc".to_string(),
            asset_id:  None,
            bids:      vec![],
            asks:      vec![],
            timestamp: None,
            hash:      None,
        });
        assert!(sniffer.check_order_event(&book).is_none());
    }

    #[test]
    fn signal_price_and_size_are_scaled() {
        let sniffer = Sniffer::new("0xabc");
        let event = make_order_event("0xabc");
        let signal = sniffer.check_order_event(&event).unwrap();
        assert_eq!(signal.price, 650);               // "0.65" × 1000
        assert_eq!(signal.size,  50_000 * 1_000);    // "50000" × 1000
    }
}
