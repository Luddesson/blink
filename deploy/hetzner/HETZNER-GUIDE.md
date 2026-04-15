# Blink Engine — Hetzner CPX11 Deployment Guide

## Server Specs
- **Hetzner CPX11**: 2 vCPU (AMD), 2 GB RAM, 40 GB SSD, Ubuntu 22.04/24.04
- **Cost**: ~€4.85/month
- **IP**: `5.161.100.38`

## Quick Start

### First-time deploy (from Windows PowerShell):
```powershell
cd C:\Users\ludvi\Documents\GitHub\blink
.\deploy\deploy-hetzner.ps1 -FirstRun
```

This will:
1. Provision the server (install Rust, Python, create user, firewall, swap, systemd)
2. Sync code via rsync
3. Push .env secrets
4. Build engine on server (~7 min first build, with 2GB swap)
5. Deploy binary + sidecar
6. Start services

### Update deploy (push code changes):
```powershell
.\deploy\deploy-hetzner.ps1
```

### Other commands:
```powershell
.\deploy\deploy-hetzner.ps1 -Status   # Check health
.\deploy\deploy-hetzner.ps1 -Logs     # Tail engine logs
.\deploy\deploy-hetzner.ps1 -Tunnel   # SSH tunnel to dashboard
.\deploy\deploy-hetzner.ps1 -Restart  # Restart services
.\deploy\deploy-hetzner.ps1 -Stop     # Stop services
.\deploy\deploy-hetzner.ps1 -EnvOnly  # Push .env without rebuild
```

## Architecture on Server

```
/opt/blink/
├── engine              ← compiled binary
├── .env                ← secrets (chmod 600)
├── alpha-sidecar/      ← Python source
├── sidecar-venv/       ← Python virtualenv
├── static/             ← Web UI assets
├── data/
│   ├── paper_portfolio_state.json
│   ├── paper_warm_state.json
│   └── alpha_predictions.db
├── logs/
│   ├── sessions/
│   └── reports/
└── src/                ← git repo (for rebuilds)
```

## Services

| Service | Port | Description |
|---------|------|-------------|
| `blink-engine` | 3030 (internal) | Trading engine + Web UI |
| `blink-sidecar` | connects to 7878 | Alpha AI sidecar |

Both auto-restart on crash. Engine starts first, sidecar depends on it.

```bash
# On server:
systemctl status blink-engine
systemctl status blink-sidecar
journalctl -u blink-engine -f --no-pager
journalctl -u blink-sidecar -f --no-pager
```

## Accessing the Dashboard

Port 3030 is **NOT exposed** to the internet (security: unauthenticated API).

### Option A: SSH Tunnel (simplest)
```powershell
.\deploy\deploy-hetzner.ps1 -Tunnel
# Opens http://localhost:3030
```

### Option B: Cloudflare Tunnel (persistent access)
```bash
# On server:
curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -o /usr/local/bin/cloudflared
chmod +x /usr/local/bin/cloudflared
cloudflared tunnel login
cloudflared tunnel create blink
cloudflared tunnel route dns blink blink.yourdomain.com
cloudflared tunnel run --url http://localhost:3030 blink
```

## Security

- **Firewall**: Only port 22 (SSH) is open
- **Port 3030**: Internal only — use SSH tunnel or Cloudflare Tunnel
- **Port 7878**: Internal only (engine RPC)
- **.env**: Owned by `blink` user, `chmod 600`
- **Services**: Run as unprivileged `blink` user with systemd hardening

## Memory Budget (2 GB)

| Component | Expected RSS |
|-----------|-------------|
| OS + systemd | ~200 MB |
| Rust engine | ~50-150 MB |
| Python sidecar | ~100-200 MB |
| Swap (2 GB) | For builds only |
| **Headroom** | ~1.2 GB+ |

## Updating .env

```powershell
# Edit locally, then push:
.\deploy\deploy-hetzner.ps1 -EnvOnly
.\deploy\deploy-hetzner.ps1 -Restart
```

## Troubleshooting

### Engine won't start
```bash
ssh root@5.161.100.38
journalctl -u blink-engine -n 50 --no-pager
cat /opt/blink/.env | head  # check config
```

### Out of memory during build
```bash
# Check swap
swapon --show
free -h
# Swap should be 2G, if not:
fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
```

### Sidecar crashes
```bash
journalctl -u blink-sidecar -n 30 --no-pager
# Common: missing OPENAI_API_KEY in .env
```
