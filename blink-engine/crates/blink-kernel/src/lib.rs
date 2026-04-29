//! # blink-kernel
//!
//! V1 decision kernel for the Blink v2 pipeline.
//!
//! Ports the legacy `engine::paper_engine::handle_signal` gate logic into
//! a pure, allocation-free function that operates on a frozen
//! [`DecisionSnapshot`] and returns a [`KernelVerdict`].
//!
//! ## Layering
//!
//! ```text
//!   caller                                                 kernel
//!   ------                                                 ------
//!   build DecisionSnapshot (frozen)            ─────────▶  V1Kernel::decide
//!   tsc_decide = Timestamp::now()                          (no time reads,
//!                                                           no alloc,
//!                                                           no mut state)
//!   verdict = kernel.decide(&snap, &mut stats) ◀─────────
//!   tsc_decide_end = Timestamp::now()
//!
//!   // OUTSIDE the timed hot-path span:
//!   outcome = verdict_to_outcome(verdict, &run_id, attempt);
//!   recent_keys.insert(outcome.semantic_key);
//! ```
//!
//! ## Non-goals (v1)
//!
//! - No public `Arena`. Proved no-alloc via `stats_alloc` dev dep; if a
//!   scratch arena is needed later it is added privately.
//! - No float. All math is `i128` intermediate, clamped once.
//! - No timestamp reads inside the kernel. Caller stamps.
//! - No mutation of the snapshot. Dedup-ring insertion is the caller's
//!   responsibility after observing a `Submit` verdict.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod types;
pub mod config;
pub mod dedup;
pub mod stats;
pub mod snapshot;
pub mod kernel;
pub mod adapter;
pub mod v1;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use adapter::verdict_to_outcome;
pub use config::KernelConfig;
pub use dedup::RecentKeySet;
pub use kernel::{DecisionKernel, KernelVerdict};
pub use snapshot::DecisionSnapshot;
pub use stats::KernelStats;
pub use types::{IntentFields, NotionalUUsdc, PriceTicks, SemanticIntentKey, SharesU};
pub use v1::V1Kernel;
