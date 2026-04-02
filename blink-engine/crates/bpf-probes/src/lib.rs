//! eBPF Kernel Telemetry for the Blink HFT Engine.
//!
//! Provides kernel-level latency metrics via eBPF tracepoints:
//! - **TCP RTT**: Round-trip time for Polymarket CLOB connections
//!   (`tracepoint/tcp/tcp_rcv_established`, filtered to 104.18.0.0/16)
//! - **Scheduler jitter**: Wakeup-to-run latency for the engine process
//!   (`tracepoint/sched/sched_switch` + `sched_wakeup`)
//! - **Syscall profiling**: Time in `send()` / `recv()` / `epoll_wait()`
//!   (`raw_tracepoint/sys_enter` + `sys_exit`)
//!
//! # Platform support
//!
//! | Platform | Feature               | Behavior                     |
//! |----------|-----------------------|------------------------------|
//! | Linux    | `ebpf-telemetry`      | Full kernel telemetry        |
//! | Linux    | (default)             | No-op stub                   |
//! | Windows  | any                   | No-op stub, TUI shows "N/A"  |
//! | macOS    | any                   | No-op stub, TUI shows "N/A"  |
//!
//! # Requirements (Linux with feature enabled)
//!
//! - Linux kernel ≥ 5.8 (BPF ring buffer support)
//! - `CAP_BPF` + `CAP_PERFMON` capabilities
//! - BTF enabled kernel (`CONFIG_DEBUG_INFO_BTF=y`)
//! - Pre-compiled BPF object files in `bpf/` directory

pub mod stats;
pub mod latency_probe;

#[cfg(all(target_os = "linux", feature = "ebpf-telemetry"))]
mod linux_impl;

#[cfg(all(target_os = "linux", feature = "ebpf-telemetry"))]
pub use linux_impl::BpfTelemetry;

#[cfg(not(all(target_os = "linux", feature = "ebpf-telemetry")))]
mod stub_impl;

#[cfg(not(all(target_os = "linux", feature = "ebpf-telemetry")))]
pub use stub_impl::BpfTelemetry;

pub use stats::{KernelSnapshot, RttStats, SchedStats, SyscallHistogram, SyscallStats};
pub use latency_probe::{
    EbpfProbe, LatencyProbe, LoggingProbe, NullProbe,
    LATENCY_ALERT_THRESHOLD_US,
};
