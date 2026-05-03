//! Lightweight per-stage latency instrumentation and hot-path counters.
//!
//! # Usage
//! ```
//! use engine::hot_metrics::{StageTimer, HotStage, counters};
//! let _t = StageTimer::start(HotStage::Enrich);
//! // ... work ...
//! // timer records elapsed_ns on drop
//! counters().signals_in.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//! ```

// hdrhistogram bucket-index arithmetic is monomorphised into this crate and
// triggers clippy::modulo_one even though the code is in a dependency.
#![allow(clippy::modulo_one)]

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use hdrhistogram::Histogram;
use serde::Serialize;
use tracing::info;

// ─── HotStage ────────────────────────────────────────────────────────────────

/// Pipeline stage identifiers for latency instrumentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotStage {
    Detect,
    QueueWait,
    Enrich,
    Sizing,
    Risk,
    Drift,
    Sign,
    Submit,
    Ack,
    Reconcile,
}

impl HotStage {
    const ALL: &'static [HotStage] = &[
        HotStage::Detect,
        HotStage::QueueWait,
        HotStage::Enrich,
        HotStage::Sizing,
        HotStage::Risk,
        HotStage::Drift,
        HotStage::Sign,
        HotStage::Submit,
        HotStage::Ack,
        HotStage::Reconcile,
    ];

    fn name(self) -> &'static str {
        match self {
            HotStage::Detect => "detect",
            HotStage::QueueWait => "queue_wait",
            HotStage::Enrich => "enrich",
            HotStage::Sizing => "sizing",
            HotStage::Risk => "risk",
            HotStage::Drift => "drift",
            HotStage::Sign => "sign",
            HotStage::Submit => "submit",
            HotStage::Ack => "ack",
            HotStage::Reconcile => "reconcile",
        }
    }

    fn index(self) -> usize {
        match self {
            HotStage::Detect => 0,
            HotStage::QueueWait => 1,
            HotStage::Enrich => 2,
            HotStage::Sizing => 3,
            HotStage::Risk => 4,
            HotStage::Drift => 5,
            HotStage::Sign => 6,
            HotStage::Submit => 7,
            HotStage::Ack => 8,
            HotStage::Reconcile => 9,
        }
    }
}

// ─── Per-stage histograms ─────────────────────────────────────────────────────

const NUM_STAGES: usize = 10;

struct StageHistograms {
    histograms: [Mutex<Histogram<u64>>; NUM_STAGES],
}

impl StageHistograms {
    fn new() -> Self {
        Self {
            histograms: std::array::from_fn(|_| {
                Mutex::new(Histogram::<u64>::new(3).expect("hdrhistogram init"))
            }),
        }
    }

    // hdrhistogram internals use integer modulo in bucket computations that
    // get monomorphised into this crate; suppress the lint here.
    #[allow(clippy::modulo_one)]
    fn record(&self, stage: HotStage, ns: u64) {
        if let Ok(mut h) = self.histograms[stage.index()].lock() {
            let _ = h.record(ns.max(1));
        }
    }

    #[allow(clippy::modulo_one)]
    fn percentiles(&self, stage: HotStage) -> (u64, u64, u64) {
        let Ok(h) = self.histograms[stage.index()].lock() else {
            return (0, 0, 0);
        };
        // value_at_quantile returns 0 for an empty histogram — safe to call unconditionally.
        (
            h.value_at_quantile(0.50),
            h.value_at_quantile(0.95),
            h.value_at_quantile(0.99),
        )
    }

    #[allow(clippy::modulo_one)]
    fn snapshot(&self, stage: HotStage) -> HotStageLatencySnapshot {
        let Ok(h) = self.histograms[stage.index()].lock() else {
            return HotStageLatencySnapshot::empty(stage.name());
        };
        HotStageLatencySnapshot::new(
            stage.name(),
            h.len(),
            h.value_at_quantile(0.50),
            h.value_at_quantile(0.95),
            h.value_at_quantile(0.99),
            h.max(),
        )
    }
}

// ─── HotCounters ─────────────────────────────────────────────────────────────

