//! # blink-chaos
//!
//! Chaos / fault-injection harnesses for the Blink HFT rebuild (Phase 7,
//! §3 of the plan). This crate is **test-only infrastructure**: it is
//! never linked into the production hot path. The public surface splits
//! into two parts:
//!
//! 1. [`FaultInjector`] — an in-process registry that allows a test to
//!    activate / clear a named [`Fault`]. Production code never touches
//!    this crate, so the injector's role is to drive the *mock* side of
//!    the scenario (the fake CLOB, the fake Polygon WS) rather than to
//!    perturb real code paths from within.
//! 2. [`mock`] — concrete misbehaving servers the integration tests
//!    stand up: [`mock::clob::MockClobServer`] and
//!    [`mock::polygon_ws::MockPolygonWs`].
//!
//! ## Scenarios implemented
//!
//! Each scenario lives in `tests/scenario_<name>.rs`. Run the whole
//! suite with:
//!
//! ```text
//! cargo test -p blink-chaos
//! ```
//!
//! See the crate README / task spec for the full list and their
//! ignore-status rationale. Scenarios whose victim crate has not landed
//! yet (`blink-breakers`, clock-skew shim in `blink-timestamps`) are
//! annotated `#[ignore = "..."]` so the suite stays green today while
//! making the dependency explicit to the future implementer.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

pub mod mock;

use std::sync::Arc;

use parking_lot::Mutex;

// ─── Fault taxonomy ───────────────────────────────────────────────────────

/// A deterministic, test-only failure mode that a mock server (or other
/// harness) should present the next time a matching interaction occurs.
///
/// Variants are additive — do not renumber or reorder without updating
/// every scenario that matches on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// After `after_messages` frames have been pushed to a WS client,
    /// close the socket and refuse reconnects for `reconnect_stays_dead_ms`.
    WsDrop {
        /// How many frames to allow before dropping.
        after_messages: u32,
        /// Duration (ms) to reject inbound TCP after the drop.
        reconnect_stays_dead_ms: u32,
    },
    /// Stall `percentile` % of RPC responses by `delay_ms` — used to
    /// simulate a slow upstream node.
    RpcStall {
        /// Percentage (0..=100) of calls to inject on.
        percentile: u8,
        /// How long to stall the affected call.
        delay_ms: u32,
    },
    /// Reply with HTTP `status` for the next `streak` requests.
    Http5xx {
        /// Status code to serve (e.g. 500, 429, 503).
        status: u16,
        /// How many consecutive requests to fail before returning to
        /// normal behaviour.
        streak: u32,
    },
    /// One-shot timestamp jump announced to the (future) test-clock
    /// shim in `blink-timestamps`. Scenario 4 today is
    /// `#[ignore]` — see `tests/scenario_clock_skew_jump.rs`.
    ClockSkew {
        /// Positive = forward jump; negative = backward jump, in ns.
        jump_ns: i64,
        /// Logical time (ns) at which the jump should land.
        at_logical_ns: u64,
    },
    /// After `after_requests` requests, hard-reset the TCP connection
    /// mid-response.
    ConnectionReset {
        /// Count of accepted requests before the reset.
        after_requests: u32,
    },
    /// Refuse all inbound TCP until the given logical time.
    NetworkPartition {
        /// Logical-ns deadline after which connects are accepted again.
        until_ns: u64,
    },
}

/// Shared, name-keyed fault registry.
///
/// Mock servers consult [`Self::should_fail`] on each interaction; a
/// scenario drives the servers by calling [`Self::inject`] before the
/// stimulus it wants to perturb and [`Self::clear`] to rewind.
///
/// Every variant of [`Fault`] is **consumed** after one successful
/// `should_fail` observation — i.e. the injector erases the fault once
/// it has been observed. Scenarios that want a streak of N failures
/// model that inside the [`Fault`] variant itself (`streak`,
/// `after_messages`, `after_requests`) rather than by re-injecting.
#[derive(Default, Clone)]
pub struct FaultInjector {
    inner: Arc<Mutex<FaultMap>>,
}

#[derive(Default)]
struct FaultMap {
    faults: std::collections::HashMap<&'static str, Fault>,
}

