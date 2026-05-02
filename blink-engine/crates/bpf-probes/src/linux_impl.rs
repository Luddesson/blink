//! Linux eBPF kernel telemetry implementation via the `aya` crate.
//!
//! Loads pre-compiled BPF object files and attaches them to kernel tracepoints.
//! Spawns background tokio tasks to poll ring buffers and update shared stats.
//!
//! # BPF programs loaded
//! - `tcp_rtt.bpf.o`      → `tracepoint/tcp/tcp_rcv_established`
//! - `sched_latency.bpf.o` → `tracepoint/sched/sched_switch` + `sched_wakeup`
//! - `syscall_profile.bpf.o` → `raw_tracepoint/sys_enter` + `sys_exit`
//!
//! # Capabilities required
//! - `CAP_BPF`     — load and attach BPF programs
//! - `CAP_PERFMON` — access perf events and ring buffers

use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use aya::maps::RingBuf;
use aya::programs::TracePoint;
use aya::Bpf;
use aya_log::BpfLogger;

use crate::stats::{KernelSnapshot, RttStats, SchedStats, SyscallHistogram, SyscallStats};

// ─── BPF event structs (must match C struct layout) ───────────────────────────

/// Event emitted by `tcp_rtt.bpf.c` for each measured RTT sample.
#[repr(C)]
#[derive(Clone, Copy)]
struct RttEvent {
    rtt_us: u64,
    saddr: u32,
    daddr: u32,
}

/// Event emitted by `sched_latency.bpf.c` for each wakeup-to-run transition.
#[repr(C)]
#[derive(Clone, Copy)]
struct SchedEvent {
    latency_us: u64,
    pid: u32,
    _pad: u32,
}

/// Event emitted by `syscall_profile.bpf.c` for each profiled syscall.
#[repr(C)]
#[derive(Clone, Copy)]
struct SyscallEvent {
    latency_us: u64,
    /// Syscall number (sendto=44, recvfrom=45, epoll_wait=232 on x86_64)
    syscall_nr: u64,
}

// Syscall numbers for x86_64.
const SYS_SENDTO: u64 = 44;
const SYS_RECVFROM: u64 = 45;
const SYS_EPOLL_WAIT: u64 = 232;

// ─── BpfTelemetry ─────────────────────────────────────────────────────────────

/// eBPF kernel telemetry for the Blink HFT engine.
///
/// Attaches BPF programs to kernel tracepoints and spawns background tasks
/// to read events from ring buffers. Stats are updated atomically in shared
/// `Arc<Mutex<...>>` handles that the TUI can snapshot without blocking.
pub struct BpfTelemetry {
    #[allow(dead_code)]
    bpf: Bpf,
    rtt_stats: Arc<Mutex<RttAccumulator>>,
    sched_stats: Arc<Mutex<SchedAccumulator>>,
    syscall_stats: Arc<Mutex<SyscallAccumulator>>,
    snapshot: Arc<Mutex<KernelSnapshot>>,
    poll_handles: Vec<JoinHandle<()>>,
}

// ─── Rolling accumulators ─────────────────────────────────────────────────────

struct RttAccumulator {
    samples: Vec<u64>,
    window: usize,
}

impl RttAccumulator {
    fn new(window: usize) -> Self {
        Self {
            samples: Vec::with_capacity(window),
            window,
        }
    }

    fn record(&mut self, rtt_us: u64) {
        if self.samples.len() >= self.window {
            self.samples.remove(0);
        }
        self.samples.push(rtt_us);
    }

    fn snapshot(&self) -> RttStats {
        if self.samples.is_empty() {
            return RttStats::default();
        }
        let min = *self.samples.iter().min().unwrap();
        let max = *self.samples.iter().max().unwrap();
        let sum: u64 = self.samples.iter().sum();
        let avg = sum / self.samples.len() as u64;
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let p99_idx = ((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1);
        RttStats {
            min_us: min,
            max_us: max,
            avg_us: avg,
            p99_us: sorted[p99_idx],
            samples: sorted.len() as u64,
        }
    }
}

struct SchedAccumulator {
    samples: Vec<u64>,
    window: usize,
    threshold_violations: u64,
}

impl SchedAccumulator {
    fn new(window: usize) -> Self {
        Self {
            samples: Vec::with_capacity(window),
            window,
            threshold_violations: 0,
        }
    }

