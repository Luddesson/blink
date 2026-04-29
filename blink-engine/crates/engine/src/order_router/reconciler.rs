//! Single-owner reconciler for the order router.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::hot_metrics;
use crate::order_executor::OrderExecutor;

use super::fill_hook::RouterFillHook;
use super::router::{PendingOrderStore, RouterCounters};
use super::state::OrderState;

const RECONCILE_INTERVAL: Duration = Duration::from_millis(250);

/// Stale threshold for `Created`/`Submitting` states (not SubmitUnknown).
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
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
        loop {
            ticker.tick().await;
            sweep(&store, &counters, &executor, fill_hook.as_deref()).await;
        }
    });
}

async fn sweep(
    store: &PendingOrderStore,
    counters: &RouterCounters,
    executor: &OrderExecutor,
    fill_hook: Option<&dyn RouterFillHook>,
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
                        // Parse fill amounts from string decimals into milliUSDC u64.
                        let size_matched_u64: u64 = status
                            .size_matched
                            .as_deref()
                            .and_then(|s| s.parse::<f64>().ok())
                            .map(|f| (f * 1_000.0) as u64)
                            .unwrap_or(0);
                        let fill_price_u64: u64 = 0; // price not in OrderStatus; use 0 sentinel

                        let new_state = map_exchange_status(&status.status);
                        if let Some(ns) = new_state {
                            if let Some(mut entry) = store.get_mut(&intent_id) {
                                match ns {
                                    OrderState::Filled => {
                                        let delta = entry
                                            .apply_fill_update(size_matched_u64, fill_price_u64);
                                        update_fill_metrics(delta, true, fill_hook);
                                        hot_metrics::counters()
                                            .full_fills
                                            .fetch_add(1, Ordering::Relaxed);
                                        hot_metrics::counters()
                                            .pending_orders_count
                                            .fetch_sub(1, Ordering::Relaxed);
                                        entry.transition(ns);
                                    }
                                    OrderState::PartialFilled => {
                                        let delta = entry
                                            .apply_fill_update(size_matched_u64, fill_price_u64);
                                        if delta > 0 {
                                            update_fill_metrics(delta, false, fill_hook);
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
                            }
                        } else if upper == "LIVE" || upper == "MATCHED" {
                            // Still live — cancel not yet processed; stay Cancelling.
                        } else if upper == "FILLED" || upper == "PARTIALLY_FILLED" {
                            // Filled before cancel landed — update state.
                            let is_full = upper == "FILLED";
                            if let Some(mut entry) = store.get_mut(&intent_id) {
                                if is_full {
                                    hot_metrics::counters()
                                        .full_fills
                                        .fetch_add(1, Ordering::Relaxed);
                                    hot_metrics::counters()
                                        .pending_orders_count
                                        .fetch_sub(1, Ordering::Relaxed);
                                    entry.transition(OrderState::Filled);
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
                                    entry.transition(OrderState::Filled);
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
                                    entry.transition(OrderState::PartialFilled);
                                    hot_metrics::counters()
                                        .submit_unknown_resolved_acked_total
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                "CANCELLED" | "CANCELED" => {
                                    entry.transition(OrderState::Cancelled);
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
                    warn!(
                        intent_id,
                        age_secs = age.as_secs(),
                        "Reconciler: order stuck in {:?} — marking Stale",
                        state
                    );
                    if let Some(mut entry) = store.get_mut(&intent_id) {
                        entry.transition(OrderState::Stale);
                        hot_metrics::counters()
                            .pending_orders_count
                            .fetch_sub(1, Ordering::Relaxed);
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

/// Update fill metrics and notify the fill hook.
fn update_fill_metrics(delta_u64: u64, is_full: bool, hook: Option<&dyn RouterFillHook>) {
    hot_metrics::counters()
        .fills_delta_size_last
        .store(delta_u64 as i64, Ordering::Relaxed);

    let full = hot_metrics::counters().full_fills.load(Ordering::Relaxed);
    let partial = hot_metrics::counters()
        .partial_fills
        .load(Ordering::Relaxed);
    let total = full + partial;
    if total > 0 {
        // full-fill ratio in per-mille (integer math).
        let ratio = (full * 1_000) / total;
        hot_metrics::counters()
            .partial_fill_ratio_permille
            .store(ratio as i64, Ordering::Relaxed);
    }

    if let Some(h) = hook {
        if is_full {
            // intent_id not available at this level — hook is notified by reconciler caller
            // with intent_id when needed; here we use 0 as sentinel for batch updates.
            h.on_full_fill(0);
        } else {
            h.on_partial_fill(0, delta_u64, 0);
        }
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