/// Atomic counters for hot-path events.
pub struct HotCounters {
    pub signals_in: AtomicU64,
    pub dedup_hits: AtomicU64,
    pub submits_started: AtomicU64,
    pub submits_ack: AtomicU64,
    pub submits_rejected: AtomicU64,
    pub submit_unknown: AtomicU64,
    pub cancels_started: AtomicU64,
    pub cancels_ack: AtomicU64,
    pub heartbeat_misses: AtomicU64,
    pub reconcile_lag_ms_last: AtomicI64,
    pub partial_fills: AtomicU64,
    pub full_fills: AtomicU64,
    /// Total WebSocket reconnect attempts.
    pub ws_reconnects_total: AtomicU64,
    /// Milliseconds between the last two WS sessions (gap latency).
    pub ws_gap_ms_last: AtomicI64,
    /// In-flight HTTP submit requests (incremented on send, decremented on
    /// response/timeout). Exposed as `blink_http_submit_inflight` gauge.
    pub http_submit_inflight: AtomicI64,
    // TODO: wire a custom reqwest connector/middleware to increment these.
    /// Total TLS handshakes initiated (best-effort; requires custom connector).
    pub tls_handshakes_total: AtomicU64,
    /// Total DNS lookups initiated (best-effort; requires custom resolver hook).
    pub dns_lookups_total: AtomicU64,
    // ─── Router-specific counters (Phase 2) ──────────────────────────────────
    pub router_dropped_full: AtomicU64,
    pub router_retries_total: AtomicU64,
    pub router_reconcile_sweeps: AtomicU64,
    pub pending_orders_count: AtomicI64,
    /// Deduped duplicates where the WS path had published first.
    pub ws_dedup_wins: AtomicU64,
    /// Deduped duplicates where the REST path had published first.
    pub rest_dedup_wins: AtomicU64,
    // ─── Pre-trade gate counters (Phase 3) ───────────────────────────────────
    pub gate_proceed: AtomicU64,
    pub gate_skip_stale: AtomicU64,
    pub gate_skip_drift: AtomicU64,
    pub gate_skip_post_only: AtomicU64,
    // ─── Signal pipeline counters (Phase 3) ──────────────────────────────────
    pub signal_worker_queue_depth_max: AtomicU64,
    pub signal_dispatcher_backlog: AtomicU64,
    pub signal_per_token_workers_active: AtomicU64,
    pub signal_pertoken_overflow_dropped: AtomicU64,
    pub signal_dispatcher_dropped: AtomicU64,
    // ─── Phase 3 fill/cancel/GC counters ─────────────────────────────────────
    pub submit_unknown_resolved_acked_total: AtomicU64,
    pub submit_unknown_resolved_rejected_total: AtomicU64,
    pub submit_unknown_lookup_attempts_total: AtomicU64,
    pub cancel_success_total: AtomicU64,
    pub cancel_reject_total: AtomicU64,
    pub fills_delta_size_last: AtomicI64,
    pub partial_fill_ratio_permille: AtomicI64,
    pub router_gc_evicted_total: AtomicU64,
    // ─── Phase 3 risk admission counters/gauges ──────────────────────────────
    pub risk_admits_total: AtomicU64,
    pub risk_throttles_total: AtomicU64,
    pub risk_rejects_rate: AtomicU64,
    pub risk_rejects_pending_count: AtomicU64,
    pub risk_rejects_market_notional: AtomicU64,
    pub risk_rejects_account_notional: AtomicU64,
    pub risk_rejects_max_single_order: AtomicU64,
    pub risk_tokens_available: AtomicU64,
    pub risk_cancel_tokens_available: AtomicU64,
    pub risk_per_market_pending_max: AtomicU64,

    // Phase 5: maker-layering.
    pub maker_layers_planned_total: AtomicU64,
    pub maker_layers_placed_total: AtomicU64,
    pub maker_layers_reprice_total: AtomicU64,
    pub maker_layers_stale_evictions_total: AtomicU64,
    pub maker_active_layers: AtomicU64,
}

