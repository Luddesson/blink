//! # blink-ingress
//!
//! Pluggable ingress layer for the Blink HFT rebuild (Phase 2, §3 of the
//! plan). Every external data feed — the legacy RN1 REST poller, the
//! Polymarket CLOB WebSocket firehose, Polygon node log subscriptions, the
//! optional mempool tap — is modelled as a [`Source`]. Each source owns
//! a dedicated OS thread and pushes decoded [`blink_types::RawEvent`]s
//! into a per-source SPSC ring from [`blink_rings`].
//!
//! ## Backpressure discipline
//!
//! The ring is **bounded** and this crate is strict drop-on-full:
//! `Producer::push` returning `Err(_)` bumps `events_dropped` and discards
//! the event. Sources must never block and must never buffer unboundedly
//! — the downstream decision kernel is the authority on what is fresh
//! enough to act on. If a source sees its drop counter climb, that is a
//! signal to scale the ring or the decoder, not to buffer.
//!
//! ## Source lifecycle
//!
//! ```text
//!   let src: Box<dyn Source> = Box::new(ClobWsSource::new(cfg));
//!   let (prod, cons) = blink_rings::bounded::<RawEvent>(1 << 14);
//!   let handle = IngressRuntime::launch(src, prod, /*core*/ None);
//!   // … engine consumes `cons` …
//!   handle.shutdown();
//!   handle.join();
//! ```
//!
//! `Source::run` takes `Box<Self>` — it consumes the source. The only
//! state that survives the move is the shared [`SourceCounters`] Arc
//! that `stats_handle()` hands out before the move, which is how
//! observers read live ingest / drop / reconnect counters during the
//! run.
//!
//! ## Sources provided
//!
//! | Source                   | Kind                            | Notes                                   |
//! |--------------------------|---------------------------------|-----------------------------------------|
//! | [`Rn1RestSource`]        | [`SourceKind::Rn1Rest`]         | Legacy REST poller. Retired in p2-retire. |
//! | [`ClobWsSource`]         | [`SourceKind::ClobWs`]          | Polymarket CLOB activity WSS.           |
//! | [`BlockchainLogsSource`] | [`SourceKind::BlockchainLogs`]  | Polygon `eth_subscribe("logs",…)`.      |
//! | [`MempoolSource`]        | [`SourceKind::MempoolCtf`]      | `feature = "mempool-tap"`, gated.       |

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use blink_rings::{Consumer, Producer};
pub use blink_types::{RawEvent, SourceKind};

mod rn1_rest;
mod clob_ws;
mod blockchain_logs;
mod runtime;

#[cfg(feature = "mempool-tap")]
mod mempool;

pub use blockchain_logs::{BlockchainLogsConfig, BlockchainLogsSource};
pub use clob_ws::{ClobWsConfig, ClobWsSource};
pub use rn1_rest::{Rn1RestConfig, Rn1RestSource};
pub use runtime::{IngressRuntime, SourceHandle};

#[cfg(feature = "mempool-tap")]
pub use mempool::{MempoolConfig, MempoolSource};

// ─── ShutdownToken ────────────────────────────────────────────────────────

