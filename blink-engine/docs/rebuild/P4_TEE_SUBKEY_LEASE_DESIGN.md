# P4-TEELease — TEE subkey leasing design

**Todo**: `p4-teelease`. **Blocked by**: `r1-teeaudit` sign-off.

## Today

Every order submission is signed inside the TEE. Hot path makes an IPC/HTTP call to the
TEE for each signature. P99 signer latency is ~3 ms — dominant in the current budget.

## Proposal

TEE issues **short-lived per-market subkeys** (EIP-712-signed delegation: the master key
authorises a fresh secp256k1 key to sign orders for market `M` with quantity cap `Q`
until time `T`). Hot path signs with the subkey in user space (latency ~150 µs with
`blink-signer`). Audit trail: every subkey issuance is logged with a fresh TEE
attestation quote.

```
                 ┌─────────────────┐
                 │  Blink Operator │
                 └────────┬────────┘
                          │ SubkeyLeaseRequest{market, cap, ttl}
                          ▼
                 ┌─────────────────┐
                 │  TEE Vault      │  Attests: "master authorised subkey K for M until T"
                 │                 │  Emits signed delegation + quote
                 └────────┬────────┘
                          │ SubkeyLease{ subkey_pk, sig_by_master, attestation_quote }
                          ▼
                 ┌─────────────────┐
                 │  SignerPool     │  Uses subkey_sk for orders in M, stops at T
                 └────────┬────────┘
                          │ Signed order
                          ▼
                      CLOB POST
```

## Schema (frozen once implemented)

```rust
pub struct SubkeyLease {
    pub market_id: MarketId,
    pub subkey_pk: [u8; 33],          // compressed secp256k1
    pub cap_notional_usdc_u64: u64,
    pub issued_at_ns: u64,
    pub expires_at_ns: u64,
    pub lease_id: [u8; 16],
    pub master_sig: [u8; 65],         // EIP-712 delegation sig
    pub tee_attestation: Vec<u8>,    // opaque quote blob
}
```

Journal row adds `subkey_lease_id: [u8;16]` so any fill can be traced back to the
specific lease + attestation.

## Rotation policy

- Lease TTL: **24 h** (matches typical TEE attestation freshness).
- Rotation trigger: 1 h before expiry, async.
- Hard kill: if lease not refreshed by `expires_at_ns - 5m`, submitter refuses new orders
  on that market until new lease arrives. Paging alert fires.

## Emergency revoke

Master publishes a revocation list (on-chain or via a signed HTTP endpoint); submitter
checks on every signature that subkey is not in the list. Adds ~50 ns (single hash-set
lookup). Accept.

## Open questions (resolve in r1-teeaudit)

See `R1_TEE_AUDIT_CHECKLIST.md`. This design is a **proposal**; implementation is gated
on security owner sign-off.

## Implementation plan (post-signoff)

1. `tee-vault` crate: add `lease_subkey(market, cap, ttl)` method.
2. `blink-signer`: add `LeasedSignerPool` that refuses to sign once expires_at passed.
3. `blink-submit`: consumes `LeasedSignerPool` instead of master `SignerPool`.
4. Shadow mode first: both paths run, fills match.
5. Graduate per plan §6.