impl HotCounters {
    fn new() -> Self {
        Self {
            signals_in: AtomicU64::new(0),
            dedup_hits: AtomicU64::new(0),
            submits_started: AtomicU64::new(0),
            submits_ack: AtomicU64::new(0),
            submits_rejected: AtomicU64::new(0),
            submit_unknown: AtomicU64::new(0),
            cancels_started: AtomicU64::new(0),
            cancels_ack: AtomicU64::new(0),
            heartbeat_misses: AtomicU64::new(0),
            reconcile_lag_ms_last: AtomicI64::new(0),
            partial_fills: AtomicU64::new(0),
            full_fills: AtomicU64::new(0),
            ws_reconnects_total: AtomicU64::new(0),
            ws_gap_ms_last: AtomicI64::new(0),
            http_submit_inflight: AtomicI64::new(0),
            tls_handshakes_total: AtomicU64::new(0),
            dns_lookups_total: AtomicU64::new(0),
            router_dropped_full: AtomicU64::new(0),
            router_retries_total: AtomicU64::new(0),
            router_reconcile_sweeps: AtomicU64::new(0),
            pending_orders_count: AtomicI64::new(0),
            ws_dedup_wins: AtomicU64::new(0),
            rest_dedup_wins: AtomicU64::new(0),
            gate_proceed: AtomicU64::new(0),
            gate_skip_stale: AtomicU64::new(0),
            gate_skip_drift: AtomicU64::new(0),
            gate_skip_post_only: AtomicU64::new(0),
            signal_worker_queue_depth_max: AtomicU64::new(0),
            signal_dispatcher_backlog: AtomicU64::new(0),
            signal_per_token_workers_active: AtomicU64::new(0),
            signal_pertoken_overflow_dropped: AtomicU64::new(0),
            signal_dispatcher_dropped: AtomicU64::new(0),
            submit_unknown_resolved_acked_total: AtomicU64::new(0),
            submit_unknown_resolved_rejected_total: AtomicU64::new(0),
            submit_unknown_lookup_attempts_total: AtomicU64::new(0),
            cancel_success_total: AtomicU64::new(0),
            cancel_reject_total: AtomicU64::new(0),
            fills_delta_size_last: AtomicI64::new(0),
            partial_fill_ratio_permille: AtomicI64::new(0),
            router_gc_evicted_total: AtomicU64::new(0),
            risk_admits_total: AtomicU64::new(0),
            risk_throttles_total: AtomicU64::new(0),
            risk_rejects_rate: AtomicU64::new(0),
            risk_rejects_pending_count: AtomicU64::new(0),
            risk_rejects_market_notional: AtomicU64::new(0),
            risk_rejects_account_notional: AtomicU64::new(0),
            risk_rejects_max_single_order: AtomicU64::new(0),
            risk_tokens_available: AtomicU64::new(0),
            risk_cancel_tokens_available: AtomicU64::new(0),
            risk_per_market_pending_max: AtomicU64::new(0),
            maker_layers_planned_total: AtomicU64::new(0),
            maker_layers_placed_total: AtomicU64::new(0),
            maker_layers_reprice_total: AtomicU64::new(0),
            maker_layers_stale_evictions_total: AtomicU64::new(0),
            maker_active_layers: AtomicU64::new(0),
        }
    }
}

