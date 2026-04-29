# blink-ops-probes

Operator measurement probes for the Polymarket HFT rebuild (plan §5,
risks R-2, R-7, R-8). These are **not** part of the trading engine —
they're the tools the operator runs by hand against live infrastructure
to collect the numbers that unblock design decisions.

Single binary, `blink-probe`, with three subcommands. Each emits a JSON
report to stdout (logs go to stderr).

> **Safety**: these probes hit real endpoints. Never run them from CI
> and never run the `ratelimit` probe without a throwaway API key and
> the explicit `--i-understand-this-hits-live-polymarket` flag.

Build:

```sh
cargo build -p blink-ops-probes --release
./target/release/blink-probe --help
```

---

## `blink-probe ratelimit` — R-2

Measures Polymarket CLOB `POST /order` rate-limit behavior. Sends a
valid-shaped payload with a deliberately invalid signature so the
request is rejected *after* the rate-limit layer — we observe the gate,
not the app.

```sh
export POLYMARKET_API_KEY=<throwaway-or-revoked-key>
blink-probe ratelimit \
  --rps 10 --duration-secs 30 \
  --i-understand-this-hits-live-polymarket \
  > r2.json
```

Report fields:

| field                     | meaning                                       |
| ------------------------- | --------------------------------------------- |
| `rps_target` / `rps_achieved` | scheduled vs actual send rate             |
| `status_counts`           | HTTP status → count                           |
| `first_429_at_request`    | index of first `429` (if any)                 |
| `retry_after_observed`    | unique `Retry-After` values                   |
| `ratelimit_headers_seen`  | last-seen `x-ratelimit-*` header values       |
| `response_time.p{50,90,99}_ms` | server-response latency distribution     |

Interpreting: if `first_429_at_request` is well below `total_requests`,
the CLOB is rate-limiting us at roughly `(first_429_at_request / duration)`
RPS. Cross-check with `x-ratelimit-limit` / `-remaining` headers.

---

## `blink-probe cloudflare` — R-7

Measures the Cloudflare edge path for `clob.polymarket.com` from the
current host: DNS A records, 1000 HTTP/2 GETs on one persistent
connection, Cloudflare headers.

```sh
blink-probe cloudflare --region iad --iters 1000 > r7.json
```

**Endpoint choice**: `GET /` is used because we're measuring the
*network* — TLS handshake avoidance (persistent conn), HTTP/2 framing,
and Cloudflare edge RTT. Any response status is acceptable. We discard
the body after measuring body-complete time.

Report fields:

| field                         | meaning                                          |
| ----------------------------- | ------------------------------------------------ |
| `dns_a_records`               | all A records resolved for the host              |
| `body_complete.p{50,90,99}_ms`| per-request body-complete latency                |
| `cf_ray_samples`              | first few `cf-ray` values (format `<hex>-<POP>`) |
| `inferred_anycast_pops`       | unique 3-letter POP suffixes (e.g. `IAD`, `EWR`) |
| `cf_cache_status_samples`     | e.g. `DYNAMIC`, `HIT`                            |
| `server_header_samples`       | usually `cloudflare`                             |

Interpreting: P99 body-complete over 1000 reqs on a warm H/2 connection
is our best proxy for submit-path tail latency before we add signing.
If the POP suffix rotates across requests that's normal anycast noise.

---

## `blink-probe mempool` — R-8

Connects to a Polygon JSON-RPC WebSocket, subscribes to
`newPendingTransactions`, and measures pending→inclusion delay. If
`POLYGON_WS_URL_2` is set, a second feed runs in parallel and coverage
overlap is reported.

```sh
export POLYGON_WS_URL=wss://primary.example
export POLYGON_WS_URL_2=wss://secondary.example   # optional
blink-probe mempool \
  --duration-secs 300 \
  --ctf 0x4D97DCd97eC945f40cF65F87097ACe5EA0476045 \
  > r8.json
```

Report fields:

| field                       | meaning                                            |
| --------------------------- | -------------------------------------------------- |
| `total_pending`             | pending hashes observed on primary+secondary       |
| `pending_per_sec`           | firehose rate                                      |
| `ctf_match_count`           | pending txs whose `to` matched `--ctf`             |
| `inclusion.p{50,99}_ms`     | pending→mined delay (successful inclusions only)   |
| `coverage`                  | only when secondary feed set: `only_primary`, `only_secondary`, `in_both` within the sliding overlap window |

Interpreting: if `coverage.only_primary` + `only_secondary` is large
relative to `in_both`, a single RPC provider is not enough — we need
dual connectivity to avoid missing fills. Inclusion P99 sets the
lower bound on how long the engine should wait before assuming a
submitted tx was dropped.

---

## Tests

Only the offline invariants (help rendering, JSON report round-trip,
env-var guard) are covered by `cargo test`. The probes themselves must
be exercised by hand against real endpoints.

```sh
cargo test -p blink-ops-probes
```
