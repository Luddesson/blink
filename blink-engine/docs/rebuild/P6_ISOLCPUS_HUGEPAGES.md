# P6 — Kernel isolation + hugepages runbook

> Owner: platform/ops. Plan: §3 Phase 6.
> Preconditions: Phase 1 colo box (`blink-prod-01.use2`) available, root SSH, off-hours
> window (this requires a reboot).

This runbook hardens the prod host for deterministic low-latency operation:
1. `isolcpus` / `nohz_full` / `rcu_nocbs` — remove scheduler, timer, and RCU noise from
   the decision-critical cores.
2. Explicit 2 MiB hugepage reservation — back the arena allocators used by
   `blink-book`, `blink-kernel`, and `blink-ingress` ring buffers without relying on THP.
3. IRQ affinity — bind NIC RX/TX queues to the house-keeping cores only.
4. CPU frequency governor + c-state pinning — remove turbo-related jitter.

Applied together; they are cheap individually but only coherent as a set.

---

## 1. Target topology (AWS c6i.8xlarge — 16 physical cores, HT on)

| Cores | Role                                       | Kernel flags       |
|-------|--------------------------------------------|--------------------|
| 0–1   | OS, IRQ handling, Tokio cold pool          | (none — default)   |
| 2     | `blink-ingress`::MempoolSource             | isolcpus           |
| 3     | `blink-ingress`::ClobWsSource              | isolcpus           |
| 4     | `blink-kernel`::decide (single-threaded)   | isolcpus + nohz    |
| 5     | `blink-submit`::Submitter                  | isolcpus + nohz    |
| 6–7   | reserved for Phase-5 signal stages         | isolcpus           |
| 8–15  | housekeeping / HT siblings of above        | (leave to OS)      |

> Note on HT: core `N` and core `N+8` are hyperthread siblings on c6i. We deliberately
> do NOT run pinned threads on HT siblings — the siblings are left to the OS to absorb
> interrupts and housekeeping. Disabling HT entirely (`nosmt` cmdline) is an alternative
> we evaluate after Phase 1 measurement.

---

## 2. Kernel command-line

Edit `/etc/default/grub`, append to `GRUB_CMDLINE_LINUX`:

```
isolcpus=2-7 nohz_full=4-5 rcu_nocbs=4-5 \
intel_idle.max_cstate=1 processor.max_cstate=1 idle=poll \
hugepages=1024 default_hugepagesz=2M hugepagesz=2M \
transparent_hugepage=never mitigations=off \
skew_tick=1 tsc=reliable clocksource=tsc
```

Rationale:
- `isolcpus=2-7` — scheduler will not place normal tasks there. We pin ours via `sched_setaffinity`.
- `nohz_full=4-5` — kill the tick on the kernel/submit cores; they see only explicit
  work plus unavoidable IPIs.
- `rcu_nocbs=4-5` — offload RCU callbacks to the housekeeping cores.
- `intel_idle.max_cstate=1 processor.max_cstate=1 idle=poll` — kill c-state entry; wake
  latency becomes deterministic (at the cost of ~+30 W).
- `hugepages=1024 default_hugepagesz=2M` — reserve 2 GiB of 2 MiB pages at boot. Arena
  allocators use them via `mmap(..., MAP_HUGETLB, ...)`.
- `transparent_hugepage=never` — forbid THP. THP's khugepaged runs at random times and
  is a P99 tail source. We want explicit, not probabilistic.
- `mitigations=off` — this is a single-tenant HFT host with no untrusted code. Restores
  ~15 % on kernel-boundary-heavy workloads. **Risk**: accept; this is a trading host,
  not a general-compute host. Document in R5.
- `skew_tick=1` — stagger remaining timer ticks across cores to avoid lockstep.
- `tsc=reliable clocksource=tsc` — matches what `blink-timestamps` assumes.

Apply:
```bash
grub-mkconfig -o /boot/grub/grub.cfg
reboot
```

Verify after boot:
```bash
cat /sys/devices/system/cpu/isolated                 # → 2-7
cat /sys/devices/system/cpu/nohz_full                 # → 4-5
cat /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages   # → 1024
cat /sys/kernel/mm/transparent_hugepage/enabled       # → always [never]
cat /sys/devices/system/clocksource/clocksource0/current_clocksource  # → tsc
for i in 2 3 4 5 6 7; do cat /sys/devices/system/cpu/cpu$i/cpuidle/state*/disable; done
```

---

## 3. IRQ affinity

```bash
# 3a. Find NIC RX/TX queues
ls /sys/class/net/ens5/queues

# 3b. Build an affinity mask for cores 0-1 only (= 0x3)
for irq in $(grep -E 'ens5|nvme' /proc/interrupts | awk -F: '{print $1}' | tr -d ' '); do
  echo 3 > /proc/irq/$irq/smp_affinity
done

# 3c. Disable irqbalance (it will fight us)
systemctl disable --now irqbalance
```

Wire this as a oneshot systemd unit `/etc/systemd/system/blink-irq-affinity.service`
so it re-applies after reboot. (Template in `ops/systemd/` — TBD when file lands.)

---

## 4. Frequency governor + turbo

```bash
# performance governor on all cores
cpupower frequency-set -g performance

# disable turbo (determinism > peak)
echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo
```

Persist via `/etc/systemd/system/blink-cpu.service`.

---

## 5. Application-side pinning

`blink-rings::CoreAffinity::pin_current_to(core_id)` is already wired; each stage
thread calls it at startup. The prod configuration lives in
`/etc/blink/engine.toml`:

```toml
[affinity]
ingress_mempool = 2
ingress_clobws  = 3
kernel          = 4
submitter       = 5
signals_a       = 6
signals_b       = 7
```

Operator validates at startup via:
```bash
for tid in $(ls /proc/$(pgrep blink-engine)/task); do
  cat /proc/$(pgrep blink-engine)/task/$tid/status | grep -E 'Name|Cpus_allowed_list'
done
```

Each stage thread's `Cpus_allowed_list` must equal exactly its configured core.

---

## 6. Hugepage-backed arenas

`blink-kernel` and `blink-book` do not allocate on the hot path (verified by
`stats_alloc` tests), so hugepage backing is only relevant for the *warmup* arenas —
principally ring buffers in `blink-rings`. Implementation (future patch):

```rust
// crates/blink-rings/src/hugepage.rs  — TBD
let layout = Layout::from_size_align(cap, 2 * 1024 * 1024)?;
let ptr = unsafe {
    libc::mmap(null_mut(), cap,
               libc::PROT_READ | libc::PROT_WRITE,
               libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_HUGETLB,
               -1, 0)
};
```

Today's rtrb wrappers use the heap — fine for correctness. Upgrade after Phase-1
latency measurements tell us whether the TLB miss cost shows up.

---

## 7. Validation protocol (post-change)

1. `cyclictest -p 99 -t 6 -a 2-7 -i 200 -l 100000`
   — Max latency on isolated cores must be < 40 µs.
2. Run `blink-shadow --replay replays/24h.parquet --duration 1h` and compare
   p50/p99 against pre-change baseline. **No regression > 10 %.**
3. Prod canary: 1 h at 10 % traffic with SLO alerts armed.

---

## 8. Rollback

Every change in this runbook is reverted by removing the corresponding line and
rebooting (§2), or `systemctl stop blink-*.service && systemctl enable irqbalance`
(§3/§4). The application is unaffected — `CoreAffinity` already emits `Warn` if
CAP_SYS_NICE is absent.