// ─── JSON snapshots ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct HotStageLatencySnapshot {
    pub stage: &'static str,
    pub samples: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    pub p50_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl HotStageLatencySnapshot {
    fn empty(stage: &'static str) -> Self {
        Self::new(stage, 0, 0, 0, 0, 0)
    }

    fn new(
        stage: &'static str,
        samples: u64,
        p50_ns: u64,
        p95_ns: u64,
        p99_ns: u64,
        max_ns: u64,
    ) -> Self {
        Self {
            stage,
            samples,
            p50_ns,
            p95_ns,
            p99_ns,
            max_ns,
            p50_us: ns_to_us(p50_ns),
            p95_us: ns_to_us(p95_ns),
            p99_us: ns_to_us(p99_ns),
            max_us: ns_to_us(max_ns),
            p50_ms: ns_to_ms(p50_ns),
            p95_ms: ns_to_ms(p95_ns),
            p99_ms: ns_to_ms(p99_ns),
            max_ms: ns_to_ms(max_ns),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HotCountersSnapshot {
    pub signals_in: u64,
    pub dedup_hits: u64,
    pub submits_started: u64,
    pub submits_ack: u64,
    pub submits_rejected: u64,
    pub submit_unknown: u64,
    pub cancels_started: u64,
    pub cancels_ack: u64,
    pub heartbeat_misses: u64,
    pub reconcile_lag_ms_last: i64,
    pub partial_fills: u64,
    pub full_fills: u64,
    pub ws_reconnects_total: u64,
    pub ws_gap_ms_last: i64,
    pub http_submit_inflight: i64,
    pub tls_handshakes_total: u64,
    pub dns_lookups_total: u64,
    pub router_dropped_full: u64,
    pub router_retries_total: u64,
    pub router_reconcile_sweeps: u64,
    pub pending_orders_count: i64,
    pub ws_dedup_wins: u64,
    pub rest_dedup_wins: u64,
    pub gate_proceed: u64,
    pub gate_skip_stale: u64,
    pub gate_skip_drift: u64,
    pub gate_skip_post_only: u64,
    pub signal_worker_queue_depth_max: u64,
    pub signal_dispatcher_backlog: u64,
    pub signal_per_token_workers_active: u64,
    pub signal_pertoken_overflow_dropped: u64,
    pub signal_dispatcher_dropped: u64,
    pub submit_unknown_resolved_acked_total: u64,
    pub submit_unknown_resolved_rejected_total: u64,
    pub submit_unknown_lookup_attempts_total: u64,
    pub cancel_success_total: u64,
    pub cancel_reject_total: u64,
    pub fills_delta_size_last: i64,
    pub partial_fill_ratio_permille: i64,
    pub router_gc_evicted_total: u64,
    pub risk_admits_total: u64,
    pub risk_throttles_total: u64,
    pub risk_rejects_rate: u64,
    pub risk_rejects_pending_count: u64,
    pub risk_rejects_market_notional: u64,
    pub risk_rejects_account_notional: u64,
    pub risk_rejects_max_single_order: u64,
    pub risk_tokens_available: u64,
    pub risk_cancel_tokens_available: u64,
    pub risk_per_market_pending_max: u64,
    pub maker_layers_planned_total: u64,
    pub maker_layers_placed_total: u64,
    pub maker_layers_reprice_total: u64,
    pub maker_layers_stale_evictions_total: u64,
    pub maker_active_layers: u64,
}

impl HotCountersSnapshot {
    fn from_counters(c: &HotCounters) -> Self {
        Self {
            signals_in: c.signals_in.load(Ordering::Relaxed),
            dedup_hits: c.dedup_hits.load(Ordering::Relaxed),
            submits_started: c.submits_started.load(Ordering::Relaxed),
            submits_ack: c.submits_ack.load(Ordering::Relaxed),
            submits_rejected: c.submits_rejected.load(Ordering::Relaxed),
            submit_unknown: c.submit_unknown.load(Ordering::Relaxed),
            cancels_started: c.cancels_started.load(Ordering::Relaxed),
            cancels_ack: c.cancels_ack.load(Ordering::Relaxed),
            heartbeat_misses: c.heartbeat_misses.load(Ordering::Relaxed),
            reconcile_lag_ms_last: c.reconcile_lag_ms_last.load(Ordering::Relaxed),
            partial_fills: c.partial_fills.load(Ordering::Relaxed),
            full_fills: c.full_fills.load(Ordering::Relaxed),
            ws_reconnects_total: c.ws_reconnects_total.load(Ordering::Relaxed),
            ws_gap_ms_last: c.ws_gap_ms_last.load(Ordering::Relaxed),
            http_submit_inflight: c.http_submit_inflight.load(Ordering::Relaxed),
            tls_handshakes_total: c.tls_handshakes_total.load(Ordering::Relaxed),
            dns_lookups_total: c.dns_lookups_total.load(Ordering::Relaxed),
            router_dropped_full: c.router_dropped_full.load(Ordering::Relaxed),
            router_retries_total: c.router_retries_total.load(Ordering::Relaxed),
            router_reconcile_sweeps: c.router_reconcile_sweeps.load(Ordering::Relaxed),
            pending_orders_count: c.pending_orders_count.load(Ordering::Relaxed),
            ws_dedup_wins: c.ws_dedup_wins.load(Ordering::Relaxed),
            rest_dedup_wins: c.rest_dedup_wins.load(Ordering::Relaxed),
            gate_proceed: c.gate_proceed.load(Ordering::Relaxed),
            gate_skip_stale: c.gate_skip_stale.load(Ordering::Relaxed),
            gate_skip_drift: c.gate_skip_drift.load(Ordering::Relaxed),
            gate_skip_post_only: c.gate_skip_post_only.load(Ordering::Relaxed),
            signal_worker_queue_depth_max: c.signal_worker_queue_depth_max.load(Ordering::Relaxed),
            signal_dispatcher_backlog: c.signal_dispatcher_backlog.load(Ordering::Relaxed),
            signal_per_token_workers_active: c
                .signal_per_token_workers_active
                .load(Ordering::Relaxed),
            signal_pertoken_overflow_dropped: c
                .signal_pertoken_overflow_dropped
                .load(Ordering::Relaxed),
            signal_dispatcher_dropped: c.signal_dispatcher_dropped.load(Ordering::Relaxed),
            submit_unknown_resolved_acked_total: c
                .submit_unknown_resolved_acked_total
                .load(Ordering::Relaxed),
            submit_unknown_resolved_rejected_total: c
                .submit_unknown_resolved_rejected_total
                .load(Ordering::Relaxed),
            submit_unknown_lookup_attempts_total: c
                .submit_unknown_lookup_attempts_total
                .load(Ordering::Relaxed),
            cancel_success_total: c.cancel_success_total.load(Ordering::Relaxed),
            cancel_reject_total: c.cancel_reject_total.load(Ordering::Relaxed),
            fills_delta_size_last: c.fills_delta_size_last.load(Ordering::Relaxed),
            partial_fill_ratio_permille: c.partial_fill_ratio_permille.load(Ordering::Relaxed),
            router_gc_evicted_total: c.router_gc_evicted_total.load(Ordering::Relaxed),
            risk_admits_total: c.risk_admits_total.load(Ordering::Relaxed),
            risk_throttles_total: c.risk_throttles_total.load(Ordering::Relaxed),
            risk_rejects_rate: c.risk_rejects_rate.load(Ordering::Relaxed),
            risk_rejects_pending_count: c.risk_rejects_pending_count.load(Ordering::Relaxed),
            risk_rejects_market_notional: c.risk_rejects_market_notional.load(Ordering::Relaxed),
            risk_rejects_account_notional: c.risk_rejects_account_notional.load(Ordering::Relaxed),
            risk_rejects_max_single_order: c.risk_rejects_max_single_order.load(Ordering::Relaxed),
            risk_tokens_available: c.risk_tokens_available.load(Ordering::Relaxed),
            risk_cancel_tokens_available: c.risk_cancel_tokens_available.load(Ordering::Relaxed),
            risk_per_market_pending_max: c.risk_per_market_pending_max.load(Ordering::Relaxed),
            maker_layers_planned_total: c.maker_layers_planned_total.load(Ordering::Relaxed),
            maker_layers_placed_total: c.maker_layers_placed_total.load(Ordering::Relaxed),
            maker_layers_reprice_total: c.maker_layers_reprice_total.load(Ordering::Relaxed),
            maker_layers_stale_evictions_total: c
                .maker_layers_stale_evictions_total
                .load(Ordering::Relaxed),
            maker_active_layers: c.maker_active_layers.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HotMetricsSnapshot {
    pub generated_at_ms: u128,
    pub stages: Vec<HotStageLatencySnapshot>,
    pub bottleneck: Option<HotStageLatencySnapshot>,
    pub counters: HotCountersSnapshot,
}

/// Returns a JSON-friendly snapshot of hot-path latency and counters.
pub fn snapshot() -> HotMetricsSnapshot {
    let m = global();
    let stages: Vec<_> = HotStage::ALL
        .iter()
        .map(|&stage| m.histograms.snapshot(stage))
        .collect();
    let bottleneck = stages
        .iter()
        .filter(|stage| stage.samples > 0)
        .max_by_key(|stage| stage.p99_ns)
        .cloned();

    HotMetricsSnapshot {
        generated_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        stages,
        bottleneck,
        counters: HotCountersSnapshot::from_counters(&m.counters),
    }
}

fn ns_to_us(ns: u64) -> f64 {
    ns as f64 / 1_000.0
}

fn ns_to_ms(ns: u64) -> f64 {
    ns as f64 / 1_000_000.0
}

// ─── Global singleton ─────────────────────────────────────────────────────────

struct HotMetrics {
    histograms: StageHistograms,
    counters: HotCounters,
}

impl HotMetrics {
    fn new() -> Self {
        Self {
            histograms: StageHistograms::new(),
            counters: HotCounters::new(),
        }
    }
}

static INSTANCE: OnceLock<HotMetrics> = OnceLock::new();

fn global() -> &'static HotMetrics {
    INSTANCE.get_or_init(HotMetrics::new)
}

/// Returns a reference to the global atomic counters.
pub fn counters() -> &'static HotCounters {
    &global().counters
}

fn record_stage(stage: HotStage, ns: u64) {
    global().histograms.record(stage, ns);
}

// ─── StageTimer RAII guard ────────────────────────────────────────────────────

/// RAII guard that records stage elapsed time in nanoseconds on drop.
pub struct StageTimer {
    stage: HotStage,
    start: Instant,
}

impl StageTimer {
    /// Start timing a stage. Elapsed ns is recorded into the histogram on drop.
    #[inline]
    pub fn start(stage: HotStage) -> Self {
        Self {
            stage,
            start: Instant::now(),
        }
    }

    /// Build a timer from a previously captured `Instant` (e.g. `enqueued_at`).
    #[inline]
    pub fn from_instant(stage: HotStage, start: Instant) -> Self {
        Self { stage, start }
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        record_stage(self.stage, self.start.elapsed().as_nanos() as u64);
    }
}

// ─── Prometheus-compatible text dump ─────────────────────────────────────────

/// Returns a Prometheus text-format metrics string.
///
/// Wire into the existing `/api/metrics` endpoint (see `web_server.rs`).
/// TODO: Wire into HTTP endpoint once web_server AppState gains a hot_metrics field.
pub fn render_prom() -> String {
    let mut out = String::with_capacity(4096);
    let m = global();
    let c = &m.counters;

    macro_rules! counter {
        ($out:expr, $name:literal, $val:expr) => {
            $out.push_str(&format!("# TYPE {} counter\n{} {}\n", $name, $name, $val));
        };
    }

    counter!(
        out,
        "blink_hot_signals_in_total",
        c.signals_in.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_dedup_hits_total",
        c.dedup_hits.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_submits_started_total",
        c.submits_started.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_submits_ack_total",
        c.submits_ack.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_submits_rejected_total",
        c.submits_rejected.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_submit_unknown_total",
        c.submit_unknown.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_cancels_started_total",
        c.cancels_started.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_cancels_ack_total",
        c.cancels_ack.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_heartbeat_misses_total",
        c.heartbeat_misses.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_partial_fills_total",
        c.partial_fills.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_full_fills_total",
        c.full_fills.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_hot_ws_reconnects_total",
        c.ws_reconnects_total.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_http_tls_handshakes_total",
        c.tls_handshakes_total.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_http_dns_lookups_total",
        c.dns_lookups_total.load(Ordering::Relaxed)
    );

    // Router counters
    counter!(
        out,
        "blink_router_dropped_full_total",
        c.router_dropped_full.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_router_retries_total",
        c.router_retries_total.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_router_reconcile_sweeps_total",
        c.router_reconcile_sweeps.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_dedup_ws_wins_total",
        c.ws_dedup_wins.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_dedup_rest_wins_total",
        c.rest_dedup_wins.load(Ordering::Relaxed)
    );

    // Pre-trade gate counters
    counter!(
        out,
        "blink_gate_proceed_total",
        c.gate_proceed.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_gate_skip_stale_total",
        c.gate_skip_stale.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_gate_skip_drift_total",
        c.gate_skip_drift.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_gate_skip_post_only_total",
        c.gate_skip_post_only.load(Ordering::Relaxed)
    );

    // Signal pipeline counters
    counter!(
        out,
        "blink_signal_pertoken_overflow_dropped_total",
        c.signal_pertoken_overflow_dropped.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_signal_dispatcher_dropped_total",
        c.signal_dispatcher_dropped.load(Ordering::Relaxed)
    );
    let workers_active = c.signal_per_token_workers_active.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_signal_per_token_workers_active gauge\nblink_signal_per_token_workers_active {workers_active}\n"
    ));
    let worker_queue_depth_max = c.signal_worker_queue_depth_max.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_signal_worker_queue_depth_max gauge\nblink_signal_worker_queue_depth_max {worker_queue_depth_max}\n"
    ));
    let dispatcher_backlog = c.signal_dispatcher_backlog.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_signal_dispatcher_backlog gauge\nblink_signal_dispatcher_backlog {dispatcher_backlog}\n"
    ));

    // Phase 3: Risk admission counters
    counter!(
        out,
        "blink_risk_admits_total",
        c.risk_admits_total.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_risk_throttles_total",
        c.risk_throttles_total.load(Ordering::Relaxed)
    );
    out.push_str("# TYPE blink_risk_rejects_total counter\n");
    out.push_str(&format!(
        "blink_risk_rejects_total{{reason=\"rate\"}} {}\n",
        c.risk_rejects_rate.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "blink_risk_rejects_total{{reason=\"pending_count\"}} {}\n",
        c.risk_rejects_pending_count.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "blink_risk_rejects_total{{reason=\"market_notional\"}} {}\n",
        c.risk_rejects_market_notional.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "blink_risk_rejects_total{{reason=\"account_notional\"}} {}\n",
        c.risk_rejects_account_notional.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "blink_risk_rejects_total{{reason=\"max_single_order\"}} {}\n",
        c.risk_rejects_max_single_order.load(Ordering::Relaxed)
    ));
    let risk_tokens = c.risk_tokens_available.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_risk_tokens_available gauge\nblink_risk_tokens_available {risk_tokens}\n"
    ));
    let risk_cancel_tokens = c.risk_cancel_tokens_available.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_risk_cancel_tokens_available gauge\nblink_risk_cancel_tokens_available {risk_cancel_tokens}\n"
    ));
    let risk_pm_pending = c.risk_per_market_pending_max.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_risk_per_market_pending_max gauge\nblink_risk_per_market_pending_max {risk_pm_pending}\n"
    ));
    let pending = c.pending_orders_count.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_router_pending_orders_count gauge\nblink_router_pending_orders_count {pending}\n"
    ));

    let lag = c.reconcile_lag_ms_last.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_hot_reconcile_lag_ms gauge\nblink_hot_reconcile_lag_ms {lag}\n"
    ));
    let ws_gap = c.ws_gap_ms_last.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_hot_ws_gap_ms_last gauge\nblink_hot_ws_gap_ms_last {ws_gap}\n"
    ));
    let inflight = c.http_submit_inflight.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_http_submit_inflight gauge\nblink_http_submit_inflight {inflight}\n"
    ));
    // http_submit_p99_ms is exposed via blink_stage_submit_ns{quantile="0.99"} / 1_000_000.
    let (_, _, submit_p99_ns) = m.histograms.percentiles(HotStage::Submit);
    let submit_p99_ms = submit_p99_ns / 1_000_000;
    out.push_str(&format!(
        "# TYPE blink_http_submit_p99_ms gauge\nblink_http_submit_p99_ms {submit_p99_ms}\n"
    ));

    // Phase 3 counters
    counter!(
        out,
        "blink_submit_unknown_resolved_acked_total",
        c.submit_unknown_resolved_acked_total
            .load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_submit_unknown_resolved_rejected_total",
        c.submit_unknown_resolved_rejected_total
            .load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_submit_unknown_lookup_attempts_total",
        c.submit_unknown_lookup_attempts_total
            .load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_cancel_success_total",
        c.cancel_success_total.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_cancel_reject_total",
        c.cancel_reject_total.load(Ordering::Relaxed)
    );
    counter!(
        out,
        "blink_router_gc_evicted_total",
        c.router_gc_evicted_total.load(Ordering::Relaxed)
    );
    let fills_delta = c.fills_delta_size_last.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_fills_delta_size_last gauge\nblink_fills_delta_size_last {fills_delta}\n"
    ));
    let ratio = c.partial_fill_ratio_permille.load(Ordering::Relaxed);
    out.push_str(&format!(
        "# TYPE blink_partial_fill_ratio_permille gauge\nblink_partial_fill_ratio_permille {ratio}\n"
    ));

    for &stage in HotStage::ALL {
        let (p50, p95, p99) = m.histograms.percentiles(stage);
        let name = stage.name();
        out.push_str(&format!(
            "# TYPE blink_stage_{name}_ns summary\n\
             blink_stage_{name}_ns{{quantile=\"0.5\"}} {p50}\n\
             blink_stage_{name}_ns{{quantile=\"0.95\"}} {p95}\n\
             blink_stage_{name}_ns{{quantile=\"0.99\"}} {p99}\n"
        ));
    }

    out
}