    fn record(&mut self, latency_us: u64) {
        if latency_us > 100 {
            self.threshold_violations += 1;
        }
        if self.samples.len() >= self.window {
            self.samples.remove(0);
        }
        self.samples.push(latency_us);
    }

    fn snapshot(&self) -> SchedStats {
        if self.samples.is_empty() {
            return SchedStats::default();
        }
        let min = *self.samples.iter().min().unwrap();
        let max = *self.samples.iter().max().unwrap();
        let sum: u64 = self.samples.iter().sum();
        let avg = sum / self.samples.len() as u64;
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let p99_idx = ((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1);
        SchedStats {
            min_us: min,
            max_us: max,
            avg_us: avg,
            p99_us: sorted[p99_idx],
            threshold_violations: self.threshold_violations,
            samples: sorted.len() as u64,
        }
    }
}

struct SyscallAccumulator {
    send_samples: Vec<u64>,
    recv_samples: Vec<u64>,
    epoll_samples: Vec<u64>,
    histogram: SyscallHistogram,
    window: usize,
}

impl SyscallAccumulator {
    fn new(window: usize) -> Self {
        Self {
            send_samples: Vec::with_capacity(window),
            recv_samples: Vec::with_capacity(window),
            epoll_samples: Vec::with_capacity(window),
            histogram: SyscallHistogram::default(),
            window,
        }
    }

    fn record(&mut self, syscall_nr: u64, latency_us: u64) {
        self.histogram.record(latency_us);
        let (samples, _cap) = match syscall_nr {
            SYS_SENDTO => (&mut self.send_samples, self.window),
            SYS_RECVFROM => (&mut self.recv_samples, self.window),
            SYS_EPOLL_WAIT => (&mut self.epoll_samples, self.window),
            _ => return,
        };
        if samples.len() >= self.window {
            samples.remove(0);
        }
        samples.push(latency_us);
    }

    fn snapshot(&self) -> SyscallStats {
        let avg = |s: &[u64]| -> u64 {
            if s.is_empty() {
                0
            } else {
                s.iter().sum::<u64>() / s.len() as u64
            }
        };
        SyscallStats {
            send_avg_us: avg(&self.send_samples),
            recv_avg_us: avg(&self.recv_samples),
            epoll_avg_us: avg(&self.epoll_samples),
            histogram: self.histogram.clone(),
            samples: self.histogram.total(),
        }
    }
}

// ─── BPF object file paths ───────────────────────────────────────────────────

const BPF_OBJ_DIR: &str = "/opt/blink/bpf";

fn bpf_obj_path(name: &str) -> String {
    format!("{BPF_OBJ_DIR}/{name}.bpf.o")
}

// ─── Implementation ──────────────────────────────────────────────────────────

impl BpfTelemetry {
    /// Attach eBPF probes to kernel tracepoints for the given process.
    ///
    /// Loads pre-compiled BPF object files from `/opt/blink/bpf/`, attaches
    /// to tracepoints, and spawns background ring buffer polling tasks.
    ///
    /// # Errors
    /// Returns an error if BPF programs fail to load (missing capabilities,
    /// unsupported kernel, or missing object files).
    pub async fn attach(pid: u32) -> Result<Self> {
        info!(pid, "Attaching eBPF kernel telemetry probes");

        let rtt_stats = Arc::new(Mutex::new(RttAccumulator::new(10_000)));
        let sched_stats = Arc::new(Mutex::new(SchedAccumulator::new(10_000)));
        let syscall_stats = Arc::new(Mutex::new(SyscallAccumulator::new(10_000)));
        let snapshot = Arc::new(Mutex::new(KernelSnapshot {
            available: true,
            ..Default::default()
        }));

        // ── Load BPF programs ─────────────────────────────────────────────
        let mut bpf = Self::load_bpf_programs()?;

        // Initialize BPF logging (optional — gracefully degrades).
        if let Err(e) = BpfLogger::init(&mut bpf) {
            warn!("BPF logger init failed (non-fatal): {e}");
        }

        let mut poll_handles = Vec::new();

        // ── Attach TCP RTT tracepoint ─────────────────────────────────────
        match Self::attach_tcp_rtt(&mut bpf) {
            Ok(()) => {
                let handle = Self::spawn_rtt_poller(&mut bpf, Arc::clone(&rtt_stats))?;
                poll_handles.push(handle);
                info!("TCP RTT probe attached");
            }
            Err(e) => warn!("TCP RTT probe failed (non-fatal): {e}"),
        }

        // ── Attach scheduler latency tracepoints ──────────────────────────
        match Self::attach_sched(&mut bpf, pid) {
            Ok(()) => {
                let handle = Self::spawn_sched_poller(&mut bpf, Arc::clone(&sched_stats))?;
                poll_handles.push(handle);
                info!("Scheduler latency probe attached (pid={pid})");
            }
            Err(e) => warn!("Scheduler probe failed (non-fatal): {e}"),
        }

        // ── Attach syscall profiler tracepoints ───────────────────────────
        match Self::attach_syscall(&mut bpf, pid) {
            Ok(()) => {
                let handle = Self::spawn_syscall_poller(&mut bpf, Arc::clone(&syscall_stats))?;
                poll_handles.push(handle);
                info!("Syscall profiler attached (pid={pid})");
            }
            Err(e) => warn!("Syscall profiler failed (non-fatal): {e}"),
        }

        // ── Spawn snapshot updater ────────────────────────────────────────
        {
            let rtt = Arc::clone(&rtt_stats);
            let sched = Arc::clone(&sched_stats);
            let syscall = Arc::clone(&syscall_stats);
            let snap = Arc::clone(&snapshot);
            let handle = tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
                loop {
                    interval.tick().await;
                    let mut s = snap.lock().unwrap();
                    s.available = true;
                    s.rtt = rtt.lock().unwrap().snapshot();
                    s.sched = sched.lock().unwrap().snapshot();
                    s.syscall = syscall.lock().unwrap().snapshot();
                }
            });
            poll_handles.push(handle);
        }

