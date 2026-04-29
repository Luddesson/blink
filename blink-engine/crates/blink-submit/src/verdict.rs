//! Submit verdict types.

use blink_types::StageTimestamps;

/// Final disposition of a single submit attempt.
///
/// Callers drive recovery off `Unknown` — see
/// [`Submitter::probe_client_order_id`](super::submitter::Submitter::probe_client_order_id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitVerdict {
    /// Venue acknowledged the order (HTTP 200 + `success: true`).
    Accepted {
        /// CLOB-assigned order id echoed back in the response.
        venue_order_id: String,
        /// Client-side coid used for this submit.
        client_order_id: [u8; 16],
        /// All stage timestamps filled in through `tsc_ack`.
        stamps: StageTimestamps,
    },
    /// Venue rejected the order (`success: false` or 4xx).
    RejectedByVenue {
        /// HTTP status or a synthetic reason code (`0` = application-level).
        reason_code: u16,
        /// Human reason returned by the CLOB (`errorMsg`, or body slice).
        reason_text: String,
        client_order_id: [u8; 16],
    },
    /// The CLOB signalled that this coid was already seen (true idempotent
    /// replay). Treated as a benign outcome: the original submit was
    /// accepted.
    DuplicateDedup { client_order_id: [u8; 16] },
    /// We don't know whether the order reached the venue. Caller must
    /// invoke `probe_client_order_id` or the reconciliation worker
    /// before any retry.
    Unknown {
        client_order_id: [u8; 16],
        reason: UnknownReason,
    },
}

/// Classification of the ambiguous outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownReason {
    /// Stream-level or connection-level failure from
    /// [`blink_h2::H2Error`].
    H2Error(String),
    /// Request exceeded the submitter-configured deadline.
    Timeout,
    /// HTTP 2xx, but the body did not parse as an expected shape.
    Parse(String),
    /// HTTP 5xx / 429 — server may or may not have accepted.
    Transient { status: u16, body: String },
    /// Our own pre-flight failed (signer or local build error). Included
    /// here so the caller has a single channel for "did not make it out".
    PreflightError(String),
}
