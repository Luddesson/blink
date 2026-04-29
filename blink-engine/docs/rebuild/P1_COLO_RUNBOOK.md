# P1-Colo Runbook — Collocate service in CLOB region

**Todo**: `p1-colo`. **Status**: requires human decision + cloud account access.
This is the single biggest latency win in the plan. The service must run in the same
AWS region that Cloudflare routes `clob.polymarket.com` to.

## 1. Determine target region

```bash
# On a machine with geographically diverse egress:
for host in clob.polymarket.com ws-subscriptions-clob.polymarket.com; do
  echo "=== $host ==="
  dig +short "$host"
  for region in us-east-1 us-east-2 us-west-2 eu-west-1; do
    echo "-- from $region test box --"
    # From EC2 instance in that region:
    # curl -o /dev/null -s -w '%{time_connect} %{time_starttransfer}\n' \
    #   "https://$host/health"
  done
done
```

Cloudflare anycast returns the origin nearest the **client**. Run `blink-probe cloudflare
--region <r>` from a throwaway EC2 box in each candidate region; pick the one with
lowest P99 `time_starttransfer`. Expect `us-east-1` or `us-east-2`.

## 2. Provision

- Instance: `c7i.2xlarge` minimum (8 vCPU, AVX-512, constant_tsc, enough for isolcpus 2–7).
- Placement: default VPC subnet in chosen AZ. Enable **enhanced networking** (ENA).
- Kernel: Ubuntu 24.04 LTS or Debian 13 (kernel ≥ 6.6 for io_uring SQPOLL + fixed bufs).
- Disk: 200 GB gp3 for binaries/logs; separate io2 volume for the Polygon node (see
  `P1_NODE_RUNBOOK.md`).
- Security group: egress 443 to `clob.polymarket.com`, 443/80 to Polygon peers, ingress
  only from the operator bastion.

## 3. Validate

Exit criterion (from plan §3 Phase 1): submit RTT median ≤ 3 ms over 1 h of live traffic.
Run `blink-probe cloudflare --region <r>` from the new box for 1 h; require
`p50_connect_ms ≤ 2 && p99_body_ms ≤ 8`. If P99 > 8 ms consistently, re-measure from the
other candidate region before accepting the region choice.

## 4. Rollout

Behind `BLINK_REGION=<r>` env flag. Old box keeps running; new box joins shadow mode
first, then 1 %→10 %→50 %→100 % per plan §6.

## Risk linkage

- R-7 Cloudflare: the P99 8 ms target may not be achievable — use probe data to decide.
- R-2 Ratelimit: measure from the new region; limits are per-key not per-IP but verify.
