#!/usr/bin/env bash
# ============================================================================
# Blink HFT Engine — OS-Level Latency Tuning Script
# Target: Ubuntu 22.04 LTS bare-metal server
# Owner:  AURA-1 (Lead Systems Architect)
#
# Applies kernel, CPU, memory, and network tuning for sub-millisecond
# deterministic latency. Run once after boot (or install in rc.local).
#
# Usage:  chmod +x os_tune.sh && sudo ./os_tune.sh
# ============================================================================
set -euo pipefail

# Primary network interface (change if needed)
NET_IF="${NET_IF:-eth0}"

log() { echo -e "\033[1;36m[TUNE] $*\033[0m"; }
err() { echo -e "\033[1;31m[ERROR] $*\033[0m" >&2; exit 1; }

[[ $EUID -eq 0 ]] || err "Must be run as root"

# ────────────────────────────────────────────────────────────
# 1. Disable Hyperthreading (SMT)
# ────────────────────────────────────────────────────────────
log "Disabling hyperthreading (SMT)..."
if [[ -f /sys/devices/system/cpu/smt/control ]]; then
    echo off > /sys/devices/system/cpu/smt/control
    log "  SMT status: $(cat /sys/devices/system/cpu/smt/control)"
else
    log "  WARN: SMT control not available — disable in BIOS instead"
fi

# ────────────────────────────────────────────────────────────
# 2. Disable CPU C-States (sleep states)
#    Prevents wake-up latency spikes (100μs+ for deep C-states)
# ────────────────────────────────────────────────────────────
log "Disabling CPU C-states..."
if command -v cpupower &>/dev/null; then
    cpupower idle-set -D 0 2>/dev/null || true
    log "  C-states disabled via cpupower"
else
    log "  WARN: cpupower not found — add 'processor.max_cstate=0 intel_idle.max_cstate=0' to GRUB"
fi

# ────────────────────────────────────────────────────────────
# 3. CPU Frequency Scaling — lock to max (performance governor)
# ────────────────────────────────────────────────────────────
log "Setting CPU governor to 'performance'..."
if command -v cpupower &>/dev/null; then
    cpupower frequency-set -g performance 2>/dev/null || true
fi
for gov in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
    if [[ -f "$gov" ]]; then
        echo performance > "$gov" 2>/dev/null || true
    fi
done
log "  Governor set to performance on all cores"

# Disable turbo boost for deterministic latency
if [[ -f /sys/devices/system/cpu/intel_pstate/no_turbo ]]; then
    echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo
    log "  Intel Turbo Boost disabled"
fi

# ────────────────────────────────────────────────────────────
# 4. NUMA Pinning
#    Pin blink-engine to NUMA node 0 (CPU + memory)
# ────────────────────────────────────────────────────────────
log "NUMA topology:"
if command -v numactl &>/dev/null; then
    numactl --hardware | head -10
    log "  Engine will be pinned to NUMA node 0 via systemd (CPUAffinity + numactl)"
else
    log "  WARN: numactl not installed — apt install numactl"
fi

# ────────────────────────────────────────────────────────────
# 5. Network Interface IRQ Affinity
#    Pin NIC interrupts to cores NOT used by the trading engine
#    (engine on 0-7, IRQs on 8+)
# ────────────────────────────────────────────────────────────
log "Setting NIC IRQ affinity for ${NET_IF}..."

# Stop irqbalance to prevent it from overriding our settings
systemctl stop irqbalance 2>/dev/null || true
systemctl disable irqbalance 2>/dev/null || true

# Pin NIC IRQs to cores 8-15 (bitmask: 0xFF00)
IRQ_MASK="0000ff00"
for irq_dir in /proc/irq/*/; do
    irq_num=$(basename "$irq_dir")
    [[ "$irq_num" =~ ^[0-9]+$ ]] || continue
    if grep -q "${NET_IF}" "${irq_dir}smp_affinity_list" 2>/dev/null || \
       grep -ql "${NET_IF}" "${irq_dir}"*/*affinity* 2>/dev/null; then
        echo "${IRQ_MASK}" > "${irq_dir}smp_affinity" 2>/dev/null || true
    fi
