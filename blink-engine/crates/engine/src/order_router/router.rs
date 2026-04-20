//! `OrderRouter` — async submit actor with per-market queues and worker pool.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::hot_metrics;
use crate::order_executor::OrderExecutor;
use crate::risk_manager::{AdmitDecision, StreamRiskGate};

use super::intent::OrderIntent;
use super::state::{OrderState, PendingOrder};

// ─── Tunables ─────────────────────────────────────────────────────────────────

const INBOUND_DEPTH: usize = 4_096;
const WORKER_QUEUE_DEPTH: usize = 128;
const SUBMIT_WORKERS: usize = 8;
/// How long a terminal entry is retained in the store before GC evicts it.
const TERMINAL_RETENTION: Duration = Duration::from_secs(120);
/// GC sweep interval.
const GC_INTERVAL: Duration = Duration::from_secs(30);
/// Soft cap on live store size; overflow forces eviction of oldest terminals.
const STORE_SOFT_CAP: usize = 10_000;
/// Ring-buffer size for recent evictions (debug / observability).
const TERMINAL_RING_CAP: usize = 100;

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

/// Record of a recently evicted terminal order (debug / observability).
#[derive(Debug, Clone)]
pub struct TerminalRingEntry {
    pub intent_id: u64,
    pub state: OrderState,
    pub terminal_at: Instant,
}

pub struct OrderRouter {
    inbound_tx: mpsc::Sender<OrderIntent>,
    inbound_rx: tokio::sync::Mutex<Option<mpsc::Receiver<OrderIntent>>>,
    pub store: Arc<PendingOrderStore>,
    pub counters: Arc<RouterCounters>,
    /// Shared lock-free admission gate. Installed once via
    /// [`OrderRouter::set_risk_gate`] during engine startup.
    gate: OnceLock<Arc<StreamRiskGate>>,
    /// Bounded ring buffer of recently GC'd terminal entries.
    pub terminal_ring: Arc<Mutex<VecDeque<TerminalRingEntry>>>,
}

