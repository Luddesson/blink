//! # Single Source of Truth (SSOT) Reconciliation
//!
//! **Core principle: the exchange is always ground truth.**
//! Local state (portfolio, risk manager) is never updated until the exchange
//! confirms the actual fill via `GET /order/{id}`.
//!
//! ## Architecture
//!
//! ```text
//! Exchange (Polymarket CLOB)
//!     ↓  GET /order/{id}  →  size_matched, status
//! process_order_status()       ← canonical reconciliation logic
//!     ↓  ReconciliationOutcome::Fill { actual_size_usdc, ... }
//! LiveEngine                   ← only NOW updates portfolio + risk manager
//! ```
//!
//! ## Flow
//!
//! 1. Order submitted → `PendingOrder` inserted with `lifecycle = AwaitingConfirmation`.  
//!    **No fill recorded yet.**
//! 2. Reconciliation worker polls `GET /order/{id}` every N seconds.
//! 3. On `matched` / `filled`: extract `size_matched` from exchange response,
//!    return `ReconciliationOutcome::Fill` with *actual* amounts.
//! 4. Caller records fill using those actual amounts — never the expected ones.
//! 5. On `rejected` / `cancelled` / `expired`: return `ReconciliationOutcome::NoFill`.
//!    Caller does **not** record any fill.
//! 6. `detect_position_drift` compares local vs exchange snapshots and emits
//!    `DriftEvent` alerts whenever divergence exceeds the threshold.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::order_executor::OrderStatus;
use crate::types::OrderSide;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Orders still in `AwaitingConfirmation` after this many seconds are flagged
/// as suspected stale and an operator alert is emitted.
const MAX_PENDING_AGE_SECS: u64 = 300;

/// Position drift alert threshold: emit a `DriftEvent` when the absolute
/// difference between local and exchange size exceeds this fraction of the
/// larger of the two values.
const DRIFT_ALERT_THRESHOLD_PCT: f64 = 5.0; // 5 %

// ─── Fill Lifecycle ───────────────────────────────────────────────────────────

/// Tracks where a submitted order is in its exchange lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FillLifecycle {
    /// Sent to exchange; exchange acknowledged (order_id received) but the
    /// fill has not yet been confirmed — **no local state has been recorded**.
    AwaitingConfirmation,

    /// Exchange confirmed the fill.  Local portfolio/risk state has been
    /// updated using these exchange-confirmed amounts.
    Confirmed {
        actual_size_usdc:   f64,
        actual_size_shares: f64,
    },

    /// Order ended without a fill (rejected / cancelled / expired).
    /// **No local state was recorded.**
    NoFill { reason: String },
}

// ─── Pending Order ────────────────────────────────────────────────────────────

/// Serializable snapshot of a [`PendingOrder`] for WAL persistence.
///
/// `Instant` is not serializable, so `submitted_at_unix_secs` (Unix timestamp)
/// is stored instead.  On restore, `submitted_at` is set to `Instant::now()`
/// so age-based stale detection counts from process restart — conservative and
/// correct (the order will be re-queried immediately on the next reconcile pass).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingOrderWal {
    pub exchange_order_id:    String,
    pub token_id:             String,
    pub side:                 OrderSide,
    pub expected_size_usdc:   f64,
    pub expected_size_shares: f64,
    pub submitted_price:      f64,
    pub submitted_at_unix_secs: u64,
    pub lifecycle:            FillLifecycle,
    pub check_count:          u32,
}

impl From<&PendingOrder> for PendingOrderWal {
    fn from(p: &PendingOrder) -> Self {
        let submitted_at_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(p.submitted_at.elapsed().as_secs());
        Self {
            exchange_order_id:    p.exchange_order_id.clone(),
            token_id:             p.token_id.clone(),
            side:                 p.side,
            expected_size_usdc:   p.expected_size_usdc,
            expected_size_shares: p.expected_size_shares,
            submitted_price:      p.submitted_price,
            submitted_at_unix_secs,
            lifecycle:            p.lifecycle.clone(),
            check_count:          p.check_count,
        }
    }
}

