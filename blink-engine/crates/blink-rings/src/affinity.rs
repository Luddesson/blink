//! Core-affinity helper. Linux-only real implementation via
//! `sched_setaffinity`/`sched_getaffinity`; other targets get a graceful stub
//! returning [`AffinityError::NotPermitted`] so downstream code doesn't branch
//! on `cfg(target_os)`.

use thiserror::Error;

/// Errors returned by affinity operations. Engineered so callers can decide
/// between "log and continue" (common in CI containers without CAP_SYS_NICE)
/// and "fail hard" (production pinning contracts).
#[derive(Debug, Error)]
pub enum AffinityError {
    /// Kernel denied the request (EPERM, unsupported platform, or CPU set
    /// contains no valid cores).
    #[error("core affinity not permitted in this environment")]
    NotPermitted,
    /// The requested core id is >= the number of online CPUs.
    #[error("invalid core id {0}")]
    InvalidCore(usize),
    /// Generic OS failure surfacing errno.
    #[error("sched_setaffinity failed (errno={0})")]
    Os(i32),
}

/// Marker type grouping the public affinity helpers. Methods are free
/// functions; this is only here so `CoreAffinity::pin_current_to(…)` reads as
/// a namespaced call when callers prefer that.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoreAffinity;

impl CoreAffinity {
    /// See [`pin_current_to`].
    #[inline]
    pub fn pin_current_to(core_id: usize) -> Result<(), AffinityError> {
        pin_current_to(core_id)
    }

    /// See [`verify_pinned`].
    #[inline]
    pub fn verify_pinned() -> Option<usize> {
        verify_pinned()
    }

    /// See [`spawn_pinned`].
    #[inline]
    pub fn spawn_pinned<F>(core: usize, name: &str, f: F) -> std::thread::JoinHandle<()>
    where
        F: FnOnce() + Send + 'static,
    {
        spawn_pinned(core, name, f)
    }
}

/// Pin the current thread to `core_id`. Returns [`AffinityError::NotPermitted`]
/// on non-Linux targets or when the kernel denies the request (typical in
/// unprivileged containers). Does **not** panic.
#[cfg(target_os = "linux")]
pub fn pin_current_to(core_id: usize) -> Result<(), AffinityError> {
    // Sanity-check against online CPUs.
    let online = num_online_cpus();
    if core_id >= online {
        return Err(AffinityError::InvalidCore(core_id));
    }

    // SAFETY: `set` is an owned stack value correctly sized for the call.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(core_id, &mut set);
        let rc = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        if rc == 0 {
            Ok(())
        } else {
            let errno = *libc::__errno_location();
            if errno == libc::EPERM || errno == libc::EINVAL {
                log::warn!(
                    "pin_current_to({core_id}): sched_setaffinity denied (errno={errno}); running unpinned"
                );
                Err(AffinityError::NotPermitted)
            } else {
                Err(AffinityError::Os(errno))
            }
        }
    }
}

/// Non-Linux stub: pinning is not supported, return `NotPermitted`.
#[cfg(not(target_os = "linux"))]
pub fn pin_current_to(_core_id: usize) -> Result<(), AffinityError> {
    log::warn!("pin_current_to: core affinity not supported on this target");
    Err(AffinityError::NotPermitted)
}

/// Returns `Some(core)` iff the current thread's affinity mask contains
/// exactly one bit; otherwise `None` (no pinning, or pinned to a set).
#[cfg(target_os = "linux")]
pub fn verify_pinned() -> Option<usize> {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        let rc =
            libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set);
        if rc != 0 {
            return None;
        }
        let mut found: Option<usize> = None;
        let max = num_online_cpus().max(1);
        for cpu in 0..max {
            if libc::CPU_ISSET(cpu, &set) {
                if found.is_some() {
                    return None; // more than one bit set
                }
                found = Some(cpu);
            }
        }
        found
    }
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn verify_pinned() -> Option<usize> {
    None
}

/// Spawn a named, pinned thread. Pinning failures are logged and the thread
/// runs unpinned — tests in unprivileged CI containers depend on this fallback.
pub fn spawn_pinned<F>(core: usize, name: &str, f: F) -> std::thread::JoinHandle<()>
where
    F: FnOnce() + Send + 'static,
{
    let name_owned = name.to_owned();
    std::thread::Builder::new()
        .name(name_owned.clone())
        .spawn(move || {
            set_thread_name(&name_owned);
            if let Err(e) = pin_current_to(core) {
                log::warn!("spawn_pinned({core}, {name_owned:?}): {e}; running unpinned");
            }
            f();
        })
        .expect("failed to spawn thread")
}

/// Best-effort `pthread_setname_np`. Truncates to 15 bytes + NUL per Linux
/// kernel's TASK_COMM_LEN limit. Silently ignores failures.
#[cfg(target_os = "linux")]
fn set_thread_name(name: &str) {
    // Truncate to 15 bytes on a char boundary to stay valid UTF-8 (not that
    // pthread cares, but avoids slicing surprises).
    let mut cut = name.len().min(15);
    while cut > 0 && !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let truncated = &name[..cut];
    let mut buf = [0u8; 16];
    let bytes = truncated.as_bytes();
    buf[..bytes.len()].copy_from_slice(bytes);
    // Final NUL already present from zero-init.
    unsafe {
        let _ = libc::pthread_setname_np(libc::pthread_self(), buf.as_ptr() as *const _);
    }
}

#[cfg(not(target_os = "linux"))]
fn set_thread_name(_name: &str) {}

#[cfg(target_os = "linux")]
fn num_online_cpus() -> usize {
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if n < 1 { 1 } else { n as usize }
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn num_online_cpus() -> usize {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_current_to_is_graceful() {
        // Must not panic regardless of platform / privilege. Accept any
        // Ok/Err outcome — CI containers typically return NotPermitted.
        let res = pin_current_to(0);
        match res {
            Ok(()) => {
                // If we did pin, verify_pinned should agree (or at least not
                // panic). We don't assert a specific core because the kernel
                // may have collapsed the request.
                let _ = verify_pinned();
            }
            Err(AffinityError::NotPermitted) | Err(AffinityError::Os(_))
            | Err(AffinityError::InvalidCore(_)) => {}
        }
    }

    #[test]
    fn spawn_pinned_runs_closure_even_if_pin_fails() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        let h = spawn_pinned(0, "blink-rings-t", move || {
            f.store(true, Ordering::SeqCst);
        });
        h.join().unwrap();
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn spawn_pinned_truncates_long_names() {
        // Name deliberately longer than 15 bytes; must not panic.
        let h = spawn_pinned(0, "this-name-is-way-too-long-for-linux", || {});
        h.join().unwrap();
    }
}
