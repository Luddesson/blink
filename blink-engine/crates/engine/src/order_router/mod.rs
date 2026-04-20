//! Async order router subsystem.
//!
//! # Module layout
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`intent`] | `OrderIntent` — immutable intent created at signal ingress |
//! | [`state`] | `PendingOrder` state machine (`Created → Acked → Filled …`) |
//! | [`router`] | `OrderRouter` — dispatcher + submit-worker pool |
//! | [`reconciler`] | Single-owner 250 ms reconcile sweep |

pub mod intent;
pub mod reconciler;
pub mod router;
pub mod state;

pub use intent::{OrderIntent, SignedOrderPayload};
pub use reconciler::spawn_reconciler;
pub use router::{OrderRouter, PendingOrderStore, RouterCounters, RouterFull};
pub use state::{OrderState, PendingOrder};