impl OrderRouter {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(INBOUND_DEPTH);
        Self {
            inbound_tx: tx,
            inbound_rx: tokio::sync::Mutex::new(Some(rx)),
            store: Arc::new(DashMap::new()),
            counters: Arc::new(RouterCounters::default()),
            gate: OnceLock::new(),
            terminal_ring: Arc::new(Mutex::new(VecDeque::with_capacity(TERMINAL_RING_CAP))),
        }
    }

    /// Install the shared [`StreamRiskGate`] used by [`OrderRouter::submit`].
    /// Call once at startup, before [`OrderRouter::spawn_workers`].
    pub fn set_risk_gate(&self, gate: Arc<StreamRiskGate>) {
        if self.gate.set(gate).is_err() {
            warn!("OrderRouter::set_risk_gate called more than once — ignoring");
        }
    }

    /// Cheap clone of the inbound sender, for subsystems (e.g. maker layering)
    /// that want to enqueue intents without owning the full `OrderRouter`.
    /// Skips the risk gate — the caller is expected to `try_admit` first.
    pub fn inbound_sender(&self) -> mpsc::Sender<OrderIntent> {
        self.inbound_tx.clone()
    }

    /// Async submit. Applies token-bucket admission (if a gate is installed)
    /// before enqueueing the intent for the dispatcher.
    ///
    /// - Admit  → enqueue.
    /// - Throttle → sleep `retry_in.min(50 ms)` then retry once. If the second
    ///   attempt still throttles, drop the intent.
    /// - Reject → drop the intent.
    pub async fn submit(&self, intent: OrderIntent) -> Result<(), RouterFull> {
        if let Some(gate) = self.gate.get() {
            let decision = gate.try_admit(&intent);
            match decision {
                AdmitDecision::Admit => {
                    hot_metrics::counters()
                        .risk_admits_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                AdmitDecision::Throttle { retry_in } => {
                    hot_metrics::counters()
                        .risk_throttles_total
                        .fetch_add(1, Ordering::Relaxed);
                    let sleep = retry_in.min(Duration::from_millis(50));
                    tokio::time::sleep(sleep).await;
                    match gate.try_admit(&intent) {
                        AdmitDecision::Admit => {
                            hot_metrics::counters()
                                .risk_admits_total
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        AdmitDecision::Throttle { .. } => {
                            hot_metrics::counters()
                                .risk_throttles_total
                                .fetch_add(1, Ordering::Relaxed);
                            warn!(
                                intent_id = intent.intent_id,
                                "OrderRouter: admission throttled twice — dropping intent"
                            );
                            self.counters.dropped_full.fetch_add(1, Ordering::Relaxed);
                            return Err(RouterFull);
                        }
                        AdmitDecision::Reject { reason } => {
                            bump_reject_counter(reason);
                            warn!(
                                intent_id = intent.intent_id,
                                reason, "OrderRouter: intent rejected by risk gate"
                            );
                            return Err(RouterFull);
                        }
                    }
                }
                AdmitDecision::Reject { reason } => {
                    bump_reject_counter(reason);
                    warn!(
                        intent_id = intent.intent_id,
                        reason, "OrderRouter: intent rejected by risk gate"
                    );
                    return Err(RouterFull);
                }
            }
        }

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
        let gate = self.gate.get().cloned();

        let mut worker_txs: Vec<mpsc::Sender<OrderIntent>> = Vec::with_capacity(SUBMIT_WORKERS);
        for _ in 0..SUBMIT_WORKERS {
            let (wtx, wrx) = mpsc::channel::<OrderIntent>(WORKER_QUEUE_DEPTH);
            worker_txs.push(wtx);

            let s = Arc::clone(&store);
            let c = Arc::clone(&counters);
            let exec = Arc::clone(&executor);
            let g = gate.clone();
            tokio::spawn(async move {
                let mut wrx = wrx;
                while let Some(intent) = wrx.recv().await {
                    submit_one(intent, &s, &c, &exec, g.as_ref()).await;
                }
            });
        }

        let s_disp = Arc::clone(&store);
        let c_disp = Arc::clone(&counters);
        let gate_for_dispatch = gate.clone();
        tokio::spawn(async move {
            let mut rx = rx;
            let mut rr = 0usize;
            while let Some(intent) = rx.recv().await {
                c_disp.inbound_depth.fetch_sub(1, Ordering::Relaxed);

                let pending = PendingOrder::new(
                    intent.intent_id,
                    intent.market_id.clone(),
                    intent.token_id.clone(),
                    intent.side,
                    intent.size_u64 as f64 / 1_000.0,
                    intent.size_u64,
                    intent.price_u64 as f64 / 1_000.0,
                );
                s_disp.insert(intent.intent_id, pending);
                hot_metrics::counters()
                    .pending_orders_count
                    .fetch_add(1, Ordering::Relaxed);

                if let Some(ref g) = gate_for_dispatch {
                    g.on_order_created(&intent.market_id, intent.size_u64);
                }

                if let Err(e) = worker_txs[rr].send(intent).await {
                    error!(
                        worker = rr,
                        "OrderRouter dispatcher: worker channel closed — intent dropped: {e}"
                    );
                }
                rr = (rr + 1) % SUBMIT_WORKERS;
            }
        });

        // ── Terminal-state GC sweeper ───────────────────────────────────────
        let s_gc = Arc::clone(&store);
        let ring_gc = Arc::clone(&self.terminal_ring);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(GC_INTERVAL);
            // First tick fires immediately; skip it so the store has time to fill.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                gc_sweep(&s_gc, &ring_gc);
            }
        });
    }

    pub fn pending_count(&self) -> usize {
        self.store.len()
    }

    /// Initiate a cancel for the given intent. Transitions the order to
    /// `Cancelling` and issues `DELETE /order/{id}`. The reconciler confirms
    /// the cancel and transitions to `Cancelled` (or `Filled` on a lost race).
    ///
    /// Returns `true` if the cancel RPC was dispatched (or the order was
    /// already in a terminal state — idempotent), `false` if the order is
    /// unknown or has no order_id yet.
    pub async fn cancel_order(&self, intent_id: u64, executor: &OrderExecutor) -> bool {
        let order_id = {
            let mut entry = match self.store.get_mut(&intent_id) {
                Some(e) => e,
                None => {
                    warn!(intent_id, "OrderRouter::cancel_order: intent_id not in store");
                    return false;
                }
            };

            if entry.state.is_terminal() || entry.state == OrderState::Cancelling {
                info!(
                    intent_id,
                    state = ?entry.state,
                    "OrderRouter::cancel_order: already terminal or cancelling — noop"
                );
                return true;
            }

            if !matches!(entry.state, OrderState::Acked | OrderState::PartialFilled) {
                warn!(
                    intent_id,
                    state = ?entry.state,
                    "OrderRouter::cancel_order: order not in cancellable state"
                );
                return false;
            }

            let id = match entry.order_id.clone() {
                Some(id) => id,
                None => {
                    warn!(intent_id, "OrderRouter::cancel_order: missing order_id");
                    return false;
                }
            };

            entry.cancel_attempts = entry.cancel_attempts.saturating_add(1);
            entry.transition(OrderState::Cancelling);
            id
        };

        hot_metrics::counters()
            .cancels_started
            .fetch_add(1, Ordering::Relaxed);

        match executor.cancel_order(&order_id).await {
            Ok(()) => {
                info!(
                    intent_id,
                    order_id,
                    "OrderRouter::cancel_order: DELETE dispatched — awaiting reconciler confirmation"
                );
                true
            }
            Err(e) => {
                error!(
                    intent_id,
                    order_id,
                    error = %e,
                    "OrderRouter::cancel_order: DELETE failed — keeping Cancelling, reconciler will retry"
                );
                // Stay in Cancelling; reconciler will confirm via GET /order/{id}.
                true
            }
        }
    }
}