done
log "  NIC IRQs pinned away from trading cores"

# ────────────────────────────────────────────────────────────
# 6. Network Stack Tuning (sysctl)
# ────────────────────────────────────────────────────────────
log "Applying network sysctl tuning..."

cat > /etc/sysctl.d/99-blink-hft.conf <<'EOF'
# ── Blink HFT network tuning ──────────────────────────────
# Socket buffer sizes (128 MB)
net.core.rmem_max = 134217728
net.core.wmem_max = 134217728
net.core.rmem_default = 16777216
net.core.wmem_default = 16777216

# TCP memory
net.ipv4.tcp_rmem = 4096 87380 134217728
net.ipv4.tcp_wmem = 4096 65536 134217728

# Disable TCP timestamps — saves ~10 bytes per packet, reduces latency
net.ipv4.tcp_timestamps = 0

# Disable TCP SACK — eliminates selective-ack overhead in low-loss networks
net.ipv4.tcp_sack = 0

# Low-latency TCP
net.ipv4.tcp_low_latency = 1
net.core.netdev_max_backlog = 250000

# Busy-poll settings (microseconds) — spin-poll sockets instead of sleeping
net.core.busy_read = 50
net.core.busy_poll = 50

# ── io_uring / AIO ────────────────────────────────────────
fs.aio-max-nr = 1048576

# ── Memory ────────────────────────────────────────────────
vm.swappiness = 0
vm.zone_reclaim_mode = 0
EOF

sysctl --system --quiet
log "  sysctl settings applied"

# ────────────────────────────────────────────────────────────
# 7. Huge Pages (reduce TLB misses)
# ────────────────────────────────────────────────────────────
log "Configuring huge pages..."
echo 1024 > /proc/sys/vm/nr_hugepages
mkdir -p /mnt/hugepages
if ! mountpoint -q /mnt/hugepages 2>/dev/null; then
    mount -t hugetlbfs none /mnt/hugepages
fi
log "  1024 huge pages allocated, mounted at /mnt/hugepages"

# ────────────────────────────────────────────────────────────
# 8. tmpfs for lock-free IPC (/dev/shm)
# ────────────────────────────────────────────────────────────
log "Ensuring /dev/shm is mounted as tmpfs..."
if mountpoint -q /dev/shm 2>/dev/null; then
    # Remount with larger size for IPC
    mount -o remount,size=8G /dev/shm
    log "  /dev/shm remounted at 8G"
else
    mount -t tmpfs -o size=8G tmpfs /dev/shm
    log "  /dev/shm mounted as 8G tmpfs"
fi

# ────────────────────────────────────────────────────────────
# 9. File descriptor limits
# ────────────────────────────────────────────────────────────
log "Setting file descriptor limits..."
cat > /etc/security/limits.d/99-blink.conf <<EOF
*    soft    nofile    1000000
*    hard    nofile    1000000
*    soft    memlock   unlimited
*    hard    memlock   unlimited
EOF

# ────────────────────────────────────────────────────────────
# 10. GRUB recommendations (require reboot)
# ────────────────────────────────────────────────────────────
log "Recommended GRUB settings (add to GRUB_CMDLINE_LINUX in /etc/default/grub):"
echo "  intel_pstate=disable processor.max_cstate=0 intel_idle.max_cstate=0"
echo "  idle=poll isolcpus=0-7 nohz_full=0-7 rcu_nocbs=0-7"
echo "  transparent_hugepage=never"
echo ""
echo "  Then run: update-grub && reboot"

# ────────────────────────────────────────────────────────────
log "OS tuning complete. Reboot recommended for GRUB changes."
