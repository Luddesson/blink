#!/usr/bin/env bash
set -euo pipefail

base_url="${BLINK_BASE_URL:-http://127.0.0.1:3030}"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing dependency: $1" >&2
    exit 2
  }
}

need curl
need jq
need node
need systemctl

rpc_url="${POLYGON_RPC_URL:-https://polygon-bor-rpc.publicnode.com}"
funder_address="${POLYMARKET_FUNDER_ADDRESS:-$(grep -E '^POLYMARKET_FUNDER_ADDRESS=' /opt/blink/.env 2>/dev/null | tail -1 | cut -d= -f2-)}"
signer_address="${POLYMARKET_SIGNER_ADDRESS:-0x894E055148752F337949E483F673dAcE58B1A19a}"

native_balance_hex() {
  local address="$1"
  curl -fsS -X POST "$rpc_url" \
    -H 'Content-Type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getBalance\",\"params\":[\"$address\",\"latest\"]}" \
    | jq -r '.result // "0x0"'
}

format_wei() {
  node -e '
    const value = BigInt(process.argv[1]);
    const scale = 10n ** 18n;
    const whole = value / scale;
    const fraction = (value % scale).toString().padStart(18, "0").replace(/0+$/, "").slice(0, 6);
    console.log(fraction ? `${whole}.${fraction}` : `${whole}`);
  ' "$1"
}

echo "== blink pre-POL readiness =="
systemctl is-active blink-engine.service
systemctl show blink-engine.service \
  -p ActiveState -p SubState -p MainPID -p NRestarts -p ExecMainStatus \
  -p MemoryCurrent -p TasksCurrent --no-pager

mode_json="$(curl -fsS "$base_url/api/mode")"
status_json="$(curl -fsS "$base_url/api/status")"
risk_json="$(curl -fsS "$base_url/api/risk")"
portfolio_json="$(curl -fsS "$base_url/api/portfolio")"

echo
echo "mode:"
jq '{mode, live_active, paper_active, live_trading_env}' <<<"$mode_json"

echo
echo "risk:"
jq -n --argjson risk "$risk_json" --argjson status "$status_json" \
  '$risk + {risk_status: $status.risk_status}
   | {risk_status, trading_enabled, circuit_breaker_tripped, daily_pnl, max_daily_loss_pct, max_single_order_usdc, max_orders_per_second}'

echo
echo "portfolio:"
jq '{cash_usdc, nav_usdc, invested_usdc, total_signals, filled_orders, skipped_orders, aborted_orders, uptime_secs}' <<<"$portfolio_json"

if [[ -n "$funder_address" ]]; then
  funder_pol_hex="$(native_balance_hex "$funder_address")"
  signer_pol_hex="$(native_balance_hex "$signer_address")"
  echo
  echo "polygon gas:"
  jq -n \
    --arg rpc "$rpc_url" \
    --arg funder "$funder_address" \
    --arg funder_pol "$(format_wei "$funder_pol_hex")" \
    --arg signer "$signer_address" \
    --arg signer_pol "$(format_wei "$signer_pol_hex")" \
    '{rpc: $rpc, funder: $funder, funder_pol: $funder_pol, signer: $signer, signer_pol: $signer_pol}'
fi

echo
echo "polymarket geoblock:"
geoblock_json="$(curl -fsS "${POLYMARKET_GEOBLOCK_URL:-https://polymarket.com/api/geoblock}" 2>/tmp/blink-pre-pol-geoblock.err || true)"
if [[ -n "$geoblock_json" ]]; then
  jq '{blocked, country, region, launch_status: (if .blocked then "BLOCKED_KEEP_KILL_SWITCH_OFF" else "ELIGIBLE" end)}' <<<"$geoblock_json"
else
  jq -n --rawfile error /tmp/blink-pre-pol-geoblock.err \
    '{blocked: null, launch_status: "UNVERIFIED_KEEP_KILL_SWITCH_OFF", error: $error}'
fi

echo
echo "history 24h:"
curl -fsS -w 'TTFB:%{time_starttransfer} TOTAL:%{time_total} SIZE:%{size_download}\n' \
  "$base_url/api/history?page=1&per_page=500&range=24h" \
  -o /tmp/blink-pre-pol-history.json
jq '{source,total,total_pages,trades:(.trades|length)}' /tmp/blink-pre-pol-history.json

echo
echo "host:"
df -h /
free -h

echo
echo "expected before POL top-up:"
echo "- risk_status should be KILL_SWITCH_OFF"
echo "- trading_enabled should be false"
echo "- pUSD-backed nav should be visible"
echo "- submits_started should stay 0 after this restart while kill switch is off"
echo "- proxy accounts can be gasless for CLOB; signer/funder POL is only for direct on-chain txs"
echo "- geoblock launch_status must be ELIGIBLE before TRADING_ENABLED=true"
