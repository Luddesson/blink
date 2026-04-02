//! No-op BPF telemetry stub for unsupported platforms.
//!
//! Active on Windows, macOS, or Linux without the `ebpf-telemetry` feature.
//! Returns default (zeroed) stats and reports as unavailable. The TUI renders
//! "N/A" when `is_available() == false`.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::warn;

use crate::stats::{KernelSnapshot, RttStats, SchedStats, SyscallStats};

/// No-op BPF telemetry for unsupported platforms.
pub struct BpfTelemetry {
    snapshot: Arc<Mutex<KernelSnapshot>>,
}

impl BpfTelemetry {
    /// Creates a stub telemetry instance. Logs a warning that eBPF is unavailable.
    pub async fn attach(_pid: u32) -> Result<Self> {
        warn!("⚠️  eBPF telemetry not available — platform unsupported or feature disabled");
        Ok(Self {
            snapshot: Arc::new(Mutex::new(KernelSnapshot::default())),
        })
    }

    pub fn rtt_snapshot(&self) -> RttStats {
        RttStats::default()
    }

    pub fn sched_snapshot(&self) -> SchedStats {
        SchedStats::default()
    }

    pub fn syscall_snapshot(&self) -> SyscallStats {
        SyscallStats::default()
    }

    /// Returns the combined kernel telemetry snapshot.
    /// On stub platforms, `available` is always `false`.
    pub fn kernel_snapshot(&self) -> KernelSnapshot {
        KernelSnapshot::default()
    }

    /// Returns a shared handle to the snapshot for TUI integration.
    pub fn snapshot_handle(&self) -> Arc<Mutex<KernelSnapshot>> {
        Arc::clone(&self.snapshot)
    }

    pub fn is_available(&self) -> bool {
        false
    }

    pub fn detach(self) {
        // No-op — nothing to clean up.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn graceful_degradation_on_windows() {
        let telemetry = BpfTelemetry::attach(1234).await.unwrap();
        assert!(!telemetry.is_available());

        let snap = telemetry.kernel_snapshot();
        assert!(!snap.available);
        assert_eq!(snap.rtt.samples, 0);
        assert_eq!(snap.sched.samples, 0);
        assert_eq!(snap.syscall.samples, 0);

        telemetry.detach();
    }
}
