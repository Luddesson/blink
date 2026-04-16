---
mode: agent
description: Scaffold a new Rust module in the Blink engine crate following all project conventions.
---

Create a new module named `$MODULE_NAME` in `blink-engine/crates/engine/src/`.

**Steps:**
1. Create `crates/engine/src/$MODULE_NAME.rs`
2. Add `pub mod $MODULE_NAME;` to `lib.rs` in alphabetical order within its group
3. Start the file with a `//!` doc comment: one-line purpose + hot/cold classification
4. If the module has tunable params, add:
   ```rust
   pub struct $ModuleNameConfig { ... }
   impl $ModuleNameConfig {
       pub fn from_env() -> Self { /* unwrap_or(default) for every field */ }
   }
   ```
5. If cold-path, add: `//! **Latency class: COLD PATH. Never call from the signal → order hot path.**`
6. Add at least one unit test at the bottom of the file with `#[cfg(test)]`
7. If any new env vars are added, document them in `blink-engine/README.md`

**Never:**
- Add version numbers in `crates/engine/Cargo.toml` — use `{ workspace = true }` only
- Use `println!` — use `tracing::info!` with structured fields
- Use floats in anything that touches order prices or sizes in the hot path
- Hold a `tokio::sync::Mutex` lock across an `.await` point without justification

Run `cargo clippy --workspace -- -D warnings` and `cargo test -p engine` before finishing.
