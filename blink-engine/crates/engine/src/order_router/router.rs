//! `OrderRouter` — async submit actor with per-market queues and worker pool.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::hot_metrics;
use crate::order_executor::OrderExecutor;

use super::intent::OrderIntent;
use super::state::{OrderState, PendingOrder};

// ─── Tunables ─────────────────────────────────────────────────────────────────

const INBOUND_DEPTH: usize = 4_096;
const WORKER_QUEUE_DEPTH: usize = 128;
const SUBMIT_WORKERS: usize = 8;

// ─── Public types ─────────────────────────────────────────────────────────────

/// Thread-safe store of all pending orders managed by this router instance.
/// Keyed by `intent_id`.
pub type PendingOrderStore = DashMap<u64, PendingOrder>;

/// Returned by `OrderRouter::submit()` when the inbound queue is full.
#[derive(Debug)]
pub struct RouterFull;

impl std::fmt::Display for RouterFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OrderRouter inbound queue full — intent dropped")
    }
}

impl std::error::Error for RouterFull {}

/// Router-specific atomic counters (separate from `HotCounters`).
pub struct RouterCounters {
    pub inbound_depth: std::sync::atomic::AtomicI64,
    pub dropped_full: std::sync::atomic::AtomicU64,
    pub retries_total: std::sync::atomic::AtomicU64,
    pub reconcile_sweeps: std::sync::atomic::AtomicU64,
}

