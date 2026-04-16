#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────────────────────
# Blink Engine — Oracle Cloud ARM64 (Always Free) Provisioning Script
# Run as root: sudo ./oracle-arm-provision.sh
# Target: Ubuntu 22.04+ on VM.Standard.A1.Flex (aarch64)
# ────────────────────────────────────────────────────────────────────────────
set -euo pipefail

BLINK_USER="blink"
BLINK_HOME="/opt/blink"
REPO_URL="https://github.com/ludviggaldworthy/blink.git"
NODE_MAJOR=22

echo "══════════════════════════════════════════════════════"
echo "  Blink Engine — Oracle ARM64 Provisioner"
echo "══════════════════════════════════════════════════════"

# ── 1. System update & build tools ──────────────────────────────────────────
echo "[1/8] Updating system and installing build dependencies..."
apt-get update -qq
apt-get upgrade -y -qq
apt-get install -y -qq \
    build-essential pkg-config libssl-dev libclang-dev clang \
    git curl wget unzip htop tmux jq \
    iptables-persistent netfilter-persistent

# ── 2. Create blink service user ───────────────────────────────────────────
echo "[2/8] Creating blink user..."
if ! id "$BLINK_USER" &>/dev/null; then
    useradd -r -m -d "$BLINK_HOME" -s /bin/bash "$BLINK_USER"
fi
mkdir -p "$BLINK_HOME"
chown "$BLINK_USER:$BLINK_USER" "$BLINK_HOME"

# ── 3. Install Rust (as blink user) ────────────────────────────────────────
echo "[3/8] Installing Rust toolchain..."
su - "$BLINK_USER" -c '
    if [ ! -f "$HOME/.cargo/bin/rustup" ]; then
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    else
        $HOME/.cargo/bin/rustup update stable
    fi
'

# ── 4. Install Node.js (for web UI) ────────────────────────────────────────
echo "[4/8] Installing Node.js ${NODE_MAJOR}..."
if ! command -v node &>/dev/null; then
    curl -fsSL "https://deb.nodesource.com/setup_${NODE_MAJOR}.x" | bash -
    apt-get install -y -qq nodejs
fi
echo "Node $(node --version), npm $(npm --version)"

# ── 5. Clone or update repository ──────────────────────────────────────────
echo "[5/8] Setting up repository..."
REPO_DIR="$BLINK_HOME/blink"
if [ -d "$REPO_DIR/.git" ]; then
    su - "$BLINK_USER" -c "cd $REPO_DIR && git pull --ff-only"
else
    su - "$BLINK_USER" -c "git clone $REPO_URL $REPO_DIR"
fi

# ── 6. Build engine (release) ──────────────────────────────────────────────
echo "[6/8] Building Blink engine (release)... this takes ~10 minutes on ARM"
su - "$BLINK_USER" -c '
    source "$HOME/.cargo/env"
    cd '"$REPO_DIR"'/blink-engine
    cargo build --release -p engine 2>&1 | tail -5
'
echo "Binary: $(ls -lh "$REPO_DIR/blink-engine/target/release/engine")"

# ── 7. Build web UI ────────────────────────────────────────────────────────
echo "[7/8] Building web UI..."
su - "$BLINK_USER" -c "
    cd $REPO_DIR/blink-engine/web-ui
    npm ci --silent
    npm run build
"

# ── 8. Install systemd service ─────────────────────────────────────────────
echo "[8/8] Installing systemd service..."
cp "$REPO_DIR/deploy/blink-oracle.service" /etc/systemd/system/blink-engine.service

# Create data directory for persistence
su - "$BLINK_USER" -c "mkdir -p $REPO_DIR/blink-engine/data"

# Create .env template if it doesn't exist
ENV_FILE="$BLINK_HOME/.env"
if [ ! -f "$ENV_FILE" ]; then
    cat > "$ENV_FILE" << 'ENVEOF'
# ── Blink Engine Configuration (Oracle ARM64) ──
# Copy your trading config here.

# Mode
PAPER_TRADING=true
WEB_UI=true
RUST_LOG=info,engine=info

# Web
WS_BROADCAST_INTERVAL_SECS=10

# Sizing (Phase 6 defaults are baked in, override here if needed)
# PAPER_MAX_ORDER_USDC=25
# PAPER_SIZE_MULTIPLIER=0.10

# RN1 Wallet (required)
# RN1_WALLET_ADDRESS=

# Polymarket CLOB WebSocket
# POLY_WS_URL=wss://ws-subscriptions-clob.polymarket.com/ws/market

# API credentials (for live trading only)
# POLY_API_KEY=
# POLY_API_SECRET=
# POLY_PASSPHRASE=
ENVEOF
    chown "$BLINK_USER:$BLINK_USER" "$ENV_FILE"
    chmod 600 "$ENV_FILE"
fi

# Open firewall ports
iptables -I INPUT -p tcp --dport 5173 -j ACCEPT 2>/dev/null || true
iptables -I INPUT -p tcp --dport 7878 -j ACCEPT 2>/dev/null || true
netfilter-persistent save 2>/dev/null || true

# Enable service
systemctl daemon-reload
systemctl enable blink-engine

# ── Kernel tuning (lightweight, no HFT-grade changes) ──────────────────────
cat > /etc/sysctl.d/99-blink.conf << 'EOF'
# Blink network tuning
net.core.rmem_max=8388608
net.core.wmem_max=8388608
net.ipv4.tcp_rmem=4096 87380 8388608
net.ipv4.tcp_wmem=4096 65536 8388608
net.core.somaxconn=4096
vm.swappiness=10
EOF
sysctl --system -q

echo ""
echo "══════════════════════════════════════════════════════"
echo "  ✅ Blink Engine provisioned successfully!"
echo ""
echo "  Next steps:"
echo "  1. Edit config:    sudo nano $ENV_FILE"
echo "  2. Start engine:   sudo systemctl start blink-engine"
echo "  3. View logs:      journalctl -u blink-engine -f"
echo "  4. Dashboard:      http://$(hostname -I | awk '{print $1}'):5173"
echo "══════════════════════════════════════════════════════"
