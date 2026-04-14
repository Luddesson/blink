#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────────────────────
# Setup Cloudflare Tunnel for Blink Dashboard (free, no domain needed)
# Creates a persistent systemd service for the tunnel.
# Run as root: sudo ./setup-cloudflare-tunnel.sh [PORT]
# ────────────────────────────────────────────────────────────────────────────
set -euo pipefail

ARCH=$(dpkg --print-architecture)  # arm64 on Oracle ARM
TUNNEL_PORT="${1:-5173}"

echo "🌐 Setting up Cloudflare Tunnel for Blink Dashboard..."

# Install cloudflared if not present
if ! command -v cloudflared &>/dev/null; then
    echo "[1/3] Installing cloudflared ($ARCH)..."
    curl -fsSL "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-${ARCH}.deb" -o /tmp/cloudflared.deb
    dpkg -i /tmp/cloudflared.deb
    rm /tmp/cloudflared.deb
else
    echo "[1/3] cloudflared already installed: $(cloudflared --version)"
fi

# Create systemd service for persistent tunnel
echo "[2/3] Creating systemd tunnel service..."
cat > /etc/systemd/system/blink-tunnel.service << EOF
[Unit]
Description=Cloudflare Tunnel for Blink Dashboard
After=network-online.target blink-engine.service
Wants=network-online.target

[Service]
Type=simple
User=blink
ExecStart=/usr/bin/cloudflared tunnel --url http://localhost:${TUNNEL_PORT} --no-autoupdate
Restart=always
RestartSec=10

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=blink-tunnel

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable blink-tunnel
systemctl restart blink-tunnel

# Wait for tunnel URL to appear in logs
echo "[3/3] Waiting for tunnel URL..."
sleep 5
TUNNEL_URL=$(journalctl -u blink-tunnel --no-pager -n 50 | grep -oP 'https://[a-z0-9-]+\.trycloudflare\.com' | tail -1 || true)

echo ""
echo "══════════════════════════════════════════════════════"
if [ -n "$TUNNEL_URL" ]; then
    echo "  ✅ Tunnel active!"
    echo "  Dashboard: $TUNNEL_URL"
else
    echo "  ⏳ Tunnel starting... check URL with:"
    echo "  journalctl -u blink-tunnel -f"
fi
echo ""
echo "  Manage:"
echo "    sudo systemctl status blink-tunnel"
echo "    sudo systemctl restart blink-tunnel"
echo "    journalctl -u blink-tunnel -f"
echo "══════════════════════════════════════════════════════"
