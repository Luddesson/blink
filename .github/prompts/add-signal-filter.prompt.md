---
mode: agent
description: Scaffold a new signal pre-filter in the Blink engine pipeline correctly, following all existing conventions.
---

I want to add a new signal pre-filter to `handle_signal()` in `blink-engine/crates/engine/src/paper_engine.rs`.

**Filter spec:** $FILTER_DESCRIPTION

Before implementing, confirm:
1. Where in the 9-filter pipeline does this belong? (earlier = cheaper, later = more context available)
2. Does it need new config fields in `Config::from_env()`? If so, add them with safe defaults.
3. What is the rejection reason string to pass to `record_rejection(reason)`?

**Implementation checklist:**
- [ ] Add the gate at the correct position in `handle_signal()` — early-return pattern, not nested if
- [ ] Call `record_rejection("descriptive_snake_case_reason")` before returning
- [ ] Add the env var to `Config` with `from_env()` + `unwrap_or(DEFAULT)` pattern
- [ ] Add a unit test in the same file: test both the pass case and the reject case
- [ ] Add the new env var to `blink-engine/README.md` config table
- [ ] Verify `cargo clippy --workspace -- -D warnings` passes

Do not use floats in the filter logic. Keep the filter stateless if possible.
