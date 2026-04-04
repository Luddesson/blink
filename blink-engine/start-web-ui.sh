#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Blink Engine — Web UI Launcher
#
# Starts the Rust engine backend (with WEB_UI=true) and the Vite dev server
# for hot-reloading frontend development. Press Ctrl-C to stop both.
#
# Usage:
#   ./start-web-ui.sh              # dev mode  (Vite HMR on :5173, API on :3030)
#   ./start-web-ui.sh --prod       # prod mode (serves built frontend on :3030)
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

WEB_UI_PORT="${WEB_UI_PORT:-3030}"
VITE_PORT="${VITE_PORT:-5173}"
MODE="${1:-dev}"

# ── Colours ──────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
NC='\033[0m'

cleanup() {
    echo -e "\n${YELLOW}Shutting down...${NC}"
    kill $ENGINE_PID 2>/dev/null || true
    [ -n "${VITE_PID:-}" ] && kill $VITE_PID 2>/dev/null || true
    wait 2>/dev/null
    echo -e "${GREEN}Done.${NC}"
}
trap cleanup EXIT INT TERM

# ── Check .env exists ────────────────────────────────────────────────────────
if [ ! -f .env ]; then
    echo -e "${YELLOW}No .env file found. Copying from .env.example...${NC}"
    cp .env.example .env
    echo -e "${YELLOW}Edit .env with your configuration before running in production.${NC}"
fi

# ── Build frontend if prod mode ─────────────────────────────────────────────
if [ "$MODE" = "--prod" ] || [ "$MODE" = "prod" ]; then
    echo -e "${CYAN}[1/2] Building frontend for production...${NC}"
    cd web-ui
    npm install --silent 2>/dev/null
    npx vite build
    cd "$SCRIPT_DIR"

    echo -e "${CYAN}[2/2] Starting Blink Engine (production, port ${WEB_UI_PORT})...${NC}"
    export WEB_UI=true
    export WEB_UI_PORT="$WEB_UI_PORT"
    export WEB_UI_STATIC_DIR="web-ui/dist"
    cargo run --release &
    ENGINE_PID=$!

    echo ""
    echo -e "${GREEN}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}║   Blink Engine Web UI running at:                       ║${NC}"
    echo -e "${GREEN}║   http://localhost:${WEB_UI_PORT}                                 ║${NC}"
    echo -e "${GREEN}╚══════════════════════════════════════════════════════════╝${NC}"
    echo ""

    wait $ENGINE_PID
else
    # ── Dev mode: run both backend and Vite dev server ───────────────────
    echo -e "${CYAN}[1/3] Installing frontend dependencies...${NC}"
    cd web-ui
    npm install --silent 2>/dev/null
    cd "$SCRIPT_DIR"

    echo -e "${CYAN}[2/3] Starting Blink Engine backend (port ${WEB_UI_PORT})...${NC}"
    export WEB_UI=true
    export WEB_UI_PORT="$WEB_UI_PORT"
    cargo run &
    ENGINE_PID=$!

    echo -e "${CYAN}[3/3] Starting Vite dev server (port ${VITE_PORT})...${NC}"
    cd web-ui
    npx vite --port "$VITE_PORT" &
    VITE_PID=$!
    cd "$SCRIPT_DIR"

    echo ""
    echo -e "${GREEN}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}║   Blink Engine Web UI (dev mode):                       ║${NC}"
    echo -e "${GREEN}║   Frontend:  http://localhost:${VITE_PORT}                        ║${NC}"
    echo -e "${GREEN}║   API:       http://localhost:${WEB_UI_PORT}                        ║${NC}"
    echo -e "${GREEN}║                                                          ║${NC}"
    echo -e "${GREEN}║   Press Ctrl-C to stop both servers.                     ║${NC}"
    echo -e "${GREEN}╚══════════════════════════════════════════════════════════╝${NC}"
    echo ""

    wait $ENGINE_PID $VITE_PID
fi
