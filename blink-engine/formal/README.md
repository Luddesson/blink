# Formal Verification — Blink Engine

Symbolic proofs for the critical financial invariants in the Blink HFT engine.

## Tools

| Tool | Purpose |
|------|---------|
| **proptest** (Rust) | Property-based fuzzing (10,000+ iterations) of EIP-712 signing and risk manager |
| **Halmos** (Solidity) | Symbolic execution — proves properties hold for ALL possible inputs |
| **Kani** (Rust) | Bounded model checking via CBMC for integer overflow verification |

## Running

### proptest (Rust)

```bash
cargo test --workspace -- proptest
```

All proptest tests run 10,000 iterations by default. Key properties verified:
- `eip712_digest_is_deterministic` — same inputs always produce the same digest
- `eip712_signature_is_recoverable` — ecrecover(sign(digest, key)) == address(key)
- `daily_loss_never_exceeds_limit` — circuit breaker enforces daily loss cap
- `order_size_never_exceeds_max` — order size cap is always enforced
- `positions_never_exceed_max` — concurrent position cap is always enforced

### Halmos (Solidity)

```bash
pip install halmos
cd formal/
make verify
```

Properties proven symbolically:
- `check_ecrecover_returns_signer` — signature recovery correctness
- `check_domain_separator_deterministic` — EIP-712 domain separator is constant
- `check_different_salts_different_hashes` — collision resistance of order hashes
- `check_daily_loss_bounded` — daily loss limit is enforced
- `check_order_size_cap` — order size cap always holds
- `check_position_cap` — concurrent position limit always holds
- `check_no_overflow_in_pnl` — P&L tracking has no integer overflow

### Kani (Rust)

Kani verification of `compute_amounts()` in `order_signer.rs` for absence of
integer overflow is specified via `#[kani::proof]` harnesses. To run:

```bash
cargo kani --harness verify_compute_amounts_no_overflow
```

> Note: Kani requires Linux and the Kani toolchain (`cargo install --locked kani-verifier && cargo kani setup`).