impl From<PendingOrderWal> for PendingOrder {
    fn from(w: PendingOrderWal) -> Self {
        Self {
            exchange_order_id:    w.exchange_order_id,
            token_id:             w.token_id,
            side:                 w.side,
            expected_size_usdc:   w.expected_size_usdc,
            expected_size_shares: w.expected_size_shares,
            submitted_price:      w.submitted_price,
            submitted_at:         Instant::now(), // restart age from now — conservative
            lifecycle:            w.lifecycle,
            check_count:          w.check_count,
        }
    }
}

/// An order submitted to the exchange that is awaiting reconciliation.
///
/// Created when a live order is accepted by the exchange; removed once the
/// reconciliation worker reaches a terminal outcome.
#[derive(Debug, Clone)]
pub struct PendingOrder {
    /// Exchange-assigned order ID.
    pub exchange_order_id: String,
    pub token_id: String,
    pub side: OrderSide,
    /// USDC amount *expected* at submission time (used for drift comparison).
    pub expected_size_usdc: f64,
    /// Share count expected at submission time.
    pub expected_size_shares: f64,
    /// Limit price used when building the order.
    pub submitted_price: f64,
    /// Wall-clock instant when the order was submitted.
    pub submitted_at: Instant,
    /// Current lifecycle state — updated in place by `process_order_status`.
    pub lifecycle: FillLifecycle,
    /// Number of reconciliation passes that have inspected this order.
    pub check_count: u32,
}

impl PendingOrder {
    /// Create a new pending order in `AwaitingConfirmation` state.
    pub fn new(
        exchange_order_id: String,
        token_id: String,
        side: OrderSide,
        expected_size_usdc: f64,
        submitted_price: f64,
    ) -> Self {
        let expected_size_shares = if submitted_price > 0.0 {
            expected_size_usdc / submitted_price
        } else {
            0.0
        };
        Self {
            exchange_order_id,
            token_id,
            side,
            expected_size_usdc,
            expected_size_shares,
            submitted_price,
            submitted_at: Instant::now(),
            lifecycle: FillLifecycle::AwaitingConfirmation,
            check_count: 0,
        }
    }

    /// Returns `true` once a terminal outcome has been reached.
    pub fn is_terminal(&self) -> bool {
        !matches!(self.lifecycle, FillLifecycle::AwaitingConfirmation)
    }
}

// ─── Reconciliation Outcome ───────────────────────────────────────────────────

/// What the reconciliation pass determined for a single pending order.
///
/// The caller (`LiveEngine`) must act on this outcome.
#[derive(Debug, Clone)]
pub enum ReconciliationOutcome {
    /// Exchange confirmed a fill — update local state with these amounts.
    Fill {
        token_id: String,
        side: OrderSide,
        /// Actual USDC cost derived from `size_matched × submitted_price`.
        actual_size_usdc: f64,
        /// Actual shares filled, as reported by the exchange.
        actual_size_shares: f64,
        /// Price used when the order was built.
        submitted_price: f64,
        /// True when `size_matched < expected_size` (partial fill).
        partial_fill: bool,
        /// `actual_size_usdc / expected_size_usdc` — 1.0 = full fill.
        fill_ratio: f64,
    },

    /// Exchange did not fill the order — record **nothing** locally.
    NoFill { token_id: String, reason: String },

    /// Order is still live/pending — try again next pass.
    StillPending,

    /// Order has been pending for an unusually long time; operator should
    /// investigate whether the order is truly outstanding on the exchange.
    SuspectedStale { elapsed_secs: u64 },
}

// ─── Core Reconciliation Logic ────────────────────────────────────────────────