impl Default for RouterCounters {
    fn default() -> Self {
        Self {
            inbound_depth: std::sync::atomic::AtomicI64::new(0),
            dropped_full: std::sync::atomic::AtomicU64::new(0),
            retries_total: std::sync::atomic::AtomicU64::new(0),
            reconcile_sweeps: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

// ─── OrderRouter ──────────────────────────────────────────────────────────────

pub struct OrderRouter {
    inbound_tx: mpsc::Sender<OrderIntent>,
    inbound_rx: tokio::sync::Mutex<Option<mpsc::Receiver<OrderIntent>>>,
    pub store: Arc<PendingOrderStore>,
    pub counters: Arc<RouterCounters>,
}

impl OrderRouter {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(INBOUND_DEPTH);
        Self {
            inbound_tx: tx,
            inbound_rx: tokio::sync::Mutex::new(Some(rx)),
            store: Arc::new(DashMap::new()),
            counters: Arc::new(RouterCounters::default()),
        }
    }

    /// Non-blocking submit.
    pub fn submit(&self, intent: OrderIntent) -> Result<(), RouterFull> {
        match self.inbound_tx.try_send(intent) {
            Ok(()) => {
                self.counters.inbound_depth.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => {
                self.counters.dropped_full.fetch_add(1, Ordering::Relaxed);
                hot_metrics::counters()
                    .router_dropped_full
                    .fetch_add(1, Ordering::Relaxed);
                hot_metrics::counters()
                    .submits_rejected
                    .fetch_add(1, Ordering::Relaxed);
                Err(RouterFull)
            }
        }
    }

    /// Spawn the dispatcher task, submit-worker pool.
    pub async fn spawn_workers(&self, executor: Arc<OrderExecutor>) {
        let rx = {
            let mut guard = self.inbound_rx.lock().await;
            match guard.take() {
                Some(rx) => rx,
                None => {
                    warn!("OrderRouter::spawn_workers called more than once — ignoring");
                    return;
                }
            }
        };

        let store = Arc::clone(&self.store);
        let counters = Arc::clone(&self.counters);

        let mut worker_txs: Vec<mpsc::Sender<OrderIntent>> = Vec::with_capacity(SUBMIT_WORKERS);
        for _ in 0..SUBMIT_WORKERS {
            let (wtx, wrx) = mpsc::channel::<OrderIntent>(WORKER_QUEUE_DEPTH);
            worker_txs.push(wtx);

            let s = Arc::clone(&store);
            let c = Arc::clone(&counters);
            let exec = Arc::clone(&executor);
            tokio::spawn(async move {
                let mut wrx = wrx;
                while let Some(intent) = wrx.recv().await {
                    submit_one(intent, &s, &c, &exec).await;
                }
            });
        }

        let s_disp = Arc::clone(&store);
        let c_disp = Arc::clone(&counters);
        tokio::spawn(async move {
            let mut rx = rx;
            let mut rr = 0usize;
            while let Some(intent) = rx.recv().await {
                c_disp.inbound_depth.fetch_sub(1, Ordering::Relaxed);

                let pending = PendingOrder::new(
                    intent.intent_id,
                    intent.token_id.clone(),
                    intent.side,
                    intent.size_u64 as f64 / 1_000.0,
                    intent.price_u64 as f64 / 1_000.0,
                );
                s_disp.insert(intent.intent_id, pending);
                hot_metrics::counters()
                    .pending_orders_count
                    .fetch_add(1, Ordering::Relaxed);

                if let Err(e) = worker_txs[rr].send(intent).await {
                    error!(
                        worker = rr,
                        "OrderRouter dispatcher: worker channel closed — intent dropped: {e}"
                    );
                }
                rr = (rr + 1) % SUBMIT_WORKERS;
            }
        });
    }

    pub fn pending_count(&self) -> usize {
        self.store.len()
    }
}

impl Default for OrderRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Submit worker ────────────────────────────────────────────────────────────

async fn submit_one(
    intent: OrderIntent,
    store: &PendingOrderStore,
    counters: &RouterCounters,
    executor: &OrderExecutor,
) {
    hot_metrics::counters()
        .submits_started
        .fetch_add(1, Ordering::Relaxed);
    let _submit_timer = hot_metrics::StageTimer::start(hot_metrics::HotStage::Submit);

    if let Some(mut entry) = store.get_mut(&intent.intent_id) {
        entry.transition(OrderState::Submitting);
        entry.submit_attempts += 1;
    }

    let signed = match &intent.signed_payload {
        Some(s) => s.clone(),
        None => {
            error!(
                intent_id = intent.intent_id,
                "OrderRouter: intent missing signed_payload — dropping"
            );
            if let Some(mut entry) = store.get_mut(&intent.intent_id) {
                entry.transition(OrderState::Rejected);
            }
            hot_metrics::counters()
                .submits_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    match executor.submit_order(&signed, intent.tif).await {
        Ok(crate::order_executor::SubmitOutcome::Success(resp)) if resp.success => {
            let _ack_timer = hot_metrics::StageTimer::start(hot_metrics::HotStage::Ack);
            let order_id = resp.order_id.clone();
            info!(
                intent_id = intent.intent_id,
                order_id = ?order_id,
                "OrderRouter: ✅ order submitted and acked"
            );
            if let Some(mut entry) = store.get_mut(&intent.intent_id) {
                entry.order_id = order_id;
                entry.transition(OrderState::Acked);
            }
            hot_metrics::counters()
                .submits_ack
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(crate::order_executor::SubmitOutcome::Success(resp)) => {
            warn!(
                intent_id = intent.intent_id,
                error = ?resp.error_msg,
                "OrderRouter: ❌ order rejected by exchange"
            );
            if let Some(mut entry) = store.get_mut(&intent.intent_id) {
                entry.transition(OrderState::Rejected);
            }
            hot_metrics::counters()
                .submits_rejected
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(crate::order_executor::SubmitOutcome::Unknown) => {
            warn!(
                intent_id = intent.intent_id,
                "OrderRouter: submit outcome unknown (all attempts timed out) — parking in SubmitUnknown"
            );
            if let Some(mut entry) = store.get_mut(&intent.intent_id) {
                entry.transition(OrderState::SubmitUnknown);
            }
            hot_metrics::counters()
                .submit_unknown
                .fetch_add(1, Ordering::Relaxed);
            counters.retries_total.fetch_add(1, Ordering::Relaxed);
            hot_metrics::counters()
                .router_retries_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(e) => {
            warn!(
                intent_id = intent.intent_id,
                error = %e,
                "OrderRouter: submit failed (non-retryable error) — marking Rejected"
            );
            if let Some(mut entry) = store.get_mut(&intent.intent_id) {
                entry.transition(OrderState::Rejected);
            }
            hot_metrics::counters()
                .submits_rejected
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}