impl Default for OrderRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Submit worker ────────────────────────────────────────────────────────────

/// Evicts terminal `PendingOrder` entries older than `TERMINAL_RETENTION`.
///
/// If the store exceeds `STORE_SOFT_CAP`, additionally evicts the oldest
/// terminal entries until back under the cap. Every eviction increments
/// `router_gc_evicted_total` and is pushed into the `terminal_ring` (capped
/// at `TERMINAL_RING_CAP`).
fn gc_sweep(
    store: &PendingOrderStore,
    ring: &Mutex<VecDeque<TerminalRingEntry>>,
) {
    // Collect candidates: (intent_id, state, terminal_at) for terminal entries.
    let mut terminals: Vec<(u64, OrderState, Instant)> = store
        .iter()
        .filter_map(|e| {
            let p = e.value();
            p.terminal_at
                .filter(|_| p.state.is_terminal())
                .map(|ts| (p.intent_id, p.state.clone(), ts))
        })
        .collect();

    // Oldest terminal first.
    terminals.sort_by_key(|(_, _, ts)| *ts);

    let now = Instant::now();
    let mut evicted = 0u64;

    // Phase 1: drop anything past retention.
    for (intent_id, state, terminal_at) in terminals.iter() {
        if now.duration_since(*terminal_at) > TERMINAL_RETENTION {
            if store.remove(intent_id).is_some() {
                push_ring(ring, TerminalRingEntry {
                    intent_id: *intent_id,
                    state: state.clone(),
                    terminal_at: *terminal_at,
                });
                evicted += 1;
            }
        }
    }

    // Phase 2: soft-cap enforcement (oldest terminal first, already sorted).
    if store.len() > STORE_SOFT_CAP {
        for (intent_id, state, terminal_at) in terminals.iter() {
            if store.len() <= STORE_SOFT_CAP {
                break;
            }
            if store.remove(intent_id).is_some() {
                push_ring(ring, TerminalRingEntry {
                    intent_id: *intent_id,
                    state: state.clone(),
                    terminal_at: *terminal_at,
                });
                evicted += 1;
            }
        }
    }

    if evicted > 0 {
        hot_metrics::counters()
            .router_gc_evicted_total
            .fetch_add(evicted, Ordering::Relaxed);
        info!(evicted, remaining = store.len(), "OrderRouter GC sweep");
    }
}

fn push_ring(ring: &Mutex<VecDeque<TerminalRingEntry>>, entry: TerminalRingEntry) {
    if let Ok(mut g) = ring.lock() {
        if g.len() >= TERMINAL_RING_CAP {
            g.pop_front();
        }
        g.push_back(entry);
    }
}

async fn submit_one(
    intent: OrderIntent,
    store: &PendingOrderStore,
    counters: &RouterCounters,
    executor: &OrderExecutor,
    gate: Option<&Arc<StreamRiskGate>>,
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
            if let Some(g) = gate {
                g.on_order_terminal(&intent.market_id, intent.size_u64);
            }
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
            if let Some(g) = gate {
                g.on_order_terminal(&intent.market_id, intent.size_u64);
            }
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
            if let Some(g) = gate {
                g.on_order_terminal(&intent.market_id, intent.size_u64);
            }
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
            if let Some(g) = gate {
                g.on_order_terminal(&intent.market_id, intent.size_u64);
            }
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
            if let Some(g) = gate {
                g.on_order_terminal(&intent.market_id, intent.size_u64);
            }
        }
    }
}

fn bump_reject_counter(reason: &str) {
    let c = hot_metrics::counters();
    match reason {
        "rate" => c.risk_rejects_rate.fetch_add(1, Ordering::Relaxed),
        "pending_count" => c.risk_rejects_pending_count.fetch_add(1, Ordering::Relaxed),
        "market_notional" => c.risk_rejects_market_notional.fetch_add(1, Ordering::Relaxed),
        "account_notional" => c.risk_rejects_account_notional.fetch_add(1, Ordering::Relaxed),
        "max_single_order" => c.risk_rejects_max_single_order.fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };
}
