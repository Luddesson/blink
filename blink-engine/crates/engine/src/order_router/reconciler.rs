//! Single-owner reconciler for the order router.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::hot_metrics;
use crate::order_executor::OrderExecutor;
use crate::risk_manager::StreamRiskGate;

use super::fill_hook::{RouterFillEvent, RouterFillHook};
use super::router::{PendingOrderStore, RouterCounters};
use super::state::OrderState;

const RECONCILE_INTERVAL: Duration = Duration::from_millis(250);

/// Watchdog threshold for `Created`/`Submitting` states.
const CREATED_STALE_THRESHOLD: Duration = Duration::from_secs(300);

/// Maximum lookup attempts before declaring a SubmitUnknown order Rejected.
const SUBMIT_UNKNOWN_MAX_LOOKUPS: u32 = 3;

/// Minimum age before the first lookup (avoid spurious lookups on fast acks).
const SUBMIT_UNKNOWN_MIN_AGE: Duration = Duration::from_secs(5);

fn submit_unknown_timeout() -> Duration {
    let ms = std::env::var("BLINK_SUBMIT_UNKNOWN_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30_000);
    Duration::from_millis(ms)
}

/// Spawn the reconciler as a detached tokio task.
pub fn spawn_reconciler(
    store: Arc<PendingOrderStore>,
    counters: Arc<RouterCounters>,
    executor: Arc<OrderExecutor>,
    fill_hook: Option<Arc<dyn RouterFillHook>>,
    risk_gate: Option<Arc<StreamRiskGate>>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
        loop {
            ticker.tick().await;
            sweep(
                &store,
                &counters,
                &executor,
                fill_hook.as_deref(),
                risk_gate.as_ref(),
            )
            .await;
        }
    });
}

