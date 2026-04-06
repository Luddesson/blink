# Blink Engine — Server Provisioning Requirements

Target: **AWS c7i.metal-24xl** (48 physical cores, 192 GiB RAM, up to 40 Gbps network)  
Region: **us-east-1** (lowest latency to Polymarket CLOB / Base L2 RPC endpoints)

---

## 1. Instance Selection

| Parameter | Value |
|---|---|
| Instance type | `c7i.metal-24xl` |
| Region | `us-east-1` (N. Virginia) |
| AZ | `us-east-1a` or `us-east-1b` (pick same AZ as your ENI/EIP) |
| AMI | Ubuntu 24.04 LTS (HVM, SSD, `x86_64`) |
| Tenancy | Dedicated Host preferred; default acceptable for paper phase |
| Placement Group | `cluster` strategy — reduces east-west latency if co-locating ClickHouse |

---

## 2. Storage

| Mount | Type | Size | Purpose |
|---|---|---|---|
| `/` (root) | `gp3`, 200 GiB, 3000 IOPS | OS + binaries |
| `/data/clickhouse` | NVMe instance store (ephemeral) or `io2`, 500 GiB, 10 000 IOPS | ClickHouse data |
| `/data/logs` | `gp3`, 100 GiB | Engine + system logs |

> **NVMe mount** (if using instance store):  
> ```bash
> mkfs.ext4 /dev/nvme1n1
> mkdir -p /data/clickhouse
> mount /dev/nvme1n1 /data/clickhouse
> echo '/dev/nvme1n1 /data/clickhouse ext4 defaults,noatime 0 2' >> /etc/fstab
> ```

---

## 3. Networking

| Resource | Requirement |
|---|---|
| Elastic IP | 1× static EIP attached to instance — whitelist this IP with Polymarket API |
| Security Group (ingress) | 22/tcp (SSH, from operator IPs only), 8080/tcp (Blink dashboard, operator IPs only), 8123/tcp (ClickHouse HTTP, localhost only) |
| Security Group (egress) | 443/tcp (Polymarket CLOB + Alchemy RPC), 80/tcp, 8545/tcp (Reth RPC if self-hosted) |
| Enhanced Networking | Enabled by default on c7i (ENA); verify with `ethtool -i eth0` |
| MTU | Set to 9001 (jumbo frames) for intra-VPC traffic: `ip link set eth0 mtu 9001` |

---

## 4. OS & Kernel Tuning

```bash
# TCP tuning for low-latency WebSocket + REST
echo 'net.core.rmem_max=134217728'        >> /etc/sysctl.d/99-blink.conf
echo 'net.core.wmem_max=134217728'        >> /etc/sysctl.d/99-blink.conf
echo 'net.ipv4.tcp_rmem=4096 87380 134217728' >> /etc/sysctl.d/99-blink.conf
echo 'net.ipv4.tcp_wmem=4096 65536 134217728' >> /etc/sysctl.d/99-blink.conf
echo 'net.ipv4.tcp_low_latency=1'         >> /etc/sysctl.d/99-blink.conf
echo 'net.core.netdev_max_backlog=30000'  >> /etc/sysctl.d/99-blink.conf
echo 'vm.swappiness=1'                    >> /etc/sysctl.d/99-blink.conf
sysctl -p /etc/sysctl.d/99-blink.conf

# CPU governor: performance mode
apt-get install -y cpufrequtils
echo 'GOVERNOR=performance' > /etc/default/cpufrequtils
systemctl restart cpufrequtils

# Disable transparent huge pages (reduces TLB jitter)
echo never > /sys/kernel/mm/transparent_hugepage/enabled
echo never > /sys/kernel/mm/transparent_hugepage/defrag
echo 'echo never > /sys/kernel/mm/transparent_hugepage/enabled' >> /etc/rc.local
echo 'echo never > /sys/kernel/mm/transparent_hugepage/defrag'  >> /etc/rc.local
```

---

## 5. Software Prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
source ~/.cargo/env

# Docker (for ClickHouse)
apt-get install -y docker.io docker-compose-plugin
systemctl enable docker && systemctl start docker

# Build dependencies
apt-get install -y build-essential pkg-config libssl-dev clang lld

# Foundry (for on-chain verification tooling)
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

---

## 6. IAM / Secrets

| Secret | Storage | Notes |
|---|---|---|
| `POLYMARKET_API_KEY` | AWS Secrets Manager or `.env` (chmod 600) | Never commit to git |
| `POLYMARKET_SECRET` | Same | |
| `POLYMARKET_PASSPHRASE` | Same | |
| `SIGNER_PRIVATE_KEY` | AWS Secrets Manager preferred | Key used to sign EIP-712 orders |
| `ALCHEMY_RPC_URL` | `.env` | Base mainnet RPC endpoint |

Load at runtime:
```bash
# Example: fetch from Secrets Manager into env
export $(aws secretsmanager get-secret-value --secret-id blink/live --query SecretString --output text | jq -r 'to_entries|.[]|"\(.key)=\(.value)"')
```

---

## 7. Pre-Flight Checklist

Run before switching `LIVE_TRADING=true`:

- [ ] EIP whitelisted with Polymarket (support ticket)
- [ ] `--preflight-live` passes (market data reachable, auth headers valid)
- [ ] ClickHouse running: `docker compose up -d && docker compose ps`
- [ ] 7-day paper run completed with zero crashes (started 2026-04-06)
- [ ] Circuit breaker test passed (`kill_switch_trips_at_paper_trading_nav`)
- [ ] Halmos formal proofs pass (`make verify` in `formal/`)
- [ ] `RiskConfig`: `max_single_order_usdc=5.0` for canary phase
- [ ] Monitoring dashboard reachable at `:8080`
- [ ] `/api/failsafe` endpoint returns healthy snapshot
- [ ] Backup key held offline (hardware wallet or printed)

---

## 8. Deployment

```bash
git clone https://github.com/<org>/Blink.git /opt/blink
cd /opt/blink/blink-engine
cp .env.example .env && $EDITOR .env          # fill in secrets
cargo build --release -p engine
docker compose up -d clickhouse
./target/release/engine --preflight-live      # dry-run check
LIVE_TRADING=true ./target/release/engine     # go live
```

Use a `systemd` unit or `tmux` session to keep the engine running across SSH disconnections.