// ─── Periodic snapshot logger ─────────────────────────────────────────────────

/// Spawns a Tokio task that logs a structured JSON snapshot every 10 seconds at INFO.
pub fn spawn_periodic_logger() {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            log_snapshot();
        }
    });
}

fn log_snapshot() {
    let m = global();
    let c = &m.counters;
    let mut stages = serde_json::Map::new();
    for &stage in HotStage::ALL {
        let (p50, p95, p99) = m.histograms.percentiles(stage);
        stages.insert(
            stage.name().to_string(),
            serde_json::json!({ "p50_ns": p50, "p95_ns": p95, "p99_ns": p99 }),
        );
    }
    info!(
        tag = "hot_metrics_snapshot",
        signals_in        = c.signals_in.load(Ordering::Relaxed),
        dedup_hits        = c.dedup_hits.load(Ordering::Relaxed),
        submits_started   = c.submits_started.load(Ordering::Relaxed),
        submits_ack       = c.submits_ack.load(Ordering::Relaxed),
        submits_rejected  = c.submits_rejected.load(Ordering::Relaxed),
        submit_unknown    = c.submit_unknown.load(Ordering::Relaxed),
        cancels_started   = c.cancels_started.load(Ordering::Relaxed),
        cancels_ack       = c.cancels_ack.load(Ordering::Relaxed),
        heartbeat_misses  = c.heartbeat_misses.load(Ordering::Relaxed),
        partial_fills     = c.partial_fills.load(Ordering::Relaxed),
        full_fills        = c.full_fills.load(Ordering::Relaxed),
        reconcile_lag_ms  = c.reconcile_lag_ms_last.load(Ordering::Relaxed),
        stages            = %serde_json::to_string(&stages).unwrap_or_default(),
        "hot_metrics_snapshot"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_snapshot_reports_samples_and_unit_conversions() {
        let histograms = StageHistograms::new();

        let empty = histograms.snapshot(HotStage::Submit);
        assert_eq!(empty.stage, "submit");
        assert_eq!(empty.samples, 0);
        assert_eq!(empty.p99_ns, 0);
        assert_eq!(empty.p99_us, 0.0);
        assert_eq!(empty.p99_ms, 0.0);

        histograms.record(HotStage::Submit, 1_000);
        histograms.record(HotStage::Submit, 2_000_000);

        let snap = histograms.snapshot(HotStage::Submit);
        assert_eq!(snap.stage, "submit");
        assert_eq!(snap.samples, 2);
        assert!(snap.p50_ns >= 1_000);
        assert!(snap.p95_ns >= 1_000);
        assert!(snap.p99_ns >= 1_000);
        assert!(snap.max_ns >= 2_000_000);
        assert!(snap.p99_us > 0.0);
        assert!(snap.p99_ms > 0.0);
    }

    #[test]
    fn global_snapshot_includes_bottleneck_and_counters() {
        let expected_signals = counters()
            .signals_in
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        record_stage(HotStage::Risk, 25_000);

        let snap = snapshot();
        assert!(snap.generated_at_ms > 0);
        assert!(snap.counters.signals_in >= expected_signals);
        assert!(snap.bottleneck.is_some());

        let risk = snap
            .stages
            .iter()
            .find(|stage| stage.stage == "risk")
            .expect("risk stage present");
        assert!(risk.samples > 0);
        assert!(risk.p99_ns > 0);
    }
}
