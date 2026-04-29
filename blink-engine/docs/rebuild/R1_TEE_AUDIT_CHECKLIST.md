# R-1 — TEE attestation audit checklist

**Todo**: `r1-teeaudit`. **Owner**: security eng + TEE vault owner. **Blocker for**: `p4-teelease`.

The plan §3 Phase 4 proposes moving from "TEE signs every order" to "TEE issues
short-lived per-market subkeys; hot path signs with subkey". The audit trail must still
prove "TEE gated this submission", just transitively.

## Questions to resolve before implementing `p4-teelease`

1. **Today's invariant**: does the TEE currently sign every order, or only issue a master
   subkey at startup? (Read `crates/tee-vault/` + `crates/engine/src/order_signer.rs`.)
2. **Attestation freshness**: how long is a TEE attestation quote considered valid?
   Subkey TTL must be ≤ that freshness window.
3. **Revocation path**: if a subkey leaks, can we revoke on-chain before the next batch
   of fills settles? Polymarket's CTF does not natively support per-key revocation lists
   — so "revocation" means withdrawing the operator key's approval. Document the MTTR.
4. **Audit log**: where is "TEE authorised subkey X for market Y at time T with quote Q"
   persisted? Must survive a node reboot and be queryable by auditors.
5. **Subkey scope**: per-market? per-(market, side)? per-(market, day)? Narrower scope
   reduces blast radius but increases TEE roundtrip frequency. Propose: per-market,
   24 h TTL, automatic rotation 1 h before expiry.

## Acceptance criteria

- Threat model doc covering: hot-path private-key exposure, compromised subkey,
  compromised TEE (attestation spoofing), replay of revoked subkeys.
- Written sign-off from the TEE vault owner on the proposed subkey TTL + scope.
- Audit-log query runbook: "given an on-chain fill at time T, show the subkey that
  signed it + the TEE attestation that issued it".

**Until this is signed off, `p4-teelease` is BLOCKED.**
