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
use std::time::Instant;

use hdrhistogram::Histogram;
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
    // ─── Phase 3: Risk admission counters/gauges ─────────────────────────────
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
        }
    }
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
            $out.push_str(&format!(
                "# TYPE {} counter\n{} {}\n",
                $name,
                $name,
                $val
            ));
        };
    }

    counter!(out, "blink_hot_signals_in_total",       c.signals_in.load(Ordering::Relaxed));
    counter!(out, "blink_hot_dedup_hits_total",        c.dedup_hits.load(Ordering::Relaxed));
    counter!(out, "blink_hot_submits_started_total",   c.submits_started.load(Ordering::Relaxed));
    counter!(out, "blink_hot_submits_ack_total",       c.submits_ack.load(Ordering::Relaxed));
    counter!(out, "blink_hot_submits_rejected_total",  c.submits_rejected.load(Ordering::Relaxed));
    counter!(out, "blink_hot_submit_unknown_total",    c.submit_unknown.load(Ordering::Relaxed));
    counter!(out, "blink_hot_cancels_started_total",   c.cancels_started.load(Ordering::Relaxed));
    counter!(out, "blink_hot_cancels_ack_total",       c.cancels_ack.load(Ordering::Relaxed));
    counter!(out, "blink_hot_heartbeat_misses_total",  c.heartbeat_misses.load(Ordering::Relaxed));
    counter!(out, "blink_hot_partial_fills_total",     c.partial_fills.load(Ordering::Relaxed));
    counter!(out, "blink_hot_full_fills_total",        c.full_fills.load(Ordering::Relaxed));
    counter!(out, "blink_hot_ws_reconnects_total",     c.ws_reconnects_total.load(Ordering::Relaxed));
    counter!(out, "blink_http_tls_handshakes_total",   c.tls_handshakes_total.load(Ordering::Relaxed));
    counter!(out, "blink_http_dns_lookups_total",      c.dns_lookups_total.load(Ordering::Relaxed));

    // Router counters
    counter!(out, "blink_router_dropped_full_total",     c.router_dropped_full.load(Ordering::Relaxed));
    counter!(out, "blink_router_retries_total",          c.router_retries_total.load(Ordering::Relaxed));
    counter!(out, "blink_router_reconcile_sweeps_total", c.router_reconcile_sweeps.load(Ordering::Relaxed));
    counter!(out, "blink_dedup_ws_wins_total",          c.ws_dedup_wins.load(Ordering::Relaxed));
    counter!(out, "blink_dedup_rest_wins_total",        c.rest_dedup_wins.load(Ordering::Relaxed));

    // Phase 3: Risk admission counters
    counter!(out, "blink_risk_admits_total",            c.risk_admits_total.load(Ordering::Relaxed));
    counter!(out, "blink_risk_throttles_total",         c.risk_throttles_total.load(Ordering::Relaxed));
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