/// Process a `GET /order/{id}` exchange response for a single pending order.
///
/// Updates `pending.lifecycle` in place and returns a [`ReconciliationOutcome`]
/// that the caller must act on.  This is the **only** function that should
/// decide whether a local fill should be recorded.
pub fn process_order_status(
    pending: &mut PendingOrder,
    status: &OrderStatus,
) -> ReconciliationOutcome {
    pending.check_count += 1;

    // Stale-order guard — alert operator if order lingers without resolution.
    let elapsed = pending.submitted_at.elapsed().as_secs();
    if elapsed > MAX_PENDING_AGE_SECS
        && matches!(pending.lifecycle, FillLifecycle::AwaitingConfirmation)
    {
        warn!(
            order_id  = %pending.exchange_order_id,
            token_id  = %pending.token_id,
            elapsed_secs = elapsed,
            "⚠️  Order pending >{MAX_PENDING_AGE_SECS}s — suspected stale; operator review recommended"
        );
        return ReconciliationOutcome::SuspectedStale { elapsed_secs: elapsed };
    }

    let state = status.status.to_ascii_lowercase();

    match state.as_str() {
        // ── Filled ────────────────────────────────────────────────────────
        "matched" | "filled" => {
            let size_matched_shares =
                parse_decimal_field(&status.size_matched, "size_matched");
            let remaining =
                parse_decimal_field(&status.remaining_amount, "remaining_amount");

            let (actual_shares, actual_usdc) = if size_matched_shares > 0.0 {
                let usdc = size_matched_shares * pending.submitted_price;
                (size_matched_shares, usdc)
            } else {
                // Exchange did not return size_matched — fall back to expected
                // size and emit a warning so the operator knows the fill amount
                // is unverified.
                warn!(
                    order_id = %pending.exchange_order_id,
                    "size_matched absent from exchange response — using expected size as fallback (unverified)"
                );
                (pending.expected_size_shares, pending.expected_size_usdc)
            };

            let fill_ratio = if pending.expected_size_usdc > 0.0 {
                actual_usdc / pending.expected_size_usdc
            } else {
                1.0
            };
            // Partial if there is remaining amount or fill ratio is below 95 %.
            let partial_fill = remaining > 0.001 || fill_ratio < 0.95;

            if partial_fill {
                warn!(
                    order_id      = %pending.exchange_order_id,
                    expected_usdc = pending.expected_size_usdc,
                    actual_usdc,
                    fill_ratio    = format!("{:.1}%", fill_ratio * 100.0),
                    remaining,
                    "⚠️  Partial fill — recording actual amount only"
                );
            } else {
                info!(
                    order_id    = %pending.exchange_order_id,
                    actual_usdc,
                    actual_shares,
                    "✅ Full fill confirmed by exchange"
                );
            }

            pending.lifecycle = FillLifecycle::Confirmed {
                actual_size_usdc:   actual_usdc,
                actual_size_shares: actual_shares,
            };

            ReconciliationOutcome::Fill {
                token_id:           pending.token_id.clone(),
                side:               pending.side,
                actual_size_usdc:   actual_usdc,
                actual_size_shares: actual_shares,
                submitted_price:    pending.submitted_price,
                partial_fill,
                fill_ratio,
            }
        }

        // ── Not filled ────────────────────────────────────────────────────
        "cancelled" | "canceled" => {
            let reason = "exchange_cancelled".to_string();
            warn!(
                order_id = %pending.exchange_order_id,
                token_id = %pending.token_id,
                "Order cancelled by exchange — no fill recorded"
            );
            pending.lifecycle = FillLifecycle::NoFill { reason: reason.clone() };
            ReconciliationOutcome::NoFill {
                token_id: pending.token_id.clone(),
                reason,
            }
        }

        "rejected" => {
            let reason = "exchange_rejected".to_string();
            error!(
                order_id = %pending.exchange_order_id,
                token_id = %pending.token_id,
                "Order rejected by exchange — no fill recorded"
            );
            pending.lifecycle = FillLifecycle::NoFill { reason: reason.clone() };
            ReconciliationOutcome::NoFill {
                token_id: pending.token_id.clone(),
                reason,
            }
        }

        "expired" => {
            let reason = "order_expired".to_string();
            warn!(
                order_id = %pending.exchange_order_id,
                token_id = %pending.token_id,
                "Order expired on exchange — no fill recorded"
            );
            pending.lifecycle = FillLifecycle::NoFill { reason: reason.clone() };
            ReconciliationOutcome::NoFill {
                token_id: pending.token_id.clone(),
                reason,
            }
        }

        // ── Still open ────────────────────────────────────────────────────
        "live" | "open" | "unmatched" | "delayed" | "pending" => {
            ReconciliationOutcome::StillPending
        }

        other => {
            warn!(
                order_id = %pending.exchange_order_id,
                status   = other,
                "Unknown order status returned by exchange — retrying next pass"
            );
            ReconciliationOutcome::StillPending
        }
    }
}