impl FaultInjector {
    /// Fresh, empty injector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Activate a named fault. Overwrites any prior fault under the
    /// same name.
    pub fn inject(&self, name: &'static str, fault: Fault) {
        self.inner.lock().faults.insert(name, fault);
    }

    /// Drop the named fault (no-op if not present).
    pub fn clear(&self, name: &'static str) {
        self.inner.lock().faults.remove(name);
    }

    /// Observe — returns `Some(fault)` exactly once; subsequent calls
    /// return `None` until `inject` is called again.
    pub fn should_fail(&self, name: &'static str) -> Option<Fault> {
        self.inner.lock().faults.remove(name)
    }

    /// Peek without consuming. Handy for mocks that need to stay in
    /// "bad" mode across many requests (e.g. an `Http5xx { streak: 20 }`
    /// wants to be observed 20 times).
    pub fn peek(&self, name: &'static str) -> Option<Fault> {
        self.inner.lock().faults.get(name).cloned()
    }

    /// Decrement the internal counter of a streak-style fault, removing
    /// it once the streak hits zero. Returns the fault that was
    /// observed (before decrement) so the caller can act on it.
    ///
    /// Currently only meaningful for [`Fault::Http5xx`]
    /// (`streak`), [`Fault::WsDrop`] (`after_messages`), and
    /// [`Fault::ConnectionReset`] (`after_requests`).
    pub fn observe_streak(&self, name: &'static str) -> Option<Fault> {
        let mut g = self.inner.lock();
        let f = g.faults.get_mut(name)?.clone();
        match g.faults.get_mut(name)? {
            Fault::Http5xx { streak, .. } => {
                if *streak <= 1 {
                    g.faults.remove(name);
                } else {
                    *streak -= 1;
                }
            }
            Fault::ConnectionReset { after_requests } => {
                if *after_requests == 0 {
                    g.faults.remove(name);
                } else {
                    *after_requests -= 1;
                }
            }
            Fault::WsDrop { after_messages, .. } => {
                if *after_messages == 0 {
                    g.faults.remove(name);
                } else {
                    *after_messages -= 1;
                }
            }
            _ => {
                g.faults.remove(name);
            }
        }
        Some(f)
    }
}

impl std::fmt::Debug for FaultInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = self.inner.lock();
        f.debug_struct("FaultInjector")
            .field("active", &g.faults.keys().collect::<Vec<_>>())
            .finish()
    }
}

// ─── Unit tests for the injector itself ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_and_consume() {
        let fi = FaultInjector::new();
        assert!(fi.should_fail("x").is_none());
        fi.inject("x", Fault::Http5xx { status: 500, streak: 1 });
        assert!(matches!(
            fi.should_fail("x"),
            Some(Fault::Http5xx { status: 500, streak: 1 })
        ));
        assert!(fi.should_fail("x").is_none());
    }

    #[test]
    fn peek_does_not_consume() {
        let fi = FaultInjector::new();
        fi.inject("y", Fault::RpcStall { percentile: 5, delay_ms: 500 });
        assert!(fi.peek("y").is_some());
        assert!(fi.peek("y").is_some(), "peek is non-destructive");
        fi.clear("y");
        assert!(fi.peek("y").is_none());
    }

    #[test]
    fn observe_streak_counts_down() {
        let fi = FaultInjector::new();
        fi.inject("z", Fault::Http5xx { status: 500, streak: 3 });
        for i in 0..3 {
            let f = fi.observe_streak("z").unwrap_or_else(|| panic!("iter {i}"));
            assert!(matches!(f, Fault::Http5xx { status: 500, .. }));
        }
        assert!(fi.peek("z").is_none(), "streak should be exhausted");
    }

    #[test]
    fn clear_is_idempotent() {
        let fi = FaultInjector::new();
        fi.clear("missing");
        fi.inject("a", Fault::NetworkPartition { until_ns: 0 });
        fi.clear("a");
        fi.clear("a");
        assert!(fi.peek("a").is_none());
    }

    #[test]
    fn debug_lists_active_names() {
        let fi = FaultInjector::new();
        fi.inject("one", Fault::NetworkPartition { until_ns: 0 });
        let s = format!("{:?}", fi);
        assert!(s.contains("one"), "debug should name active faults: {s}");
    }
}