/// Cooperative shutdown signal passed to every [`Source::run`] invocation.
///
/// Backed by a plain atomic + a Tokio watch channel so it works equally
/// well inside blocking-thread loops and inside async tasks the source
/// spawns internally.
#[derive(Debug, Clone)]
pub struct ShutdownToken {
    flag: Arc<std::sync::atomic::AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl Default for ShutdownToken {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownToken {
    /// Create a fresh, un-tripped token.
    pub fn new() -> Self {
        Self {
            flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Trip the token. Idempotent; subsequent waiters return immediately.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Has shutdown been requested?
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Async wait — resolves as soon as [`Self::cancel`] is called.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let notified = self.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

// ─── Stats ────────────────────────────────────────────────────────────────

/// Shared, lock-free counters for a single source. Cloned cheaply via
/// `Arc` between the running source thread and external observers.
#[derive(Debug, Default)]
pub struct SourceCounters {
    /// Events successfully pushed into the sink ring.
    pub events_ingested: AtomicU64,
    /// Events the source produced but dropped because the ring was full.
    /// Critical HFT signal — a non-zero drop count means the decoder is
    /// outrunning the consumer or the ring is undersized.
    pub events_dropped: AtomicU64,
    /// Count of reconnect cycles (WS drops, REST 5xx bursts, …).
    pub reconnects: AtomicU64,
    /// Wall-clock nanoseconds of the most recently ingested event.
    pub last_event_wall_ns: AtomicU64,
}

impl SourceCounters {
    /// Fresh zeroed counters wrapped in an `Arc`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Snapshot to a plain-value [`SourceStats`].
    pub fn snapshot(&self) -> SourceStats {
        SourceStats {
            events_ingested: self.events_ingested.load(Ordering::Relaxed),
            events_dropped: self.events_dropped.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            last_event_wall_ns: self.last_event_wall_ns.load(Ordering::Relaxed),
        }
    }
}

/// Plain-value snapshot of a [`SourceCounters`]. Suitable for metrics
/// serialization and parity tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceStats {
    /// Events successfully handed to the sink ring.
    pub events_ingested: u64,
    /// Events produced but dropped (ring full).
    pub events_dropped: u64,
    /// Reconnect cycles.
    pub reconnects: u64,
    /// Wall-clock nanoseconds of the most recently ingested event.
    pub last_event_wall_ns: u64,
}

// ─── Source trait ─────────────────────────────────────────────────────────

/// A long-running ingress source.
///
/// Implementations own their external I/O handles (HTTP client, WS
/// connection, RPC socket) and decode frames into
/// [`blink_types::RawEvent`]s pushed strictly (drop-on-full) into the
/// provided [`Producer`]. Implementations SHOULD set
/// [`RawEvent::observe_only`] on mempool-derived events per the legal
/// memo in `docs/rebuild/R3_LEGAL_MEMO_STUB.md`.
pub trait Source: Send + 'static {
    /// Nominal [`SourceKind`] tag written into every emitted [`RawEvent`].
    fn kind(&self) -> SourceKind;

    /// Snapshot of the shared counters. Valid both before the source is
    /// consumed by [`Self::run`] and — via a previously cloned
    /// [`Self::stats_handle`] — during and after the run.
    fn stats(&self) -> SourceStats {
        self.stats_handle().snapshot()
    }

    /// Clone of the live counters. Callers who want to observe stats
    /// after handing the `Box<dyn Source>` to [`Self::run`] MUST grab
    /// this handle beforehand.
    fn stats_handle(&self) -> Arc<SourceCounters>;

    /// Run loop. Consumes the source; exits when `shutdown` is tripped
    /// or the underlying feed closes unrecoverably.
    ///
    /// Implementations MUST honour `shutdown.is_cancelled()` promptly
    /// (checked at least once per ingest loop iteration / between
    /// reconnect attempts).
    fn run(self: Box<Self>, sink: Producer<RawEvent>, shutdown: ShutdownToken);
}

/// Helper: strict-push with counter accounting. Used by every concrete
/// source to centralise the drop-on-full discipline. Returns `true` on
/// successful push, `false` on drop.
#[inline]
pub(crate) fn try_push(
    sink: &mut Producer<RawEvent>,
    counters: &SourceCounters,
    ev: RawEvent,
) -> bool {
    let wall = ev.wall_ns;
    match sink.push(ev) {
        Ok(()) => {
            counters.events_ingested.fetch_add(1, Ordering::Relaxed);
            counters.last_event_wall_ns.store(wall, Ordering::Relaxed);
            true
        }
        Err(_dropped) => {
            counters.events_dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_token_round_trips() {
        let tok = ShutdownToken::new();
        assert!(!tok.is_cancelled());
        let tok2 = tok.clone();
        let t = tokio::spawn(async move {
            tok2.cancelled().await;
            42u8
        });
        tok.cancel();
        assert_eq!(t.await.unwrap(), 42);
        assert!(tok.is_cancelled());
    }

    #[test]
    fn counters_snapshot_is_isolated() {
        let c = SourceCounters::new();
        c.events_ingested.fetch_add(3, Ordering::Relaxed);
        c.events_dropped.fetch_add(1, Ordering::Relaxed);
        let s = c.snapshot();
        assert_eq!(s.events_ingested, 3);
        assert_eq!(s.events_dropped, 1);
        c.events_ingested.fetch_add(1, Ordering::Relaxed);
        assert_eq!(s.events_ingested, 3, "snapshot is a value copy");
    }
}