async fn sweep(
    store: &PendingOrderStore,
    counters: &RouterCounters,
    executor: &OrderExecutor,
    fill_hook: Option<&dyn RouterFillHook>,
    risk_gate: Option<&Arc<StreamRiskGate>>,
) {
    counters.reconcile_sweeps.fetch_add(1, Ordering::Relaxed);
    hot_metrics::counters()
        .router_reconcile_sweeps
        .fetch_add(1, Ordering::Relaxed);

    let snapshot: Vec<(u64, Option<String>, String, OrderState, Duration)> = store
        .iter()
        .map(|e| {
            let p = e.value();
            (
                p.intent_id,
                p.order_id.clone(),
                p.client_order_id.clone(),
                p.state.clone(),
                p.created_at.elapsed(),
            )
        })
        .collect();

    let sweep_start = std::time::Instant::now();

    for (intent_id, order_id_opt, client_order_id, state, age) in snapshot {
        match state {
            OrderState::Acked | OrderState::PartialFilled => {
                let order_id = match order_id_opt {
                    Some(id) => id,
                    None => continue,
                };
                match executor.get_order_status(&order_id).await {
                    Ok(status) => {
                        let new_state = map_exchange_status(&status.status);
                        if let Some(ns) = new_state {
                            if let Some(mut entry) = store.get_mut(&intent_id) {
                                let size_matched_u64 = matched_notional_u64(
                                    status.size_matched.as_deref(),
                                    entry.entry_price,
                                );
                                let fill_price_u64 = price_to_u64(entry.entry_price);
                                match ns {
                                    OrderState::Filled => {
                                        let delta = entry
                                            .apply_fill_update(size_matched_u64, fill_price_u64);
                                        update_fill_metrics(
                                            fill_event_from_entry(
                                                &entry,
                                                delta,
                                                fill_price_u64,
                                                true,
                                            ),
                                            fill_hook,
                                        );
                                        hot_metrics::counters()
                                            .full_fills
                                            .fetch_add(1, Ordering::Relaxed);
                                        hot_metrics::counters()
                                            .pending_orders_count
                                            .fetch_sub(1, Ordering::Relaxed);
                                        entry.transition(ns);
                                        release_risk_gate(
                                            risk_gate,
                                            &entry.market_id,
                                            entry.size_u64,
                                        );
                                    }
                                    OrderState::PartialFilled => {
                                        let delta = entry
                                            .apply_fill_update(size_matched_u64, fill_price_u64);
                                        if delta > 0 {
                                            update_fill_metrics(
                                                fill_event_from_entry(
                                                    &entry,
                                                    delta,
                                                    fill_price_u64,
                                                    false,
                                                ),
                                                fill_hook,
                                            );
                                            hot_metrics::counters()
                                                .partial_fills
                                                .fetch_add(1, Ordering::Relaxed);
                                        }
                                        entry.transition(ns);
                                    }
                                    OrderState::Cancelled | OrderState::Rejected => {
                                        hot_metrics::counters()
                                            .pending_orders_count
                                            .fetch_sub(1, Ordering::Relaxed);
                                        entry.transition(ns);
                                        release_risk_gate(
                                            risk_gate,
                                            &entry.market_id,
                                            entry.size_u64,
                                        );
                                    }
                                    other => {
                                        entry.transition(other);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            intent_id,
                            order_id,
                            error = %e,
                            "Reconciler: status fetch failed"
                        );
                    }
                }
            }

            OrderState::Cancelling => {
                let order_id = match order_id_opt {
                    Some(id) => id,
                    None => continue,
                };
                match executor.get_order_status(&order_id).await {
                    Ok(status) => {
                        let upper = status.status.to_uppercase();
                        if upper == "CANCELLED" || upper == "CANCELED" {
                            hot_metrics::counters()
                                .cancel_success_total
                                .fetch_add(1, Ordering::Relaxed);
                            hot_metrics::counters()
                                .cancels_ack
                                .fetch_add(1, Ordering::Relaxed);
                            hot_metrics::counters()
                                .pending_orders_count
                                .fetch_sub(1, Ordering::Relaxed);
                            if let Some(mut entry) = store.get_mut(&intent_id) {
                                entry.transition(OrderState::Cancelled);
                                release_risk_gate(risk_gate, &entry.market_id, entry.size_u64);
                            }
                        } else if upper == "LIVE" || upper == "MATCHED" {
                            // Still live — cancel not yet processed; stay Cancelling.
                        } else if upper == "FILLED" || upper == "PARTIALLY_FILLED" {
                            // Filled before cancel landed — update state.
                            let is_full = upper == "FILLED";
                            if let Some(mut entry) = store.get_mut(&intent_id) {
                                let size_matched_u64 = matched_notional_u64(
                                    status.size_matched.as_deref(),
                                    entry.entry_price,
                                );
                                let fill_price_u64 = price_to_u64(entry.entry_price);
                                let delta =
                                    entry.apply_fill_update(size_matched_u64, fill_price_u64);
                                if delta > 0 {
                                    update_fill_metrics(
                                        fill_event_from_entry(
                                            &entry,
                                            delta,
                                            fill_price_u64,
                                            is_full,
                                        ),
                                        fill_hook,
                                    );
                                }
                                if is_full {
                                    hot_metrics::counters()
                                        .full_fills
                                        .fetch_add(1, Ordering::Relaxed);
                                    hot_metrics::counters()
                                        .pending_orders_count
                                        .fetch_sub(1, Ordering::Relaxed);
                                    entry.transition(OrderState::Filled);
                                    release_risk_gate(risk_gate, &entry.market_id, entry.size_u64);
                                } else {
                                    hot_metrics::counters()
                                        .partial_fills
                                        .fetch_add(1, Ordering::Relaxed);
                                    entry.transition(OrderState::PartialFilled);
                                }
                            }
                        } else {
                            // Exchange rejected the cancel or returned unknown status.
                            warn!(
                                intent_id,
                                order_id,
                                exchange_status = status.status,
                                "Reconciler: cancel not confirmed — order stays Cancelling"
                            );
                            hot_metrics::counters()
                                .cancel_reject_total
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        warn!(
                            intent_id,
                            order_id,
                            error = %e,
                            "Reconciler: cancel status fetch failed"
                        );
                    }
                }
            }

            OrderState::SubmitUnknown => {
                let timeout = submit_unknown_timeout();

                // Don't rush — wait at least MIN_AGE before first lookup.
                if age < SUBMIT_UNKNOWN_MIN_AGE {
                    continue;
                }

                // Get current lookup_attempts before mutating.
                let lookup_attempts = store
                    .get(&intent_id)
                    .map(|e| e.lookup_attempts)
                    .unwrap_or(0);

                if age >= timeout && lookup_attempts >= SUBMIT_UNKNOWN_MAX_LOOKUPS {
                    // All lookups exhausted and timeout reached — assume Rejected.
                    warn!(
                        intent_id,
                        client_order_id,
                        lookup_attempts,
                        age_secs = age.as_secs(),
                        "Reconciler: SubmitUnknown exhausted lookups → Rejected"
                    );
                    if let Some(mut entry) = store.get_mut(&intent_id) {
                        entry.transition(OrderState::Rejected);
                        release_risk_gate(risk_gate, &entry.market_id, entry.size_u64);
                        hot_metrics::counters()
                            .submit_unknown_resolved_rejected_total
                            .fetch_add(1, Ordering::Relaxed);
                        hot_metrics::counters()
                            .pending_orders_count
                            .fetch_sub(1, Ordering::Relaxed);
                    }
                    continue;
                }

                // Perform a lookup.
                hot_metrics::counters()
                    .submit_unknown_lookup_attempts_total
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(mut entry) = store.get_mut(&intent_id) {
                    entry.lookup_attempts += 1;
                }

                info!(
                    intent_id,
                    client_order_id,
                    attempt = lookup_attempts + 1,
                    "Reconciler: SubmitUnknown — querying venue by client_order_id"
                );

                match executor.find_order_by_client_id(&client_order_id).await {
                    Ok(Some(found)) => {
                        let upper = found.status.to_uppercase();
                        info!(
                            intent_id,
                            client_order_id,
                            exchange_status = %found.status,
                            order_id = %found.id,
                            "Reconciler: SubmitUnknown resolved"
                        );
                        if let Some(mut entry) = store.get_mut(&intent_id) {
                            entry.order_id = Some(found.id.clone());
                            match upper.as_str() {
                                "LIVE" | "DELAYED" => {
                                    entry.transition(OrderState::Acked);
                                    hot_metrics::counters()
                                        .submit_unknown_resolved_acked_total
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                "MATCHED" | "FILLED" => {
                                    let fill_price = found
                                        .price
                                        .as_deref()
                                        .and_then(|p| p.parse::<f64>().ok())
                                        .unwrap_or(entry.entry_price);
                                    let fill_price_u64 = price_to_u64(fill_price);
                                    let size_matched_u64 = matched_notional_u64(
                                        found.size_matched.as_deref(),
                                        fill_price,
                                    );
                                    let delta =
                                        entry.apply_fill_update(size_matched_u64, fill_price_u64);
                                    if delta > 0 {
                                        update_fill_metrics(
                                            fill_event_from_entry(
                                                &entry,
                                                delta,
                                                fill_price_u64,
                                                true,
                                            ),
                                            fill_hook,
                                        );
                                    }
                                    entry.transition(OrderState::Filled);
                                    release_risk_gate(risk_gate, &entry.market_id, entry.size_u64);
                                    hot_metrics::counters()
                                        .full_fills
                                        .fetch_add(1, Ordering::Relaxed);
                                    hot_metrics::counters()
                                        .pending_orders_count
                                        .fetch_sub(1, Ordering::Relaxed);
                                    hot_metrics::counters()
                                        .submit_unknown_resolved_acked_total
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                "PARTIALLY_FILLED" => {
                                    let fill_price = found
                                        .price
                                        .as_deref()
                                        .and_then(|p| p.parse::<f64>().ok())
                                        .unwrap_or(entry.entry_price);
                                    let fill_price_u64 = price_to_u64(fill_price);
                                    let size_matched_u64 = matched_notional_u64(
                                        found.size_matched.as_deref(),
                                        fill_price,
                                    );
                                    let delta =
                                        entry.apply_fill_update(size_matched_u64, fill_price_u64);
                                    if delta > 0 {
                                        update_fill_metrics(
                                            fill_event_from_entry(
                                                &entry,
                                                delta,
                                                fill_price_u64,
                                                false,
                                            ),
                                            fill_hook,
                                        );
                                    }
                                    entry.transition(OrderState::PartialFilled);
                                    hot_metrics::counters()
                                        .submit_unknown_resolved_acked_total
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                "CANCELLED" | "CANCELED" => {
                                    entry.transition(OrderState::Cancelled);
                                    release_risk_gate(risk_gate, &entry.market_id, entry.size_u64);
                                    hot_metrics::counters()
                                        .pending_orders_count
                                        .fetch_sub(1, Ordering::Relaxed);
                                    hot_metrics::counters()
                                        .submit_unknown_resolved_rejected_total
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                _ => {
                                    // Unknown status — stay in SubmitUnknown for next sweep.
                                    warn!(
                                        intent_id,
                                        exchange_status = %found.status,
                                        "Reconciler: SubmitUnknown — unrecognised exchange status, keeping"
                                    );
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // Not found yet.
                        if age < timeout {
                            info!(
                                intent_id,
                                client_order_id,
                                age_secs = age.as_secs(),
                                timeout_secs = timeout.as_secs(),
                                "Reconciler: SubmitUnknown — not found at venue yet, keeping"
                            );
                        }
                        // If age >= timeout and lookups < max, next sweep will increment and eventually reject.
                    }
                    Err(e) => {
                        warn!(
                            intent_id,
                            client_order_id,
                            error = %e,
                            "Reconciler: find_order_by_client_id failed"
                        );
                    }
                }
            }

            OrderState::Created | OrderState::Submitting => {
                if age >= CREATED_STALE_THRESHOLD {
                    if let Some(mut entry) = store.get_mut(&intent_id) {
                        if entry.last_updated.elapsed() >= CREATED_STALE_THRESHOLD {
                            warn!(
                                intent_id,
                                age_secs = age.as_secs(),
                                "Reconciler: order stuck in {:?}; keeping pending risk until venue terminal truth",
                                state
                            );
                            entry.last_updated = std::time::Instant::now();
                        }
                    }
                }
            }

            OrderState::Filled
            | OrderState::Cancelled
            | OrderState::Rejected
            | OrderState::Stale => {}
        }
    }

    hot_metrics::counters()
        .reconcile_lag_ms_last
        .store(sweep_start.elapsed().as_millis() as i64, Ordering::Relaxed);
}

fn release_risk_gate(gate: Option<&Arc<StreamRiskGate>>, market_id: &str, size_u64: u64) {
    if let Some(g) = gate {
        g.on_order_terminal(market_id, size_u64);
    }
}

fn price_to_u64(price: f64) -> u64 {
    (price.max(0.0) * 1_000.0).round() as u64
}

fn matched_notional_u64(size_matched: Option<&str>, fill_price: f64) -> u64 {
    size_matched
        .and_then(|s| s.parse::<f64>().ok())
        .map(|shares| (shares * fill_price.max(0.0) * 1_000.0).round() as u64)
        .unwrap_or(0)
}

fn fill_event_from_entry(
    entry: &super::state::PendingOrder,
    delta_u64: u64,
    fill_price_u64: u64,
    is_full: bool,
) -> RouterFillEvent {
    RouterFillEvent {
        intent_id: entry.intent_id,
        order_id: entry.order_id.clone(),
        token_id: entry.token_id.clone(),
        side: entry.side,
        delta_size_u64: delta_u64,
        cumulative_size_u64: entry.filled_size_u64,
        remaining_size_u64: entry.remaining_size_u64,
        fill_price_u64,
        is_full,
    }
}

/// Update fill metrics and notify the fill hook.
fn update_fill_metrics(event: RouterFillEvent, hook: Option<&dyn RouterFillHook>) {
    hot_metrics::counters()
        .fills_delta_size_last
        .store(event.delta_size_u64 as i64, Ordering::Relaxed);

    let full = hot_metrics::counters().full_fills.load(Ordering::Relaxed);
    let partial = hot_metrics::counters()
        .partial_fills
        .load(Ordering::Relaxed);
    let total = full + partial;
    if let Some(ratio) = (full * 1_000).checked_div(total) {
        // full-fill ratio in per-mille (integer math).
        hot_metrics::counters()
            .partial_fill_ratio_permille
            .store(ratio as i64, Ordering::Relaxed);
    }

    if let Some(h) = hook {
        h.on_fill_update(event);
    }
}

pub fn map_exchange_status(status: &str) -> Option<OrderState> {
    match status.to_uppercase().as_str() {
        "MATCHED" | "FILLED" => Some(OrderState::Filled),
        "PARTIALLY_FILLED" => Some(OrderState::PartialFilled),
        "CANCELLED" | "CANCELED" => Some(OrderState::Cancelled),
        "REJECTED" => Some(OrderState::Rejected),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::types::OrderSide;

    use super::super::fill_hook::{RouterFillEvent, RouterFillHook};
    use super::super::state::PendingOrder;
    use super::*;

    struct RecordingFillHook {
        events: Mutex<Vec<RouterFillEvent>>,
    }

    impl RecordingFillHook {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }
    }

    impl RouterFillHook for RecordingFillHook {
        fn on_fill_update(&self, event: RouterFillEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn on_partial_fill(&self, _intent_id: u64, _delta_size_u64: u64, _fill_price_u64: u64) {}

        fn on_full_fill(&self, _intent_id: u64) {}
    }

    #[test]
    fn matched_notional_converts_share_fill_to_milli_usdc() {
        assert_eq!(matched_notional_u64(Some("2.5"), 0.40), 1_000);
    }

    #[test]
    fn matched_notional_rounds_to_nearest_milli_usdc() {
        assert_eq!(matched_notional_u64(Some("1.234"), 0.333), 411);
    }

    #[test]
    fn fill_event_from_entry_carries_delta_and_remaining_notional() {
        let mut entry = PendingOrder::new(
            42,
            "market-a".to_string(),
            "token-a".to_string(),
            OrderSide::Buy,
            1.00,
            1_000,
            0.40,
        );
        let fill_price_u64 = price_to_u64(0.40);
        let delta = entry.apply_fill_update(600, fill_price_u64);

        let event = fill_event_from_entry(&entry, delta, fill_price_u64, false);

        assert_eq!(event.intent_id, 42);
        assert_eq!(event.token_id, "token-a");
        assert_eq!(event.side, OrderSide::Buy);
        assert_eq!(event.delta_size_u64, 600);
        assert_eq!(event.cumulative_size_u64, 600);
        assert_eq!(event.remaining_size_u64, 400);
        assert_eq!(event.fill_price_u64, 400);
        assert!(!event.is_full);
    }

    #[test]
    fn update_fill_metrics_dispatches_structured_hook_event() {
        let hook = RecordingFillHook::new();
        let event = RouterFillEvent {
            intent_id: 7,
            order_id: Some("ord-7".to_string()),
            token_id: "token-7".to_string(),
            side: OrderSide::Buy,
            delta_size_u64: 250,
            cumulative_size_u64: 250,
            remaining_size_u64: 750,
            fill_price_u64: 500,
            is_full: false,
        };

        update_fill_metrics(event, Some(&hook));

        let events = hook.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].intent_id, 7);
        assert_eq!(events[0].delta_size_u64, 250);
        assert_eq!(events[0].remaining_size_u64, 750);
    }
}
