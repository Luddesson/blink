# R-3 — Legal sign-off stub: mempool-derived RN1 intent

**Todo**: `r3-legal`. **Owner**: counsel. **Blocker for**: `p2-ingress` mempool source
path going to production (can be implemented + shadow-tested without counsel).

## The question

We currently copy-trade RN1 by polling the Polymarket activity API *after* their order
is matched on-chain. Phase 2 proposes reading the **unconfirmed transaction** from the
Polygon mempool and executing our copy trade *before* RN1's tx is included in a block.

Three legal questions:

1. Is this still "copy trading a public wallet" or does it cross into
   "front-running" / "trading ahead" under applicable law? Jurisdiction depends on
   where RN1 is domiciled (unknown) and where we are domiciled (operator).
2. Does this violate Polymarket's ToS (Terms of Service)? The CTF itself does not
   prohibit it, but §10 of Polymarket's ToS contains an anti-abuse clause whose
   interpretation is ambiguous.
3. Tax / reporting treatment: front-running via mempool observation may be
   characterised differently from post-settlement copy trading.

## What's needed

- Counsel review of the specific implementation (not just the concept).
- Written determination: permitted / permitted with caveats / prohibited.
- If permitted with caveats, encode those caveats as feature flags
  (e.g. `BLINK_MEMPOOL_MIN_LEAD_MS`, `BLINK_MEMPOOL_SIZE_CAP`).

## Technical gating

`p2-ingress` can ship `MempoolSource` in **shadow mode only** (`observe, never submit`)
without counsel sign-off — that's code-level observation of public mempool data, which
is neither regulated nor a ToS violation. Going from shadow → live submission **requires
sign-off**.

Encode this as: `BLINK_MEMPOOL_SUBMIT=false` default. Only flip true after sign-off
documented + stored in `docs/rebuild/R3_LEGAL_SIGNOFF.md`.
