#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────
#  Blink Engine — Deploy / Update Script
#  Builds on server (with swap) and restarts services.
#  Usage: ./deploy.sh [--first-run]
# ─────────────────────────────────────────────────────────────────
set -euo pipefail

SERVER="root@5.161.100.38"
BLINK_DIR="/opt/blink"
REPO_URL="https://github.com/ludviggoogle/blink.git"  # Update with your actual repo URL
BRANCH="master"

FIRST_RUN=false
if [[ "${1:-}" == "--first-run" ]]; then
    FIRST_RUN=true
fi

echo "══════════════════════════════════════════════"
echo "  Blink Engine — Deploy to Hetzner"
echo "══════════════════════════════════════════════"

if $FIRST_RUN; then
    echo "[1/6] First run — provisioning server..."
    ssh $SERVER 'bash -s' < "$(dirname "$0")/provision.sh"
    echo ""
    echo "[2/6] Cloning repository..."
    ssh $SERVER "su - blink -c 'git clone ${REPO_URL} ${BLINK_DIR}/src || (cd ${BLINK_DIR}/src && git pull)'"
else
    echo "[1/6] Pulling latest code..."
    ssh $SERVER "cd ${BLINK_DIR}/src && sudo -u blink git fetch origin && sudo -u blink git reset --hard origin/${BRANCH}"
fi

echo "[3/6] Building engine (this takes 5-10 min on first build)..."
ssh $SERVER "source /root/.cargo/env && cd ${BLINK_DIR}/src/blink-engine && cargo build --release -p engine 2>&1 | tail -5"

echo "[4/6] Deploying binary and sidecar..."
ssh $SERVER <<'DEPLOY'
set -e
BLINK_DIR="/opt/blink"

# Copy engine binary
cp ${BLINK_DIR}/src/blink-engine/target/release/engine ${BLINK_DIR}/engine
chmod +x ${BLINK_DIR}/engine

# Build and copy active web UI assets
if [ -d "${BLINK_DIR}/src/blink-ui" ] && command -v npm >/dev/null 2>&1; then
    cd ${BLINK_DIR}/src/blink-ui
    npm ci --silent 2>/dev/null
    npm run build --silent 2>/dev/null
    mkdir -p ${BLINK_DIR}/static/ui/assets
    rm -f ${BLINK_DIR}/static/ui/index.html
    rm -f ${BLINK_DIR}/static/ui/assets/*
    cp -r ${BLINK_DIR}/src/blink-engine/static/ui/* ${BLINK_DIR}/static/ui/ 2>/dev/null || true
fi

# Copy alpha sidecar source
rsync -a --delete ${BLINK_DIR}/src/blink-engine/alpha-sidecar/ ${BLINK_DIR}/alpha-sidecar/

# Install sidecar dependencies
${BLINK_DIR}/sidecar-venv/bin/pip install -q -e ${BLINK_DIR}/alpha-sidecar/

# Fix ownership
chown -R blink:blink ${BLINK_DIR}

echo "  Binary, sidecar, and static assets deployed"
DEPLOY

echo "[5/6] Restarting services..."
ssh $SERVER "systemctl restart blink-engine && sleep 3 && systemctl restart blink-sidecar"

echo "[6/6] Verifying..."
sleep 5
ssh $SERVER <<'VERIFY'
echo "  Engine:  $(systemctl is-active blink-engine)"
echo "  Sidecar: $(systemctl is-active blink-sidecar)"
echo "  Memory:  $(free -h | grep Mem | awk '{print $3 "/" $2}')"
echo ""
# Quick health check
if curl -sf http://127.0.0.1:3030/api/status > /dev/null 2>&1; then
    echo "  ✅ Engine API responding"
    curl -sf http://127.0.0.1:3030/api/status | jq '{ws_connected, trading_paused, risk_status}'
else
    echo "  ⚠️  Engine API not responding yet (may still be starting)"
fi
VERIFY

echo ""
echo "══════════════════════════════════════════════"
echo "  Deploy complete!"
echo "  Monitor: ssh $SERVER journalctl -u blink-engine -f"
echo "  Dashboard: ssh -L 3030:localhost:3030 $SERVER"
echo "══════════════════════════════════════════════"
