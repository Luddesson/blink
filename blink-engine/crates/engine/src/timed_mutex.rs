//! Thin `std::sync::Mutex` wrapper that emits `tracing::warn!` when a guard
//! is held longer than [`WARN_THRESHOLD`].
//!
//! Intended for the engine's sync (non-async) mutexes.  A guard held >100 ms
//! on the Tokio executor thread stalls the entire async runtime; the warning
//! surfaces this before it becomes a deadlock or a latency regression.
//!
//! # Drop-in replacement
//!
//! ```
//! use engine::timed_mutex::TimedMutex;
//!
//! let m = TimedMutex::new("my_lock", 0u32);
//! *m.lock_or_recover() = 42;
//! assert_eq!(*m.lock_or_recover(), 42);
//! ```

use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Emit a warning when a guard is held longer than this.
const WARN_THRESHOLD: Duration = Duration::from_millis(100);

// ─── TimedMutex ──────────────────────────────────────────────────────────────

/// A `std::sync::Mutex<T>` wrapper that warns when its guard is held too long.
pub struct TimedMutex<T> {
    inner: Mutex<T>,
    name: &'static str,
}

impl<T> TimedMutex<T> {
    pub fn new(name: &'static str, value: T) -> Self {
        Self {
            inner: Mutex::new(value),
            name,
        }
    }

    /// Acquire the lock, recovering from poison.
    ///
    /// If a thread panicked while holding the lock, the poisoned guard is
    /// recovered rather than propagating a panic.  The inner value may be
    /// partially mutated but the engine continues rather than cascading.
    pub fn lock_or_recover(&self) -> TimedMutexGuard<'_, T> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        TimedMutexGuard {
            inner: guard,
            acquired_at: Instant::now(),
            name: self.name,
        }
    }

    /// Acquire the lock, propagating poison as `Err`.
    pub fn lock(
        &self,
    ) -> Result<TimedMutexGuard<'_, T>, std::sync::PoisonError<MutexGuard<'_, T>>> {
        self.inner.lock().map(|guard| TimedMutexGuard {
            inner: guard,
            acquired_at: Instant::now(),
            name: self.name,
        })
    }

    pub fn into_inner(self) -> Result<T, std::sync::PoisonError<T>> {
        self.inner.into_inner()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for TimedMutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TimedMutex({:?}, name={:?})", self.inner, self.name)
    }
}

// ─── TimedMutexGuard ─────────────────────────────────────────────────────────

/// Guard returned by [`TimedMutex::lock_or_recover`] / [`TimedMutex::lock`].
///
/// Dereferences transparently to `T`. On drop, emits `tracing::warn!` if the
/// guard was held longer than [`WARN_THRESHOLD`].
pub struct TimedMutexGuard<'a, T> {
    inner: MutexGuard<'a, T>,
    acquired_at: Instant,
    name: &'static str,
}

impl<T> Deref for TimedMutexGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for TimedMutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T> Drop for TimedMutexGuard<'_, T> {
    fn drop(&mut self) {
        let held = self.acquired_at.elapsed();
        if held > WARN_THRESHOLD {
            tracing::warn!(
                mutex = self.name,
                held_ms = held.as_millis(),
                "Sync mutex held >{}ms — potential executor stall or deadlock risk",
                WARN_THRESHOLD.as_millis(),
            );
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn basic_lock_and_mutate() {
        let m = TimedMutex::new("test", 0u32);
        *m.lock_or_recover() = 42;
        assert_eq!(*m.lock_or_recover(), 42);
    }

    #[test]
    fn poison_recovery() {
        let m = Arc::new(TimedMutex::new("poison_test", 0u32));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock_or_recover();
            panic!("intentional poison");
        })
        .join();
        // Should recover from poison without a secondary panic.
        *m.lock_or_recover() = 99;
        assert_eq!(*m.lock_or_recover(), 99);
    }

    #[test]
    fn warn_threshold_constant() {
        assert_eq!(WARN_THRESHOLD, Duration::from_millis(100));
    }
}