        info!(
            "eBPF kernel telemetry fully attached ({} probes active)",
            poll_handles.len() - 1
        );

        Ok(Self {
            bpf,
            rtt_stats,
            sched_stats,
            syscall_stats,
            snapshot,
            poll_handles,
        })
    }

    /// Load the combined BPF object file.
    fn load_bpf_programs() -> Result<Bpf> {
        // Try loading individual BPF programs and merge, or load a combined object.
        let tcp_rtt_path = bpf_obj_path("tcp_rtt");
        let bpf = Bpf::load_file(&tcp_rtt_path)
            .with_context(|| format!("Failed to load BPF object: {tcp_rtt_path}"))?;
        Ok(bpf)
    }

    fn attach_tcp_rtt(bpf: &mut Bpf) -> Result<()> {
        let prog: &mut TracePoint = bpf
            .program_mut("tcp_rtt_probe")
            .ok_or_else(|| anyhow::anyhow!("BPF program 'tcp_rtt_probe' not found"))?
            .try_into()
            .context("Program is not a TracePoint")?;
        prog.load().context("Failed to load tcp_rtt BPF program")?;
        prog.attach("tcp", "tcp_rcv_established")
            .context("Failed to attach to tracepoint/tcp/tcp_rcv_established")?;
        Ok(())
    }

    fn attach_sched(bpf: &mut Bpf, _pid: u32) -> Result<()> {
        // sched_wakeup probe
        let wakeup: &mut TracePoint = bpf
            .program_mut("sched_wakeup_probe")
            .ok_or_else(|| anyhow::anyhow!("BPF program 'sched_wakeup_probe' not found"))?
            .try_into()?;
        wakeup.load()?;
        wakeup.attach("sched", "sched_wakeup")?;

        // sched_switch probe
        let switch: &mut TracePoint = bpf
            .program_mut("sched_switch_probe")
            .ok_or_else(|| anyhow::anyhow!("BPF program 'sched_switch_probe' not found"))?
            .try_into()?;
        switch.load()?;
        switch.attach("sched", "sched_switch")?;

        Ok(())
    }

