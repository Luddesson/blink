//! Process-global kill switch. Engaged → every breaker rejects.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Process-global kill switch. Cheap to clone (Arc-ed). Cheap to poll
/// (relaxed atomic load).
#[derive(Debug, Clone, Default)]
pub struct KillSwitch {
    inner: Arc<KillSwitchInner>,
}

#[derive(Debug)]
struct KillSwitchInner {
    engaged: AtomicBool,
    /// Monotonic generation counter; bumps on every `engage`. Lets breakers
    /// treat repeat engages as fresh events even for the same operator.
    gen: AtomicU64,
}

impl Default for KillSwitchInner {
    fn default() -> Self {
        Self {
            engaged: AtomicBool::new(false),
            gen: AtomicU64::new(0),
        }
    }
}

impl KillSwitch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Engage the switch. All subsequent `admit` calls will reject.
    /// `operator` identifies who flipped it (string lives for the process).
    pub fn engage(&self, _operator: &'static str) {
        self.inner.gen.fetch_add(1, Ordering::AcqRel);
        self.inner.engaged.store(true, Ordering::Release);
    }

    /// Disengage the switch.
    pub fn disengage(&self) {
        self.inner.engaged.store(false, Ordering::Release);
    }

    /// Is the switch currently engaged? Relaxed load — OK on hot path.
    #[inline]
    pub fn is_engaged(&self) -> bool {
        self.inner.engaged.load(Ordering::Acquire)
    }

    /// Generation of the last engage call. Useful for logging only.
    pub fn generation(&self) -> u64 {
        self.inner.gen.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engage_disengage() {
        let k = KillSwitch::new();
        assert!(!k.is_engaged());
        k.engage("test");
        assert!(k.is_engaged());
        k.disengage();
        assert!(!k.is_engaged());
    }

    #[test]
    fn engage_bumps_generation() {
        let k = KillSwitch::new();
        let g0 = k.generation();
        k.engage("a");
        k.engage("b");
        assert_eq!(k.generation(), g0 + 2);
    }
}
