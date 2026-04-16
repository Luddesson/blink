#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────────────────────
# Push local code to Oracle ARM server and rebuild
# Usage: bash deploy/push-and-rebuild.sh <SERVER_IP> [SSH_KEY]
# ────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SERVER_IP="${1:?Usage: push-and-rebuild.sh <SERVER_IP> [SSH_KEY_PATH]}"
SSH_KEY="${2:-$HOME/.ssh/id_ed25519}"
SSH_USER="ubuntu"
REMOTE_DIR="/opt/blink/blink"
SSH_CMD="ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new $SSH_USER@$SERVER_IP"

echo "🚀 Deploying Blink to $SERVER_IP..."

# Step 1: Sync code (exclude build artifacts, node_modules, secrets)
echo "[1/4] Syncing code..."
rsync -azP --delete \
    --exclude='target/' \
    --exclude='node_modules/' \
    --exclude='.env' \
    --exclude='.env.*' \
    --exclude='*.pid' \
    --exclude='data/' \
    --exclude='logs/' \
    --exclude='*.pem' \
    --exclude='*.key' \
    -e "ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new" \
    . "$SSH_USER@$SERVER_IP:$REMOTE_DIR/"

# Step 2: Build engine on server
echo "[2/4] Building engine (release)..."
$SSH_CMD "sudo -u blink bash -c '
    source /opt/blink/.cargo/env
    cd $REMOTE_DIR/blink-engine
    cargo build --release -p engine 2>&1 | tail -3
'"

# Step 3: Rebuild web UI
echo "[3/4] Building web UI..."
$SSH_CMD "sudo -u blink bash -c '
    cd $REMOTE_DIR/blink-engine/web-ui
    npm ci --silent
    npm run build
'"

# Step 4: Restart service
echo "[4/4] Restarting blink-engine service..."
$SSH_CMD "sudo systemctl restart blink-engine"
sleep 3
$SSH_CMD "sudo systemctl status blink-engine --no-pager -l | head -15"

echo ""
echo "✅ Deployed successfully!"
echo "Dashboard: http://$SERVER_IP:5173"
