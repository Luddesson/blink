//! Single-owner reconciler for the order router.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::hot_metrics;
use crate::order_executor::OrderExecutor;

use super::router::{PendingOrderStore, RouterCounters};
use super::state::OrderState;

const RECONCILE_INTERVAL: Duration = Duration::from_millis(250);
const STALE_THRESHOLD: Duration = Duration::from_secs(300);

/// Spawn the reconciler as a detached tokio task.
pub fn spawn_reconciler(
    store: Arc<PendingOrderStore>,
    counters: Arc<RouterCounters>,
    executor: Arc<OrderExecutor>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
        loop {
            ticker.tick().await;
            sweep(&store, &counters, &executor).await;
        }
    });
}

async fn sweep(
    store: &PendingOrderStore,
    counters: &RouterCounters,
    executor: &OrderExecutor,
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
                                if matches!(ns, OrderState::Filled) {
                                    hot_metrics::counters()
                                        .full_fills
                                        .fetch_add(1, Ordering::Relaxed);
                                    hot_metrics::counters()
                                        .pending_orders_count
                                        .fetch_sub(1, Ordering::Relaxed);
                                } else if matches!(ns, OrderState::PartialFilled) {
                                    hot_metrics::counters()
                                        .partial_fills
                                        .fetch_add(1, Ordering::Relaxed);
                                } else if matches!(
                                    ns,
                                    OrderState::Cancelled | OrderState::Rejected
                                ) {
                                    hot_metrics::counters()
                                        .pending_orders_count
                                        .fetch_sub(1, Ordering::Relaxed);
                                }
                                entry.transition(ns);
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

            OrderState::SubmitUnknown => {
                info!(
                    intent_id,
                    client_order_id,
                    "Reconciler: SubmitUnknown — awaiting venue confirmation \
                     (implement GET /orders?client_order_id lookup in Phase 2.5)"
                );

                if age >= STALE_THRESHOLD {
                    if let Some(mut entry) = store.get_mut(&intent_id) {
                        entry.transition(OrderState::Stale);
                        hot_metrics::counters()
                            .pending_orders_count
                            .fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }

            OrderState::Created | OrderState::Submitting => {
                if age >= STALE_THRESHOLD {
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

            OrderState::Cancelling => {}
        }
    }

    hot_metrics::counters().reconcile_lag_ms_last.store(
        sweep_start.elapsed().as_millis() as i64,
        Ordering::Relaxed,
    );
}

fn map_exchange_status(status: &str) -> Option<OrderState> {
    match status.to_uppercase().as_str() {
        "MATCHED" | "FILLED" => Some(OrderState::Filled),
        "PARTIALLY_FILLED" => Some(OrderState::PartialFilled),
        "CANCELLED" | "CANCELED" => Some(OrderState::Cancelled),
        "REJECTED" => Some(OrderState::Rejected),
        _ => None,
    }
}
