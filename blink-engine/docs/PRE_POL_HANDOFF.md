# Blink Pre-POL Handoff

Current posture:

- Engine runs in live observer mode with `LIVE_TRADING=true`.
- Actual order submission is blocked by `TRADING_ENABLED=false`.
- Current live bankroll source is on-chain pUSD, not paper default cash.
- Last verified pUSD balance: `4.506107`.
- Signer POL is present; proxy/funder POL is still `0`, which is acceptable for the signature type 2 gasless CLOB path but not for direct proxy-funded on-chain transactions.
- Exchange submit path reached Polymarket, but the current environment received a region restriction response.
- A geoblock guard now checks `https://polymarket.com/api/geoblock` during startup and blocks any attempt to set `TRADING_ENABLED=true` while the location is restricted or unverified.

Why the kill switch stays off:

- The engine proved it can sign and reach `POST /order`.
- Polymarket rejected live submission from this environment with a region restriction.
- No further live order attempts should be made until the operator confirms compliant access.
- Do not try to bypass geoblocking. The engine should only be enabled from an eligible, compliant environment.

Fixed before this handoff:

- Live UI/API risk handle now points to the live risk manager.
- Live UI/API portfolio now reflects the live engine portfolio.
- Live portfolio cash is seeded from on-chain pUSD preflight.
- CLOB REST orderbook seed uses `/book?token_id=...`.
- Vault signing uses async `VaultHandle::sign_digest` and no longer panics inside Tokio.
- Live accounting no longer records a fill merely because an intent was queued.
- History UI defaults to 24h and caps fetch size.
- Paper runtime state was archived before live prep.
- Preflight and `/api/config` now enforce the geoblock guard before live trading can be enabled.

Run this before adding POL:

```bash
cd /root/blink_src/blink-engine
scripts/pre_pol_check.sh
```

Expected before POL:

- Service active.
- `risk_status` is `KILL_SWITCH_OFF`.
- `trading_enabled` is `false`.
- `nav_usdc` matches pUSD-backed cash.
- `submits_started` remains zero after the current restart.
- `polymarket geoblock.launch_status` is either `BLOCKED_KEEP_KILL_SWITCH_OFF` or `ELIGIBLE`; only `ELIGIBLE` can proceed to live.

After POL is added:

1. Re-run `scripts/pre_pol_check.sh`.
2. Re-run live preflight from `/opt/blink` with the deployed environment.
3. Confirm signer POL is non-zero and pUSD/allowance remain visible.
4. Resolve the Polymarket region restriction through compliant eligibility before enabling `TRADING_ENABLED=true`.
5. Keep canary sizing at `$1` until real exchange accepts and reconciliation confirms fills.
