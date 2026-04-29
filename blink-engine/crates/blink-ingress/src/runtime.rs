//! `IngressRuntime` — spawns pinned OS threads for a collection of
//! [`Source`]s, wires each to its own SPSC ring, and returns a list of
//! [`Consumer`]s for the engine to drain.
//!
//! Fan-in policy is left to the caller (per the task spec): this crate
//! returns `Vec<Consumer<RawEvent>>` rather than merging rings, keeping
//! the ingress layer policy-free.

use std::sync::Arc;
use std::thread::JoinHandle;

use blink_rings::{bounded, Consumer};
use blink_types::RawEvent;

use crate::{Source, SourceCounters, ShutdownToken};

/// Handle to a running source.
pub struct SourceHandle {
    /// Human-readable tag (source kind + index) for logs / metrics.
    pub name: String,
    /// Live shared counters (snapshot via `counters.snapshot()`).
    pub counters: Arc<SourceCounters>,
    /// OS thread running the source's `run` loop.
    pub thread: JoinHandle<()>,
}

/// Ingress runtime — owns spawned source threads.
pub struct IngressRuntime {
    shutdown: ShutdownToken,
    handles: Vec<SourceHandle>,
}

impl IngressRuntime {
    /// Spawn one pinned OS thread per source. `ring_cap_pow2` is the
    /// capacity of each per-source ring (must be a power of two —
    /// enforced by `blink_rings::bounded`). `core_hint_start` is the
    /// starting CPU id for affinity pinning; sources are assigned
    /// sequential cores `core_hint_start + i`. Pass `None` to skip
    /// affinity (useful for tests / dev laptops).
    pub fn launch(
        sources: Vec<Box<dyn Source>>,
        ring_cap_pow2: usize,
        core_hint_start: Option<usize>,
    ) -> (Self, Vec<Consumer<RawEvent>>) {
        let shutdown = ShutdownToken::new();
        let mut handles = Vec::with_capacity(sources.len());
        let mut consumers = Vec::with_capacity(sources.len());

        for (i, src) in sources.into_iter().enumerate() {
            let kind = src.kind();
            let counters = src.stats_handle();
            let (prod, cons) = bounded::<RawEvent>(ring_cap_pow2);
            let name = format!("ingress-{kind:?}-{i}");
            let shutdown_clone = shutdown.clone();

            let thread = match core_hint_start {
                Some(start) => {
                    let core = start + i;
                    blink_rings::affinity::spawn_pinned(core, &name, move || {
                        src.run(prod, shutdown_clone);
                    })
                }
                None => {
                    let nm = name.clone();
                    std::thread::Builder::new()
                        .name(nm)
                        .spawn(move || {
                            src.run(prod, shutdown_clone);
                        })
                        .expect("ingress: spawn failed")
                }
            };

            handles.push(SourceHandle {
                name,
                counters,
                thread,
            });
            consumers.push(cons);
        }

        (Self { shutdown, handles }, consumers)
    }

    /// Trip shutdown on every running source. Safe to call multiple
    /// times.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    /// Live per-source handles (names + counters + thread).
    pub fn handles(&self) -> &[SourceHandle] {
        &self.handles
    }

    /// Clone of the shutdown token — handy if the caller wants its own
    /// downstream tasks to react to the same signal.
    pub fn shutdown_token(&self) -> ShutdownToken {
        self.shutdown.clone()
    }

    /// Trip shutdown and join every source thread.
    pub fn join(self) {
        self.shutdown.cancel();
        for h in self.handles {
            let _ = h.thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Source, SourceCounters, ShutdownToken, try_push};
    use blink_rings::Producer;
    use blink_timestamps::{init_with_policy, InitPolicy, Timestamp};
    use blink_types::{EventId, RawEvent, SourceKind, wall_clock_ns};
    use std::time::Duration;

    struct CountingSource {
        counters: Arc<SourceCounters>,
        emit: u32,
    }
    impl Source for CountingSource {
        fn kind(&self) -> SourceKind {
            SourceKind::Manual
        }
        fn stats_handle(&self) -> Arc<SourceCounters> {
            self.counters.clone()
        }
        fn run(self: Box<Self>, mut sink: Producer<RawEvent>, shutdown: ShutdownToken) {
            let _ = init_with_policy(InitPolicy::AllowFallback);
            for _ in 0..self.emit {
                if shutdown.is_cancelled() {
                    return;
                }
                let ev = RawEvent {
                    event_id: EventId::fetch_next(),
                    source: SourceKind::Manual,
                    source_seq: 0,
                    anchor: None,
                    token_id: String::new(),
                    market_id: None,
                    side: None,
                    price: None,
                    size: None,
                    tsc_in: Timestamp::now(),
                    wall_ns: wall_clock_ns(),
                    extra: None,
                    observe_only: false,
                    maker_wallet: None,
                };
                try_push(&mut sink, &self.counters, ev);
            }
        }
    }

    #[test]
    fn runtime_spawns_and_drains() {
        let src1: Box<dyn Source> = Box::new(CountingSource {
            counters: SourceCounters::new(),
            emit: 5,
        });
        let src2: Box<dyn Source> = Box::new(CountingSource {
            counters: SourceCounters::new(),
            emit: 3,
        });

        let (rt, mut consumers) = IngressRuntime::launch(vec![src1, src2], 16, None);

        let mut totals = [0u32; 2];
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline && (totals[0] < 5 || totals[1] < 3) {
            for (i, c) in consumers.iter_mut().enumerate() {
                while let Some(_ev) = c.pop() {
                    totals[i] += 1;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        rt.join();
        assert_eq!(totals[0], 5);
        assert_eq!(totals[1], 3);
    }
}
