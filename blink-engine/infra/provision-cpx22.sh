#!/usr/bin/env bash
# ============================================================================
# Blink Engine — Hetzner CPX22 Setup Script
# Target: Ubuntu 22.04 LTS (VPS — no HFT kernel tuning)
# Purpose: Paper trading soak test / development
#
# Usage:
#   chmod +x provision-cpx22.sh && sudo ./provision-cpx22.sh
# ============================================================================
set -euo pipefail

BLINK_USER="blink"
BLINK_HOME="/opt/blink"
REPO_URL="https://github.com/Luddesson/blink.git"
REPO_BRANCH="claude/trade-bot-live-ready-6zaZf"

log()  { echo -e "\n\033[1;32m[BLINK] $*\033[0m"; }
warn() { echo -e "\n\033[1;33m[WARN]  $*\033[0m"; }
err()  { echo -e "\n\033[1;31m[ERROR] $*\033[0m" >&2; exit 1; }

[[ $EUID -eq 0 ]] || err "Must be run as root: sudo ./provision-cpx22.sh"

log "Starting Blink CPX22 setup..."
log "Target directory: ${BLINK_HOME}"

# ── 1. System update ────────────────────────────────────────────────────────
log "Updating system packages (this takes a few minutes)..."
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get upgrade -y -qq

log "Installing build dependencies..."
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    build-essential \
    pkg-config \
    libssl-dev \
    clang \
    llvm \
    curl \
    git \
    jq \
    htop \
    chrony \
    ca-certificates \
    gnupg \
    lsb-release \
    screen \
    tmux

# ── 2. Create blink service user ────────────────────────────────────────────
log "Creating service user '${BLINK_USER}'..."
if ! id -u "${BLINK_USER}" &>/dev/null; then
    useradd -r -m -d "${BLINK_HOME}" -s /bin/bash "${BLINK_USER}"
    log "  User '${BLINK_USER}' created at ${BLINK_HOME}"
else
    log "  User '${BLINK_USER}' already exists — skipping"
fi

mkdir -p "${BLINK_HOME}"
chown -R "${BLINK_USER}:${BLINK_USER}" "${BLINK_HOME}"

# ── 3. Rust toolchain ────────────────────────────────────────────────────────
log "Installing Rust toolchain (stable)..."
if ! su - "${BLINK_USER}" -c 'command -v cargo &>/dev/null'; then
    su - "${BLINK_USER}" -c \
        'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --no-modify-path'
    log "  Rust installed"
else
    log "  Rust already installed — skipping"
fi

CARGO_BIN="${BLINK_HOME}/.cargo/bin"
export PATH="${CARGO_BIN}:${PATH}"

# Verify
su - "${BLINK_USER}" -c "source ~/.cargo/env && rustc --version && cargo --version"

# ── 4. ClickHouse ────────────────────────────────────────────────────────────
log "Installing ClickHouse..."
if ! command -v clickhouse-server &>/dev/null; then
    # Official ClickHouse apt repo
    curl -fsSL 'https://packages.clickhouse.com/rpm/lts/repodata/repomd.xml.key' \
        | gpg --dearmor -o /usr/share/keyrings/clickhouse-keyring.gpg

    echo "deb [signed-by=/usr/share/keyrings/clickhouse-keyring.gpg] \
https://packages.clickhouse.com/deb stable main" \
        > /etc/apt/sources.list.d/clickhouse.list

    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
        clickhouse-server clickhouse-client

    systemctl enable clickhouse-server
    systemctl start clickhouse-server

    # Wait for it to be ready
    sleep 5
    clickhouse-client --query "SELECT 1" && log "  ClickHouse is running" \
        || warn "  ClickHouse started but not yet responding — may need a moment"
else
    log "  ClickHouse already installed"
    systemctl start clickhouse-server || true
fi

# ── 5. Clone / update repository ────────────────────────────────────────────
log "Setting up repository..."
REPO_DIR="${BLINK_HOME}/blink"

if [[ ! -d "${REPO_DIR}/.git" ]]; then
    log "  Cloning from ${REPO_URL} (branch: ${REPO_BRANCH})..."
    su - "${BLINK_USER}" -c "
        git clone --branch '${REPO_BRANCH}' '${REPO_URL}' '${REPO_DIR}'
    "
else
    log "  Repository exists — pulling latest..."
    su - "${BLINK_USER}" -c "
        cd '${REPO_DIR}' && git fetch origin && git checkout '${REPO_BRANCH}' && git pull origin '${REPO_BRANCH}'
    "
fi

# ── 6. Build release binary ──────────────────────────────────────────────────
log "Building blink-engine release binary (this takes 5–15 minutes)..."
su - "${BLINK_USER}" -c "
    source ~/.cargo/env
    cd '${REPO_DIR}/blink-engine'
    cargo build --release -p engine
"
log "  Build complete: ${REPO_DIR}/blink-engine/target/release/engine"

# ── 7. Log directories ───────────────────────────────────────────────────────
log "Creating log directories..."
mkdir -p "${REPO_DIR}/blink-engine/logs/sessions"
mkdir -p "${REPO_DIR}/blink-engine/logs/reports"
chown -R "${BLINK_USER}:${BLINK_USER}" "${REPO_DIR}/blink-engine/logs"

# ── 8. Install CPX22-specific systemd service ────────────────────────────────
log "Installing systemd service..."
cp "${REPO_DIR}/blink-engine/infra/blink-engine-cpx22.service" \
   /etc/systemd/system/blink-engine.service

systemctl daemon-reload
systemctl enable blink-engine
log "  Service installed and enabled (not started yet)"

# ── 9. Environment file ───────────────────────────────────────────────────────
log "Creating environment file..."
ENV_FILE="/etc/blink-engine.env"

if [[ ! -f "${ENV_FILE}" ]]; then
    cp "${REPO_DIR}/blink-engine/infra/paper-soak-test.env" "${ENV_FILE}"
    chmod 600 "${ENV_FILE}"
    chown root:root "${ENV_FILE}"
    log "  Created ${ENV_FILE} — REVIEW IT before starting the engine"
else
    log "  ${ENV_FILE} already exists — not overwriting"
    warn "  Make sure it has PAPER_TRADING=true and correct MARKETS"
fi

# ── 10. File descriptor limits ───────────────────────────────────────────────
log "Setting file descriptor limits..."
cat > /etc/security/limits.d/99-blink.conf <<'EOF'
blink    soft    nofile    65536
blink    hard    nofile    65536
EOF

# ── Done ─────────────────────────────────────────────────────────────────────
log "======================================================"
log " SETUP COMPLETE"
log "======================================================"
echo ""
echo "  Next steps:"
echo ""
echo "  1. Review the environment file:"
echo "       sudo nano /etc/blink-engine.env"
echo "       — Set MARKETS to real Polymarket token IDs"
echo "       — Set RN1_WALLET to the wallet you want to track"
echo ""
echo "  2. Start the engine:"
echo "       sudo systemctl start blink-engine"
echo ""
echo "  3. Watch logs:"
echo "       sudo journalctl -u blink-engine -f"
echo ""
echo "  4. Stop the engine:"
echo "       sudo systemctl stop blink-engine"
echo ""
echo "  Repo:   ${REPO_DIR}"
echo "  Logs:   ${REPO_DIR}/blink-engine/logs/sessions/"
echo "  Binary: ${REPO_DIR}/blink-engine/target/release/engine"
echo ""
