# Oracle Cloud Always Free — Blink Deployment Guide

## Prerequisites
1. An Oracle Cloud account (free): https://cloud.oracle.com
2. SSH key pair (generate with `ssh-keygen -t ed25519` if you don't have one)

---

## Step 1: Create the VM (Oracle Console)

1. Log in → **Compute → Instances → Create Instance**
2. Settings:
   - **Name:** `blink-engine`
   - **Image:** Ubuntu 22.04 (or 24.04)
   - **Shape:** VM.Standard.A1.Flex → **4 OCPUs, 24 GB RAM** (Always Free)
   - **Boot volume:** 100 GB (free up to 200 GB total)
   - **Networking:** Create new VCN with public subnet
   - **SSH key:** Upload your `~/.ssh/id_ed25519.pub`
3. Click **Create** — wait ~2 minutes for provisioning
4. Note the **Public IP address** from the instance details page

## Step 2: Open Firewall Ports

In Oracle Console → **Networking → Virtual Cloud Networks → your VCN → Security Lists → Default**

Add **Ingress Rules**:
| Port | Protocol | Source | Purpose |
|------|----------|--------|---------|
| 22 | TCP | 0.0.0.0/0 | SSH (already open) |
| 5173 | TCP | 0.0.0.0/0 | Blink Dashboard (optional, Cloudflare tunnel is safer) |
| 7878 | TCP | 0.0.0.0/0 | Blink API (optional) |

Also run on the VM (Ubuntu firewall):
```bash
sudo iptables -I INPUT -p tcp --dport 5173 -j ACCEPT
sudo iptables -I INPUT -p tcp --dport 7878 -j ACCEPT
sudo netfilter-persistent save
```

## Step 3: Provision the Server

```bash
# From your Windows machine:
scp deploy/oracle-arm-provision.sh ubuntu@<PUBLIC_IP>:~/provision.sh
ssh ubuntu@<PUBLIC_IP>
chmod +x provision.sh
sudo ./provision.sh
```

This installs Rust, Node.js, builds Blink, and configures systemd. Takes ~15 minutes.

## Step 4: Configure Environment

```bash
ssh ubuntu@<PUBLIC_IP>
sudo nano /opt/blink/.env
```

Copy your local `.env` contents. Critical vars:
```
PAPER_TRADING=true
WEB_UI=true
RUST_LOG=info
WS_BROADCAST_INTERVAL_SECS=10
```

## Step 5: Start Blink

```bash
sudo systemctl start blink-engine
sudo systemctl status blink-engine   # verify running
journalctl -u blink-engine -f        # live logs
```

## Step 6: Access Dashboard

**Option A — Cloudflare Tunnel (recommended, free, secure):**
```bash
sudo /opt/blink/blink/deploy/setup-cloudflare-tunnel.sh
```
This gives you a public URL like `https://xxx.trycloudflare.com`

**Option B — Direct IP:**
Open `http://<PUBLIC_IP>:5173` in your browser.

## Step 7: Deploy Updates

From your Windows machine (requires Git Bash or WSL):
```bash
cd /path/to/blink
bash deploy/push-and-rebuild.sh <PUBLIC_IP>
```

This syncs code, rebuilds, and restarts the service automatically.

---

## Maintenance Commands

```bash
# View logs
journalctl -u blink-engine -f --no-pager

# Restart
sudo systemctl restart blink-engine

# Stop
sudo systemctl stop blink-engine

# Check resource usage
htop

# Disk usage
df -h /opt/blink
```

## Costs
**$0.00/month.** The A1.Flex shape with 4 OCPUs + 24 GB is Always Free.
Boot volume up to 200 GB is also free. Outbound data: 10 TB/month free.
