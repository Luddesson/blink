# bpf-probes — eBPF Kernel Telemetry for Blink Engine

Kernel-level latency measurement via eBPF tracepoints for the Blink HFT engine.

## Probes

| Probe | Tracepoint | What it measures |
|-------|-----------|------------------|
| `tcp_rtt` | `tp/tcp/tcp_rcv_established` | TCP RTT to Polymarket CDN (104.18.0.0/16) |
| `sched_latency` | `tp/sched/sched_wakeup` + `sched_switch` | Wakeup-to-run latency for engine PID |
| `syscall_profile` | `raw_tp/sys_enter` + `sys_exit` | send/recv/epoll_wait syscall duration |

## Requirements

### Linux production
- Kernel ≥ 5.8 (BPF ring buffer support)
- BTF enabled: `CONFIG_DEBUG_INFO_BTF=y`
- Capabilities: `CAP_BPF` + `CAP_PERFMON`

```bash
# Grant capabilities to the binary:
sudo setcap cap_bpf,cap_perfmon=eip ./blink-engine

# Or run as root (not recommended for production).
```

### Windows / macOS
The crate compiles but provides no-op stubs. TUI displays "N/A" for kernel metrics.

## Building BPF Programs

### 1. Generate vmlinux.h (one-time, on target kernel)

```bash
bpftool btf dump file /sys/kernel/btf/vmlinux format c > bpf/vmlinux.h
```

### 2. Compile BPF object files (CO-RE)

```bash
CLANG_FLAGS="-O2 -g -target bpf -D__TARGET_ARCH_x86"

clang $CLANG_FLAGS -I bpf/ -c bpf/tcp_rtt.bpf.c -o bpf/tcp_rtt.bpf.o
clang $CLANG_FLAGS -I bpf/ -c bpf/sched_latency.bpf.c -o bpf/sched_latency.bpf.o
clang $CLANG_FLAGS -I bpf/ -c bpf/syscall_profile.bpf.c -o bpf/syscall_profile.bpf.o
```

### 3. Deploy object files

```bash
sudo mkdir -p /opt/blink/bpf
sudo cp bpf/*.bpf.o /opt/blink/bpf/
```

## Enabling in the engine

```bash
# Build with eBPF feature (Linux only):
cargo build --release --features ebpf-telemetry

# Set environment variable to activate:
EBPF_TELEMETRY=true cargo run --release --features ebpf-telemetry
```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                   Userspace (Rust)                   │
│                                                     │
│  BpfTelemetry                                       │
│  ├── loads tcp_rtt.bpf.o, sched_latency.bpf.o,     │
│  │   syscall_profile.bpf.o via aya                  │
│  ├── attaches to kernel tracepoints                 │
│  ├── spawns ring buffer polling tasks (tokio)       │
│  └── updates Arc<Mutex<KernelSnapshot>> every 250ms │
│                                                     │
│  TUI (tui_app.rs)                                   │
│  └── reads KernelSnapshot for kernel latency panel  │
├─────────────────────────────────────────────────────┤
│                   Kernel (BPF)                       │
│                                                     │
│  tcp_rtt.bpf.o ─→ ring buffer ─→ RttStats          │
│  sched_latency.bpf.o ─→ ring buffer ─→ SchedStats  │
│  syscall_profile.bpf.o ─→ ring buffer ─→ SyscallStats│
└─────────────────────────────────────────────────────┘
```

## Performance

- Ring buffer (not perf events) for >100k events/sec throughput
- BPF programs pass verifier with <1M instructions
- Polling interval: 10ms (configurable)
- Snapshot update: 250ms (for TUI rendering)
- Minimal overhead: BPF programs run inline in kernel context
