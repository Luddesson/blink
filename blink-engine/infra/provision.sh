#!/usr/bin/env bash
# ============================================================================
# Blink HFT Engine — Bare-Metal Server Provisioning Script
# Target: Ubuntu 22.04 LTS (kernel 5.15+)
# Owner:  AURA-1 (Lead Systems Architect)
#
# Usage:  chmod +x provision.sh && sudo ./provision.sh
# ============================================================================
set -euo pipefail

BLINK_USER="${BLINK_USER:-blink}"
BLINK_HOME="/opt/blink"
BLINK_REPO="${BLINK_REPO:-https://github.com/your-org/blink-engine.git}"

log() { echo -e "\n\033[1;32m[BLINK] $*\033[0m"; }
err() { echo -e "\n\033[1;31m[ERROR] $*\033[0m" >&2; exit 1; }

[[ $EUID -eq 0 ]] || err "Must be run as root"

# ────────────────────────────────────────────────────────────
# 1. System update & base packages
# ────────────────────────────────────────────────────────────
log "Updating system packages..."
apt-get update -qq && apt-get upgrade -y -qq

log "Installing build dependencies..."
apt-get install -y -qq \
    build-essential \
    pkg-config \
    libssl-dev \
    clang \
    llvm \
    linux-tools-generic \
    linux-tools-$(uname -r) \
    numactl \
    cpufrequtils \
    irqbalance \
    curl \
    git \
    jq \
    htop \
    bpftrace \
    chrony \
    ca-certificates \
    gnupg \
    lsb-release

# ────────────────────────────────────────────────────────────
# 2. Create blink service user
# ────────────────────────────────────────────────────────────
log "Creating service user '${BLINK_USER}'..."
if ! id -u "${BLINK_USER}" &>/dev/null; then
    useradd -r -m -d "${BLINK_HOME}" -s /bin/bash "${BLINK_USER}"
fi

# ────────────────────────────────────────────────────────────
# 3. Rust toolchain (rustup)
# ────────────────────────────────────────────────────────────
log "Installing Rust toolchain..."
if ! command -v rustup &>/dev/null; then
    su - "${BLINK_USER}" -c \
        'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable'
fi

# Ensure cargo is on PATH for subsequent commands
CARGO_BIN="${BLINK_HOME}/.cargo/bin"
export PATH="${CARGO_BIN}:${PATH}"

# ────────────────────────────────────────────────────────────
# 4. ClickHouse (official apt repo)
# ────────────────────────────────────────────────────────────
log "Installing ClickHouse..."
if ! command -v clickhouse-server &>/dev/null; then
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
fi

# ────────────────────────────────────────────────────────────
# 5. Foundry (forge, cast, anvil)
# ────────────────────────────────────────────────────────────
log "Installing Foundry..."
if ! command -v forge &>/dev/null; then
    su - "${BLINK_USER}" -c \
        'curl -L https://foundry.paradigm.xyz | bash && ${HOME}/.foundry/bin/foundryup'
fi

# ────────────────────────────────────────────────────────────
# 6. Clone blink-engine repository
# ────────────────────────────────────────────────────────────
log "Cloning blink-engine..."
if [[ ! -d "${BLINK_HOME}/blink-engine" ]]; then
    su - "${BLINK_USER}" -c "git clone '${BLINK_REPO}' '${BLINK_HOME}/blink-engine'"
fi

# ────────────────────────────────────────────────────────────
# 7. Build release binary
# ────────────────────────────────────────────────────────────
log "Building blink-engine (release)..."
su - "${BLINK_USER}" -c "
    cd '${BLINK_HOME}/blink-engine' && \
    source '${BLINK_HOME}/.cargo/env' && \
    cargo build --release
"

# ────────────────────────────────────────────────────────────
# 8. Install systemd service
# ────────────────────────────────────────────────────────────
log "Installing systemd service..."
cp "${BLINK_HOME}/blink-engine/infra/blink-engine.service" \
   /etc/systemd/system/blink-engine.service

# Create env file template if it doesn't exist
if [[ ! -f /etc/blink-engine.env ]]; then
    cat > /etc/blink-engine.env <<'ENVEOF'
# Blink Engine environment — edit with production values
CLOB_HOST=https://clob.polymarket.com
WS_URL=wss://ws-live-data.polymarket.com
RN1_WALLET=
MARKETS=
LIVE_TRADING=false
SIGNER_PRIVATE_KEY=
POLYMARKET_FUNDER_ADDRESS=
POLYMARKET_API_KEY=
POLYMARKET_API_SECRET=
POLYMARKET_API_PASSPHRASE=
CLICKHOUSE_URL=http://127.0.0.1:8123
LOG_LEVEL=info
ENVEOF
    chmod 600 /etc/blink-engine.env
    chown "${BLINK_USER}:${BLINK_USER}" /etc/blink-engine.env
fi

systemctl daemon-reload
systemctl enable blink-engine

# ────────────────────────────────────────────────────────────
# 9. NVMe mount for Reth data (skip if already mounted)
# ────────────────────────────────────────────────────────────
log "Preparing NVMe mount point for Reth..."
mkdir -p /mnt/nvme/reth
chown "${BLINK_USER}:${BLINK_USER}" /mnt/nvme/reth
if ! mountpoint -q /mnt/nvme 2>/dev/null; then
    echo "# NOTE: Add NVMe mount to /etc/fstab manually, e.g.:"
    echo "# /dev/nvme1n1p1 /mnt/nvme ext4 noatime,discard 0 2"
fi

# ────────────────────────────────────────────────────────────
# 10. Install chrony config for PTP
# ────────────────────────────────────────────────────────────
log "Installing chrony config..."
if [[ -f "${BLINK_HOME}/blink-engine/infra/chrony.conf" ]]; then
    cp /etc/chrony/chrony.conf /etc/chrony/chrony.conf.bak 2>/dev/null || true
    cp "${BLINK_HOME}/blink-engine/infra/chrony.conf" /etc/chrony/chrony.conf
    systemctl restart chrony
fi

# ────────────────────────────────────────────────────────────
# Done
# ────────────────────────────────────────────────────────────
log "Provisioning complete!"
log "Next steps:"
echo "  1. Edit /etc/blink-engine.env with production credentials"
echo "  2. Run infra/os_tune.sh for kernel-level HFT tuning"
echo "  3. Mount NVMe and start Reth: reth node --config infra/reth_config.toml"
echo "  4. Start the engine: systemctl start blink-engine"
