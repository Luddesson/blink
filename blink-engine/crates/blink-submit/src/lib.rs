//! `blink-submit` — Polymarket CLOB submit hot-path.
//!
//! Responsibilities:
//! 1. EIP-712 encode a validated [`Intent`] into the wire body + digest
//!    (byte-for-byte compatible with `engine/src/order_signer.rs`).
//! 2. Sign the digest via a [`SignerPool`].
//! 3. POST the body over a persistent [`H2Client`] to `/order`.
//! 4. Parse the venue response and emit a [`SubmitVerdict`] with stamped
//!    [`StageTimestamps`].
//! 5. On ambiguous outcomes (timeout / stream drop) emit
//!    [`SubmitVerdict::Unknown`] — the caller is responsible for
//!    recovery (e.g. calling [`Submitter::probe_client_order_id`]).
//!
//! This crate does **not** retry internally. The retry / circuit-breaker
//! loop lives outside (future `p7-breakers`).
//!
//! ## Idempotent recovery
//!
//! Every submit carries a 16-byte `client_order_id` (coid) deterministically
//! derived from `(intent_hash, run_id, attempt)` — see
//! [`derive_client_order_id`]. Two submits of the same intent in the same
//! run produce the same coid; the CLOB dedups the duplicate. Bumping
//! `attempt` only happens when the operator explicitly elects a second
//! chance.
//!
//! [`Intent`]: blink_types::Intent
//! [`SignerPool`]: blink_signer::SignerPool
//! [`H2Client`]: blink_h2::H2Client
//! [`StageTimestamps`]: blink_types::StageTimestamps

#![forbid(unsafe_code)]

mod auth;
mod coid;
mod encoder;
mod stats;
mod submitter;
mod templates;
mod verdict;

pub use coid::{coid_from_hex, coid_to_hex, derive_client_order_id};
pub use encoder::{EncodedOrder, EncodeError, OrderEncoder, POLYMARKET_CTF_EXCHANGE};
pub use stats::{SubmitterStats, SubmitterStatsSnapshot};
pub use submitter::{PolyAuth, ProbeResult, Submitter, SubmitterConfig};
pub use templates::{
    compute_amounts_for_intent, MarketId, OrderTemplate, TemplateCache, TemplateCacheStats,
};
pub use verdict::{SubmitVerdict, UnknownReason};
