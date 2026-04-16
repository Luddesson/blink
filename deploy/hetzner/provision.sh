#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────
#  Blink Engine — Hetzner CPX11 Provisioning Script
#  Run ONCE on a fresh Ubuntu 22.04/24.04 server.
#  Usage: ssh root@5.161.100.38 'bash -s' < provision.sh
# ─────────────────────────────────────────────────────────────────
set -euo pipefail

BLINK_USER="blink"
BLINK_DIR="/opt/blink"
SWAP_SIZE="2G"

echo "══════════════════════════════════════════════"
echo "  Blink Engine — Hetzner Server Provisioning"
echo "══════════════════════════════════════════════"

# ── 1. System updates ───────────────────────────────────────────
echo "[1/9] Updating system packages..."
apt-get update -qq && apt-get upgrade -y -qq

# ── 2. Create swap (needed for Rust compilation on 2GB RAM) ────
echo "[2/9] Setting up ${SWAP_SIZE} swap..."
if [ ! -f /swapfile ]; then
    fallocate -l ${SWAP_SIZE} /swapfile
    chmod 600 /swapfile
    mkswap /swapfile
    swapon /swapfile
    echo '/swapfile none swap sw 0 0' >> /etc/fstab
    # Reduce swappiness for production (prefer RAM)
    echo 'vm.swappiness=10' >> /etc/sysctl.conf
    sysctl vm.swappiness=10
    echo "  Swap created and enabled"
else
    echo "  Swap already exists"
fi

# ── 3. Install dependencies ────────────────────────────────────
echo "[3/9] Installing dependencies..."
apt-get install -y -qq \
    build-essential pkg-config libssl-dev \
    python3 python3-pip python3-venv \
    git curl ufw jq \
    2>/dev/null

# ── 4. Install Rust ────────────────────────────────────────────
echo "[4/9] Installing Rust..."
if ! command -v cargo &>/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env"
    echo "  Rust $(rustc --version) installed"
else
    source "$HOME/.cargo/env" 2>/dev/null || true
    echo "  Rust already installed: $(rustc --version)"
fi

# ── 5. Create blink user and directory ─────────────────────────
echo "[5/9] Setting up blink user and directories..."
if ! id "$BLINK_USER" &>/dev/null; then
    useradd -r -m -s /bin/bash "$BLINK_USER"
    echo "  User '$BLINK_USER' created"
fi

mkdir -p ${BLINK_DIR}/{data,logs,logs/sessions,logs/reports,static}
chown -R ${BLINK_USER}:${BLINK_USER} ${BLINK_DIR}

# ── 6. Setup Python venv for alpha sidecar ─────────────────────
echo "[6/9] Setting up Python virtual environment..."
if [ ! -d "${BLINK_DIR}/sidecar-venv" ]; then
    python3 -m venv "${BLINK_DIR}/sidecar-venv"
    echo "  Python venv created at ${BLINK_DIR}/sidecar-venv"
fi

# ── 7. Firewall ────────────────────────────────────────────────
echo "[7/9] Configuring firewall..."
ufw --force reset >/dev/null 2>&1
ufw default deny incoming
ufw default allow outgoing
ufw allow 22/tcp   comment 'SSH'
# Port 3030 is NOT exposed — use Cloudflare Tunnel or SSH tunnel
ufw --force enable
echo "  Firewall: SSH only (port 3030 internal only)"

# ── 8. Systemd services ────────────────────────────────────────
echo "[8/9] Installing systemd services..."

cat > /etc/systemd/system/blink-engine.service << 'EOF'
[Unit]
Description=Blink Trading Engine
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=blink
Group=blink
WorkingDirectory=/opt/blink
EnvironmentFile=/opt/blink/.env
ExecStart=/opt/blink/engine
Restart=always
RestartSec=10
StartLimitIntervalSec=600
StartLimitBurst=10

# Resource limits
MemoryMax=1536M
MemoryHigh=1024M
CPUQuota=180%

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/blink/data /opt/blink/logs
PrivateTmp=true

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=blink-engine

[Install]
WantedBy=multi-user.target
EOF

cat > /etc/systemd/system/blink-sidecar.service << 'EOF'
[Unit]
Description=Blink Alpha AI Sidecar
After=blink-engine.service
Requires=blink-engine.service

[Service]
Type=simple
User=blink
Group=blink
WorkingDirectory=/opt/blink/alpha-sidecar
EnvironmentFile=/opt/blink/.env
Environment=VIRTUAL_ENV=/opt/blink/sidecar-venv
Environment=PATH=/opt/blink/sidecar-venv/bin:/usr/local/bin:/usr/bin:/bin
Environment=ALPHA_DB_PATH=/opt/blink/data/alpha_predictions.db
ExecStartPre=/bin/sleep 5
ExecStart=/opt/blink/sidecar-venv/bin/python -m alpha_sidecar.main
Restart=always
RestartSec=10
StartLimitIntervalSec=300
StartLimitBurst=5

# Resource limits
MemoryMax=512M
MemoryHigh=384M

# Security
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/blink/data /opt/blink/logs
PrivateTmp=true

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=blink-sidecar

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable blink-engine blink-sidecar

echo "  Services installed and enabled"

# ── 9. Log rotation ────────────────────────────────────────────
echo "[9/9] Setting up log rotation..."

cat > /etc/logrotate.d/blink << 'EOF'
/opt/blink/logs/*.log /opt/blink/logs/sessions/*.log {
    daily
    rotate 14
    compress
    delaycompress
    missingok
    notifempty
    copytruncate
    create 0644 blink blink
    maxsize 100M
    dateext
}
EOF

# Disk usage monitoring cron — alerts if /opt/blink exceeds 5GB
cat > /etc/cron.daily/blink-disk-check << 'CRON'
#!/bin/bash
SIZE=$(du -sm /opt/blink/data /opt/blink/logs 2>/dev/null | awk '{s+=$1} END {print s}')
if [ "$SIZE" -gt 5000 ]; then
    echo "BLINK DISK WARNING: /opt/blink using ${SIZE}MB (>5GB)" | logger -t blink-disk
    # Prune oldest session logs beyond 30
    ls -t /opt/blink/logs/sessions/*.log 2>/dev/null | tail -n +31 | xargs -r rm -f
fi
CRON
chmod +x /etc/cron.daily/blink-disk-check

echo ""
echo "══════════════════════════════════════════════"
echo "  Provisioning complete!"
echo ""
echo "  Next steps:"
echo "  1. Clone repo:  su - blink -c 'git clone <repo> /opt/blink/src'"
echo "  2. Copy .env:   nano /opt/blink/.env"
echo "  3. Build:       su - blink -c 'cd /opt/blink/src/blink-engine && cargo build --release -p engine'"
echo "  4. Deploy:      Run deploy.sh (from local machine)"
echo "══════════════════════════════════════════════"
