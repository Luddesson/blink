#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
UI_DIR="$REPO_ROOT/blink-ui"
BUILD_OUT="$REPO_ROOT/blink-engine/static/ui"
REMOTE="blink"
REMOTE_STATIC="/opt/blink/static"

echo "[deploy-ui] Building..."
cd "$UI_DIR"
npx vite build --mode production 2>&1 | tail -3

echo "[deploy-ui] Uploading to $REMOTE..."
scp -q "$BUILD_OUT/index.html" "$REMOTE:$REMOTE_STATIC/index.html"
scp -q "$BUILD_OUT/assets/"* "$REMOTE:$REMOTE_STATIC/assets/"

echo "[deploy-ui] Restarting blink-engine..."
ssh "$REMOTE" "systemctl restart blink-engine"

echo "[deploy-ui] Done! UI live on server."
