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

/// Returns the configured per-token queue depth (default 64, minimum 1).
pub fn per_token_queue_depth() -> usize {
    std::env::var("BLINK_SIGNAL_PER_TOKEN_QUEUE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(64)
        .max(1)
}
