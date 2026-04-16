---
applyTo:
  - blink-engine/crates/**/*.rs
description: Rust engine conventions for the Blink trading engine crate.
---

## Price and size representation
- All prices and sizes in the hot path are `u64 × 1000`. Never use `f64` in `order_book`, `sniffer`, `ws_client`, or any WebSocket event path.
- Use `parse_price()` / `format_price()` from `types.rs`. Never parse prices manually.
- `f64` is only acceptable in `PaperPortfolio`, `PaperPosition`, `ClosedTrade` (accounting paths).

## Error handling
- Use `anyhow::Result` for application-level errors, `thiserror` for library-level typed errors.
- Never use `.unwrap()` in non-test code except on infallible operations (e.g., `Mutex::lock()`).
- Propagate with `?`. Log before returning with `tracing::error!(err = ?e, ...)`.

## Async and concurrency
- Hold `tokio::sync::Mutex` across `.await`. Use `std::sync::Mutex` for short sync-only critical sections.
- Hot-path signals use `crossbeam_channel` (lock-free bounded MPSC). Never block the channel.
- `Arc<AtomicU64>` / `AtomicBool` for counters and flags readable from multiple tasks without locking.
- Never call `run_autoclaim()` from signal handling or TUI — portfolio lock starvation.
- Never call `BullpenBridge` from the signal → order path — 500ms+ latency.

## Module structure
- Every module starts with a `//!` doc comment: purpose + hot/cold classification.
- Cold-path modules include: `//! **Latency class: COLD PATH. Never call from the signal → order hot path.**`
- Config structs use `from_env()` with `unwrap_or(safe_default)` — never panic on missing vars.
- `main.rs` only wires modules. All business logic lives in individual modules.

## Testing
- Test both the happy path and the rejection/error path for every new gate or filter.
- Property-based tests use `proptest`. Pure functions (like `evaluate_exits`) must have proptest coverage.
- Never add side effects to `exit_strategy::evaluate_exits()` — it must remain a pure function.

## Logging
- Always use `tracing` macros with structured fields: `tracing::info!(token_id = %id, side = ?side, "msg")`.
- Never use `println!` or `eprintln!` in library code.

## Dependencies
- Never add version numbers in a crate's `Cargo.toml`. Use `{ workspace = true }` only.
