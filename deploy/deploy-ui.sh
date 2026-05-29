#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
UI_DIR="$REPO_ROOT/blink-ui"
BUILD_OUT="$REPO_ROOT/blink-engine/static/ui"
SSH_CONFIG="${BLINK_SSH_CONFIG:-$REPO_ROOT/deploy/ssh/blink-ssh-config}"
REMOTE="${BLINK_SSH_HOST:-blink}"
REMOTE_STATIC="/opt/blink/static/ui"

echo "[deploy-ui] Building..."
cd "$UI_DIR"
npx vite build --mode production 2>&1 | tail -3

echo "[deploy-ui] Uploading to $REMOTE..."
ssh -F "$SSH_CONFIG" "$REMOTE" "mkdir -p $REMOTE_STATIC/assets && rm -f $REMOTE_STATIC/index.html && rm -f $REMOTE_STATIC/assets/*"
scp -F "$SSH_CONFIG" -q "$BUILD_OUT/index.html" "$REMOTE:$REMOTE_STATIC/index.html"
scp -F "$SSH_CONFIG" -q "$BUILD_OUT/assets/"* "$REMOTE:$REMOTE_STATIC/assets/"

echo "[deploy-ui] Restarting blink-engine..."
ssh -F "$SSH_CONFIG" "$REMOTE" "chown -R blink:blink $REMOTE_STATIC && systemctl restart blink-engine"

echo "[deploy-ui] Done! UI live on server."