// ─── Position Drift Detection ─────────────────────────────────────────────────

/// A detected divergence between the bot's local position view and the
/// exchange's actual open order state.
#[derive(Debug, Clone)]
pub struct DriftEvent {
    pub token_id:    String,
    /// Shares held according to local portfolio.
    pub local_size:  f64,
    /// Shares held according to exchange snapshot.
    pub exchange_size: f64,
    /// Absolute percentage divergence.
    pub drift_pct:   f64,
    pub detected_at: std::time::SystemTime,
}

/// Compare local positions against an exchange snapshot and return any
/// markets where drift exceeds [`DRIFT_ALERT_THRESHOLD_PCT`].
///
/// # Arguments
/// * `local_positions`      — `token_id → shares` from the local portfolio.
/// * `exchange_open_orders` — `token_id → shares` from `GET /orders` snapshot.
pub fn detect_position_drift(
    local_positions:      &HashMap<String, f64>,
    exchange_open_orders: &HashMap<String, f64>,
) -> Vec<DriftEvent> {
    let mut events = Vec::new();
    let now = std::time::SystemTime::now();

    // Markets present locally — check against exchange.
    for (token_id, &local_size) in local_positions {
        let exchange_size = exchange_open_orders.get(token_id).copied().unwrap_or(0.0);
        if let Some(evt) = build_drift_event(token_id, local_size, exchange_size, now) {
            events.push(evt);
        }
    }

    // Markets present on exchange but missing locally.
    for (token_id, &exchange_size) in exchange_open_orders {
        if !local_positions.contains_key(token_id) && exchange_size > 0.001 {
            warn!(
                token_id,
                exchange_size,
                "🚨 Exchange has open order not tracked locally — possible ghost position"
            );
            events.push(DriftEvent {
                token_id:     token_id.clone(),
                local_size:   0.0,
                exchange_size,
                drift_pct:    100.0,
                detected_at:  now,
            });
        }
    }

    events
}

