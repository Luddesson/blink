//! `blink-shadow` — Phase 0 parity-replay harness.
//!
//! This crate runs **two** implementations of the decision kernel (a
//! "legacy" and a "v2") against identical, deterministic
//! [`DecisionInput`](input::DecisionInput) records and records every
//! divergence to a journal. The MVP is **replay-only**, **single-threaded**,
//! **in-memory**; no tokio, no ClickHouse, no live tap.
//!
//! # Non-goals (see sibling todos)
//! - `p0-shadow-hook`: extract the real legacy `decide()` and the v2 kernel
//!   into impls of [`kernel::DecisionKernel`]. Today we only ship stubs.
//! - `p0-shadow-capture`: capture richer book state than best bid/ask and
//!   stream `DecisionInput` from the live engine.
//! - `p0-shadow-live`: tap the live WS feed while the engine runs in
//!   production so capture and replay use the same source of truth.
//!
//! The [`bin/shadow-selftest`](../../shadow-selftest) binary proves the
//! harness itself is honest: it forces a synthetic divergence and asserts
//! the runner trips, and it also asserts that two identical kernels
//! produce zero divergences.

#![deny(missing_docs)]

pub mod divergence;
pub mod fingerprint;
pub mod input;
pub mod journal;
pub mod kernel;
pub mod live;
pub mod runner;

pub use divergence::{DivergenceField, DivergenceRecord};
pub use fingerprint::{classify_noop, fingerprint, NoOpCode, OutcomeFingerprintV1};
pub use input::{BookSnapshot, DecisionInput, ResolvedMetadata};
pub use journal::{MemoryJournal, ShadowJournal};
pub use kernel::{DecisionKernel, KernelState, Position, StubKernel};
pub use live::{CapturedRow, LiveShadowRunner, ShadowCounters};
pub use runner::{Counters, ShadowReport, ShadowRunner};
