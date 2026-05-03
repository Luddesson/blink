#!/usr/bin/env bash
set -euo pipefail

RUN_ID="${1:-$(date -u +%Y-%m-%dT%H%M%SZ)-hyper-status}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENGINE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ENGINE_DIR}/.." && pwd)"
OUT_DIR="${ENGINE_DIR}/logs/go-live/${RUN_ID}"

mkdir -p "${OUT_DIR}"

run_capture() {
  local name="$1"
  shift
  {
    printf '$'
    printf ' %q' "$@"
    printf '\n\n'
    "$@"
  } >"${OUT_DIR}/${name}.txt" 2>&1 || {
    local code=$?
    printf '\n[exit_code=%s]\n' "${code}" >>"${OUT_DIR}/${name}.txt"
    return 0
  }
}

curl_capture() {
  local name="$1"
  local url="$2"
  {
    printf '$ curl -sS %s\n\n' "${url}"
    curl -sS "${url}"
    printf '\n'
  } >"${OUT_DIR}/${name}.json" 2>&1 || {
    local code=$?
    printf '\n[exit_code=%s]\n' "${code}" >>"${OUT_DIR}/${name}.json"
    return 0
  }
}

{
  echo "# Go-Live Evidence Snapshot"
  echo
  echo "run_id: ${RUN_ID}"
  echo "captured_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "repo_root: ${REPO_ROOT}"
  echo "engine_dir: ${ENGINE_DIR}"
  echo
  echo "Scope: read-only live-canary snapshot. No env mutation, breaker reset, restart, or order endpoint is executed by this script."
  echo "Preflight: set BLINK_COLLECT_PREFLIGHT=1 to execute the read-only --preflight-live check and capture its output."
} >"${OUT_DIR}/README.md"

run_capture date_utc date -u
run_capture run_id printf '%s\n' "${RUN_ID}"
run_capture git_branch git -C "${REPO_ROOT}" branch --show-current
run_capture git_head git -C "${REPO_ROOT}" log -1 --oneline --decorate
run_capture git_status_short git -C "${REPO_ROOT}" status --short
run_capture git_diff_stat git -C "${REPO_ROOT}" diff --stat
run_capture git_staged_diff_stat git -C "${REPO_ROOT}" diff --cached --stat
run_capture git_tags_at_head git -C "${REPO_ROOT}" tag --points-at HEAD
run_capture risky_untracked_ignored git -C "${REPO_ROOT}" check-ignore -v \
  ai-agent/.env \
  blink-engine/.env.live \
  blink-engine/callgrind.out.202975 \
  blink-ui/build_log.txt \
  blink-ui/build_hard_done_1776868247

run_capture systemd_is_active systemctl is-active blink-engine
run_capture systemd_status systemctl status blink-engine --no-pager
run_capture systemd_unit systemctl cat blink-engine
run_capture systemd_journal_recent journalctl -u blink-engine --since "2 hours ago" --no-pager

if command -v rg >/dev/null 2>&1; then
  run_capture deployed_env_mode_fields rg -n "^(TRADING_ENABLED|LIVE_TRADING|PAPER_TRADING|POLYMARKET_SIGNATURE_TYPE|POLYMARKET_FUNDER_ADDRESS|MAX_SINGLE_ORDER_USDC|MAX_DAILY_LOSS_PCT|BLINK_ALLOW_NEG_RISK)=" /etc/blink-engine.env
else
  run_capture deployed_env_mode_fields grep -En "^(TRADING_ENABLED|LIVE_TRADING|PAPER_TRADING|POLYMARKET_SIGNATURE_TYPE|POLYMARKET_FUNDER_ADDRESS|MAX_SINGLE_ORDER_USDC|MAX_DAILY_LOSS_PCT|BLINK_ALLOW_NEG_RISK)=" /etc/blink-engine.env
fi

curl_capture api_status http://127.0.0.1:3030/api/status
curl_capture api_risk http://127.0.0.1:3030/api/risk
curl_capture api_failsafe http://127.0.0.1:3030/api/failsafe
curl_capture api_live_portfolio http://127.0.0.1:3030/api/live/portfolio
curl_capture api_live_executions http://127.0.0.1:3030/api/live/executions
curl_capture api_live_why_no_trade 'http://127.0.0.1:3030/api/live/why-no-trade?since_hours=24&limit=200'
curl_capture api_live_exit_readiness http://127.0.0.1:3030/api/live/exit-readiness
curl_capture api_activity http://127.0.0.1:3030/api/activity
curl_capture api_geoblock http://127.0.0.1:3030/api/geoblock

if [ "${BLINK_COLLECT_PREFLIGHT:-0}" = "1" ]; then
  run_capture preflight_live cargo --manifest-path "${ENGINE_DIR}/Cargo.toml" run -p engine -- --preflight-live
else
  {
    echo "skipped=true"
    echo "reason=Set BLINK_COLLECT_PREFLIGHT=1 to execute cargo run -p engine -- --preflight-live"
    echo "captured_at_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } >"${OUT_DIR}/preflight_live.txt"
fi

{
  echo "# Dirty Worktree Buckets"
  echo
  echo "engine_runtime:"
  git -C "${REPO_ROOT}" status --short -- blink-engine/crates blink-engine/Cargo.toml blink-engine/.env.example | sed 's/^/- /' || true
  echo
  echo "ui_static_assets:"
  git -C "${REPO_ROOT}" status --short -- blink-ui blink-engine/static/ui | sed 's/^/- /' || true
  echo
  echo "local_env_secrets:"
  git -C "${REPO_ROOT}" status --short -- '*.env' '*.env.*' '*/.env' '*/.env.*' | sed 's/^/- /' || true
  echo
  echo "generated_or_profiling:"
  git -C "${REPO_ROOT}" status --short -- blink-engine/logs logs docs/generated blink-engine/callgrind.out.* | sed 's/^/- /' || true
} >"${OUT_DIR}/dirty-worktree-buckets.md"

if [ -x "${SCRIPT_DIR}/index_go_live_evidence.py" ]; then
  "${SCRIPT_DIR}/index_go_live_evidence.py" "${OUT_DIR}" --repo-root "${REPO_ROOT}" --run-id "${RUN_ID}" >/dev/null
fi

echo "${OUT_DIR}"