fn build_drift_event(
    token_id:      &str,
    local_size:    f64,
    exchange_size: f64,
    now:           std::time::SystemTime,
) -> Option<DriftEvent> {
    let max = local_size.max(exchange_size);
    if max < 0.001 {
        return None; // Both effectively zero — no drift to report.
    }
    let drift_pct = ((local_size - exchange_size).abs() / max) * 100.0;
    if drift_pct > DRIFT_ALERT_THRESHOLD_PCT {
        warn!(
            token_id,
            local_size,
            exchange_size,
            drift_pct = format!("{:.1}%", drift_pct),
            "🚨 Position drift detected between local state and exchange"
        );
        Some(DriftEvent {
            token_id: token_id.to_string(),
            local_size,
            exchange_size,
            drift_pct,
            detected_at: now,
        })
    } else {
        None
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn parse_decimal_field(field: &Option<String>, name: &str) -> f64 {
    match field {
        Some(s) => s.parse::<f64>().unwrap_or_else(|_| {
            warn!(field = name, raw = s, "Failed to parse decimal field from exchange");
            0.0
        }),
        None => 0.0,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_executor::OrderStatus;

    fn make_order(expected_usdc: f64, price: f64) -> PendingOrder {
        PendingOrder::new(
            "order-123".to_string(),
            "token-abc".to_string(),
            OrderSide::Buy,
            expected_usdc,
            price,
        )
    }

    fn make_status(status: &str, size_matched: Option<&str>, remaining: Option<&str>) -> OrderStatus {
        OrderStatus {
            id:               "order-123".to_string(),
            status:           status.to_string(),
            maker_amount:     None,
            taker_amount:     None,
            remaining_amount: remaining.map(str::to_string),
            size_matched:     size_matched.map(str::to_string),
        }
    }

    #[test]
    fn full_fill_records_actual_amounts() {
        let mut order = make_order(100.0, 0.50);
        let status = make_status("matched", Some("200"), None); // 200 shares × $0.50 = $100
        let outcome = process_order_status(&mut order, &status);
        match outcome {
            ReconciliationOutcome::Fill { actual_size_usdc, partial_fill, fill_ratio, .. } => {
                assert!((actual_size_usdc - 100.0).abs() < 0.01);
                assert!(!partial_fill);
                assert!((fill_ratio - 1.0).abs() < 0.01);
            }
            other => panic!("Expected Fill, got {:?}", other),
        }
        assert!(order.is_terminal());
    }

    #[test]
    fn partial_fill_flagged() {
        let mut order = make_order(100.0, 0.50);
        // Only 100 of 200 expected shares matched
        let status = make_status("matched", Some("100"), Some("100"));
        let outcome = process_order_status(&mut order, &status);
        match outcome {
            ReconciliationOutcome::Fill { partial_fill, actual_size_usdc, .. } => {
                assert!(partial_fill, "Should be flagged as partial fill");
                assert!((actual_size_usdc - 50.0).abs() < 0.01);
            }
            other => panic!("Expected Fill, got {:?}", other),
        }
    }

    #[test]
    fn rejected_order_returns_no_fill() {
        let mut order = make_order(100.0, 0.50);
        let status = make_status("rejected", None, None);
        let outcome = process_order_status(&mut order, &status);
        assert!(matches!(outcome, ReconciliationOutcome::NoFill { .. }));
        assert!(order.is_terminal());
    }

    #[test]
    fn cancelled_order_returns_no_fill() {
        let mut order = make_order(100.0, 0.50);
        let status = make_status("cancelled", None, None);
        let outcome = process_order_status(&mut order, &status);
        assert!(matches!(outcome, ReconciliationOutcome::NoFill { .. }));
    }

    #[test]
    fn pending_order_returns_still_pending() {
        let mut order = make_order(100.0, 0.50);
        let status = make_status("live", None, None);
        let outcome = process_order_status(&mut order, &status);
        assert!(matches!(outcome, ReconciliationOutcome::StillPending));
        assert!(!order.is_terminal());
    }

    #[test]
    fn drift_detection_flags_mismatch() {
        let local: HashMap<String, f64> = [("token-A".to_string(), 100.0)].into();
        let exchange: HashMap<String, f64> = [("token-A".to_string(), 50.0)].into();
        let events = detect_position_drift(&local, &exchange);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].token_id, "token-A");
        assert!((events[0].drift_pct - 50.0).abs() < 1.0);
    }

    #[test]
    fn drift_detection_no_alert_when_match() {
        let local: HashMap<String, f64> = [("token-A".to_string(), 100.0)].into();
        let exchange: HashMap<String, f64> = [("token-A".to_string(), 100.0)].into();
        let events = detect_position_drift(&local, &exchange);
        assert!(events.is_empty());
    }

    #[test]
    fn ghost_position_detected() {
        let local: HashMap<String, f64> = HashMap::new();
        let exchange: HashMap<String, f64> = [("token-X".to_string(), 50.0)].into();
        let events = detect_position_drift(&local, &exchange);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].local_size, 0.0);
    }
}
