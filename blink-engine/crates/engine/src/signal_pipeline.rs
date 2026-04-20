//! Signal pipeline helpers.
//!
//! The main bounded per-token dispatch pipeline is constructed inline in
//! `main.rs` (it owns the engine-specific handlers). This module exposes
//! shared helpers used by that pipeline.
//!
//! # Environment variables
//! - `BLINK_SIGNAL_WORKERS` — max concurrent token workers
//!   (default: clamp(available_parallelism, 4, 16))
//! - `BLINK_SIGNAL_PER_TOKEN_QUEUE` — per-token channel depth (default 64)

/// Returns the configured or auto-detected worker count for the signal pipeline.
pub fn default_worker_count() -> usize {
    let auto = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(4, 16);
    std::env::var("BLINK_SIGNAL_WORKERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(auto)
}

/// Returns the configured per-token queue depth. The default is sourced from
/// the active [`crate::execution_profile::ExecutionProfile`]
/// (`max_concurrent_per_token`); `BLINK_SIGNAL_PER_TOKEN_QUEUE` overrides.
pub fn per_token_queue_depth() -> usize {
    per_token_queue_depth_for_profile(
        crate::execution_profile::ExecutionProfile::from_env(),
    )
}

/// Resolve per-token queue depth for a specific execution profile, honouring
/// the `BLINK_SIGNAL_PER_TOKEN_QUEUE` env override when present.
pub fn per_token_queue_depth_for_profile(
    profile: crate::execution_profile::ExecutionProfile,
) -> usize {
    let default = profile.knobs().max_concurrent_per_token;
    std::env::var("BLINK_SIGNAL_PER_TOKEN_QUEUE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
        .max(1)
}