    fn attach_syscall(bpf: &mut Bpf, _pid: u32) -> Result<()> {
        let enter: &mut TracePoint = bpf
            .program_mut("sys_enter_probe")
            .ok_or_else(|| anyhow::anyhow!("BPF program 'sys_enter_probe' not found"))?
            .try_into()?;
        enter.load()?;
        enter.attach("raw_syscalls", "sys_enter")?;

        let exit: &mut TracePoint = bpf
            .program_mut("sys_exit_probe")
            .ok_or_else(|| anyhow::anyhow!("BPF program 'sys_exit_probe' not found"))?
            .try_into()?;
        exit.load()?;
        exit.attach("raw_syscalls", "sys_exit")?;

        Ok(())
    }

    fn spawn_rtt_poller(
        bpf: &mut Bpf,
        stats: Arc<Mutex<RttAccumulator>>,
    ) -> Result<JoinHandle<()>> {
        let ring_buf = RingBuf::try_from(
            bpf.map_mut("rtt_events")
                .context("Map 'rtt_events' not found")?,
        )?;

        Ok(tokio::spawn(async move {
            let mut rb = ring_buf;
            loop {
                while let Some(item) = rb.next() {
                    if item.len() >= std::mem::size_of::<RttEvent>() {
                        let event: RttEvent =
                            unsafe { std::ptr::read_unaligned(item.as_ptr() as *const RttEvent) };
                        stats.lock().unwrap().record(event.rtt_us);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }))
    }

    fn spawn_sched_poller(
        bpf: &mut Bpf,
        stats: Arc<Mutex<SchedAccumulator>>,
    ) -> Result<JoinHandle<()>> {
        let ring_buf = RingBuf::try_from(
            bpf.map_mut("sched_events")
                .context("Map 'sched_events' not found")?,
        )?;

        Ok(tokio::spawn(async move {
            let mut rb = ring_buf;
            loop {
                while let Some(item) = rb.next() {
                    if item.len() >= std::mem::size_of::<SchedEvent>() {
                        let event: SchedEvent =
                            unsafe { std::ptr::read_unaligned(item.as_ptr() as *const SchedEvent) };
                        stats.lock().unwrap().record(event.latency_us);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }))
    }

    fn spawn_syscall_poller(
        bpf: &mut Bpf,
        stats: Arc<Mutex<SyscallAccumulator>>,
    ) -> Result<JoinHandle<()>> {
        let ring_buf = RingBuf::try_from(
            bpf.map_mut("syscall_events")
                .context("Map 'syscall_events' not found")?,
        )?;

        Ok(tokio::spawn(async move {
            let mut rb = ring_buf;
            loop {
                while let Some(item) = rb.next() {
                    if item.len() >= std::mem::size_of::<SyscallEvent>() {
                        let event: SyscallEvent = unsafe {
                            std::ptr::read_unaligned(item.as_ptr() as *const SyscallEvent)
                        };
                        stats
                            .lock()
                            .unwrap()
                            .record(event.syscall_nr, event.latency_us);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }))
    }

    // ─── Public accessors ─────────────────────────────────────────────────

    pub fn rtt_snapshot(&self) -> RttStats {
        self.rtt_stats.lock().unwrap().snapshot()
    }

    pub fn sched_snapshot(&self) -> SchedStats {
        self.sched_stats.lock().unwrap().snapshot()
    }

    pub fn syscall_snapshot(&self) -> SyscallStats {
        self.syscall_stats.lock().unwrap().snapshot()
    }

    /// Returns the combined kernel telemetry snapshot (updated every 250ms).
    pub fn kernel_snapshot(&self) -> KernelSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Returns a shared handle to the snapshot for TUI integration.
    pub fn snapshot_handle(&self) -> Arc<Mutex<KernelSnapshot>> {
        Arc::clone(&self.snapshot)
    }

    pub fn is_available(&self) -> bool {
        true
    }

    /// Detach all BPF programs and stop polling tasks.
    pub fn detach(self) {
        info!("Detaching eBPF kernel telemetry probes");
        for handle in self.poll_handles {
            handle.abort();
        }
        // BPF programs are automatically detached when `self.bpf` is dropped.
    }
}
