//! High-resolution monotonic timestamps for the Blink hot path.
//!
//! On x86_64 with an **invariant, constant-rate, non-stop** TSC this crate
//! reads the CPU timestamp counter directly (`rdtsc` with an `lfence`
//! serialiser). Typical cost: ~15–30 cycles — far below the 50–100 ns of
//! [`std::time::Instant::now`] on Linux.
//!
//! # Usage pattern
//!
//! Call [`init`] **once** at process start. Every subsequent
//! [`Timestamp::now`] is a lock-free read. Convert pairs of timestamps to
//! nanoseconds with [`Timestamp::elapsed_ns_since`] or to a
//! [`core::time::Duration`] with [`Timestamp::duration_since`].
//!
//! # Measurement semantics
//!
//! `rdtsc` is **not** serialising — nearby instructions can reorder around
//! it. For stage-boundary accounting we always pair it with an `lfence` so
//! the counter read is ordered after all prior loads/stores. This is the
//! Intel-recommended pattern for measurement
//! (cf. "How to Benchmark Code Execution Times on Intel® IA-32 and IA-64").
//! The cost is negligible (~5–10 cycles) compared to the stages being
//! measured (hundreds-to-thousands of cycles).
//!
//! # Correctness requirements (x86)
//!
//! The TSC is usable for our purposes **only** on CPUs advertising both
//! `constant_tsc` (frequency independent of P-states) and `nonstop_tsc`
//! (doesn't halt in deep C-states). When `invariant_tsc` / `tsc_reliable`
//! are also advertised, cross-core / cross-socket timestamps are monotonic;
//! otherwise we warn and rely on the pipeline staying on pinned cores.
//!
//! **Known limitations** (tracked as plan risks R-4):
//!
//! - Cross-core skew is validated on demand via [`run_skew_selftest`] /
//!   [`assert_skew_acceptable`]. It is not auto-run at [`init`] time — wire
//!   it into your startup health-check binary.
//! - No detection of VM live-migration TSC jumps. AWS Nitro does not jump
//!   in practice, but bare-metal multi-socket hosts and some virt
//!   platforms can. Protect with the journal's wall-clock sanity field.
//! - CPUID 0x15 (Intel ART/TSC ratio) calibration is used automatically
//!   when the CPU enumerates it; see [`calibration_source`]. The analogous
//!   AMD path (CPUID 0x80000008) is a TODO.
//!
//! [`init`] parses `/proc/cpuinfo` and **panics** if `constant_tsc` or
//! `nonstop_tsc` is missing, unless [`InitPolicy::AllowFallback`] is used
//! or the `fallback-instant` feature is enabled.

#![deny(missing_docs)]
#![forbid(unsafe_op_in_unsafe_fn)]

#[allow(unused_imports)]
use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

#[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
#[allow(unused_imports)]
use core::sync::atomic as _; // suppress unused on non-x86 fallback builds

/// Policy applied when the host CPU does not advertise the TSC invariants
/// required for safe use of `rdtsc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitPolicy {
    /// Panic if the required CPU flags are missing. The **default for
    /// production.** Prevents silently-incorrect latency measurements.
    RequireInvariantTsc,
    /// Fall back to [`std::time::Instant`] when flags are missing. Use for
    /// CI / dev machines / virtualised hosts that do not expose the flags.
    AllowFallback,
}

/// Selected timestamp backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Hardware TSC via `lfence; rdtsc`.
    Tsc,
    /// Software [`std::time::Instant`].
    Instant,
}

/// Which code path produced the active calibration. Recorded on [`init`]
/// and exposed via [`calibration_source`] for startup diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationSource {
    /// Intel CPUID leaf 0x15 (ART / TSC-ratio). Exact, no wall-clock regression.
    Cpuid15,
    /// `CLOCK_MONOTONIC_RAW` 3×250ms median regression (the legacy path).
    MonotonicRaw,
    /// Software [`std::time::Instant`] fallback (TSC unusable).
    InstantFallback,
    // TODO(p1-amd-cpuid): AMD Zen 2+ exposes an analogous frequency via
    // CPUID leaf 0x80000008 ECX. Add an `AmdCpuid8008` variant and wire
    // it up once we have an AMD box in CI.
}

/// Frozen calibration state, published atomically via [`OnceLock`].
///
/// Bundling `backend` with `tsc_hz` into a single `OnceLock` is what lets
/// callers observe a consistent pair — earlier iterations of this module
/// stored them in separate atomics and could expose a torn state during
/// concurrent init.
#[derive(Debug, Clone, Copy)]
struct CalibratedState {
    backend: Backend,
    tsc_hz: u64,
    /// Anchor used on the `Instant` backend so fallback reads fit in u64.
    /// Stored on both backends so the type is uniform.
    instant_anchor: Instant,
    source: CalibrationSource,
}

static CALIBRATED: OnceLock<CalibratedState> = OnceLock::new();

/// An opaque, monotonic timestamp.
///
/// The internal representation is a raw TSC tick count on the hardware
/// backend and a nanosecond offset from a reference point on the `Instant`
/// backend. Do **not** compare timestamps taken before and after a process
/// restart — the reference is re-initialised each run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Read a fresh timestamp. `lfence`-ordered on the TSC backend so the
    /// read is not reordered before prior memory accesses.
    ///
    /// # Panics
    ///
    /// Panics if [`init`] has not been called.
    #[inline(always)]
    pub fn now() -> Self {
        match state().backend {
            Backend::Tsc => Self(read_tsc_fenced()),
            Backend::Instant => Self(fallback_now_ns()),
        }
    }

    /// Raw underlying counter value. Useful for journaling; generally
    /// prefer [`Self::elapsed_ns_since`] for arithmetic.
    #[inline(always)]
    pub fn raw(self) -> u64 {
        self.0
    }

    /// A sentinel "unset" timestamp (raw value 0). Used by types that
    /// want a `Copy`-friendly fixed-shape placeholder — see
    /// `blink_types::StageTimestamps`. Never returned by [`Self::now`] in
    /// practice (probability indistinguishable from zero) and always
    /// distinguishable from a real timestamp by `ts.raw() == 0`.
    pub const UNSET: Self = Self(0);

    /// Nanoseconds elapsed since `earlier`. Saturates at zero when `earlier`
    /// is in the future (non-monotonic reads are clamped to zero rather
    /// than underflowing).
    #[inline(always)]
    pub fn elapsed_ns_since(self, earlier: Self) -> u64 {
        let s = state();
        let ticks = self.0.saturating_sub(earlier.0);
        match s.backend {
            Backend::Tsc => ticks_to_ns(ticks, s.tsc_hz),
            Backend::Instant => ticks, // already ns on this backend
        }
    }

    /// Nanoseconds elapsed since `earlier`, as a [`core::time::Duration`].
    #[inline]
    pub fn duration_since(self, earlier: Self) -> core::time::Duration {
        core::time::Duration::from_nanos(self.elapsed_ns_since(earlier))
    }
}

/// Initialise the timestamp subsystem with the default policy
/// ([`InitPolicy::RequireInvariantTsc`], or [`InitPolicy::AllowFallback`]
/// when the `fallback-instant` feature is enabled).
///
/// Returns the backend selected. Safe to call more than once from the same
/// or different threads; subsequent calls return the first backend chosen
/// (policy is **not** re-evaluated — calibrate once).
pub fn init() -> Backend {
    init_with_policy(default_policy())
}

fn default_policy() -> InitPolicy {
    if cfg!(feature = "fallback-instant") {
        InitPolicy::AllowFallback
    } else {
        InitPolicy::RequireInvariantTsc
    }
}

/// Initialise the timestamp subsystem with an explicit policy. See [`init`].
pub fn init_with_policy(policy: InitPolicy) -> Backend {
    CALIBRATED
        .get_or_init(|| calibrate_once(policy))
        .backend
}

/// Current backend. Panics if [`init`] has not been called.
#[inline(always)]
pub fn backend() -> Backend {
    state().backend
}

/// Measured TSC frequency in Hz. Returns 0 on the `Instant` backend.
#[inline]
pub fn tsc_hz() -> u64 {
    state().tsc_hz
}

/// Which calibration path was taken at [`init`] time. Useful for startup
/// logs and health endpoints.
#[inline]
pub fn calibration_source() -> CalibrationSource {
    state().source
}

#[inline(always)]
fn state() -> &'static CalibratedState {
    CALIBRATED.get().expect(
        "blink-timestamps: init() must be called before Timestamp::now()",
    )
}

// ─── Calibration ──────────────────────────────────────────────────────────

#[allow(clippy::needless_return)] // cfg-gated returns guard the non-x86 tail arm
fn calibrate_once(policy: InitPolicy) -> CalibratedState {
    let anchor = Instant::now();

    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        match verify_invariant_tsc() {
            Ok(()) => {
                let (tsc_hz, source) = calibrate_tsc_hz_with_source();
                return CalibratedState {
                    backend: Backend::Tsc,
                    tsc_hz,
                    instant_anchor: anchor,
                    source,
                };
            }
            Err(reason) => match policy {
                InitPolicy::RequireInvariantTsc => panic!(
                    "blink-timestamps: invariant TSC unavailable: {reason}. \
                     Enable the `fallback-instant` feature or call \
                     init_with_policy(InitPolicy::AllowFallback) on dev/CI hosts."
                ),
                InitPolicy::AllowFallback => {
                    eprintln!(
                        "blink-timestamps: falling back to Instant backend ({reason})"
                    );
                    return CalibratedState {
                        backend: Backend::Instant,
                        tsc_hz: 0,
                        instant_anchor: anchor,
                        source: CalibrationSource::InstantFallback,
                    };
                }
            },
        }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = policy;
        CalibratedState {
            backend: Backend::Instant,
            tsc_hz: 0,
            instant_anchor: anchor,
            source: CalibrationSource::InstantFallback,
        }
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
fn verify_invariant_tsc() -> Result<(), String> {
    let cpuinfo = match std::fs::read_to_string("/proc/cpuinfo") {
        Ok(s) => s,
        Err(e) => return Err(format!("cannot read /proc/cpuinfo: {e}")),
    };

    let flags_line = cpuinfo
        .lines()
        .find(|l| l.starts_with("flags"))
        .ok_or_else(|| "no 'flags' line in /proc/cpuinfo".to_string())?;
    let flags: std::collections::HashSet<&str> = flags_line
        .split_once(':')
        .map(|x| x.1)
        .unwrap_or("")
        .split_ascii_whitespace()
        .collect();

    for f in ["constant_tsc", "nonstop_tsc"] {
        if !flags.contains(f) {
            return Err(format!("missing CPU flag: {f}"));
        }
    }
    if !flags.contains("invariant_tsc") && !flags.contains("tsc_reliable") {
        eprintln!(
            "blink-timestamps: neither invariant_tsc nor tsc_reliable \
             advertised; relying on constant_tsc+nonstop_tsc. Call \
             run_skew_selftest() from your startup health check."
        );
    }
    Ok(())
}

/// Query Intel CPUID leaf 0x15 for an exact TSC frequency.
///
/// Returns `Some(hz)` when the CPU advertises all three of `EAX`
/// (denominator of TSC/crystal ratio), `EBX` (numerator), and `ECX`
/// (crystal clock frequency in Hz). Intel reserves zero in any of these
/// to mean "not enumerated" — we treat that as "unavailable" and let
/// the caller fall back to wall-clock calibration.
///
/// Available on Skylake and later Intel parts. AMD does not implement
/// leaf 0x15 — it exposes an analogous value via CPUID 0x80000008 ECX,
/// tracked as a TODO on `CalibrationSource`.
#[cfg(target_arch = "x86_64")]
fn cpuid_tsc_hz() -> Option<u64> {
    // `__cpuid_count` is a safe intrinsic on stable x86_64 (CPUID has no
    // memory effects and is baseline on the architecture). Reserved or
    // unimplemented leaves simply return zeros.
    let leaf = core::arch::x86_64::__cpuid_count(0x15, 0);
    if leaf.eax == 0 || leaf.ebx == 0 || leaf.ecx == 0 {
        return None;
    }
    Some((leaf.ecx as u64).wrapping_mul(leaf.ebx as u64) / leaf.eax as u64)
}

#[cfg(not(target_arch = "x86_64"))]
#[allow(dead_code)]
fn cpuid_tsc_hz() -> Option<u64> {
    None
}

/// Pick a TSC frequency and record which source produced it.
///
/// Strategy:
/// 1. If CPUID 0x15 is enumerated, take its value and do one cheap (~20 ms)
///    cross-check against `CLOCK_MONOTONIC_RAW`. When they agree to within
///    1 % we skip the expensive 3×250 ms regression — faster startup.
/// 2. When the cross-check disagrees (stale CPUID after VM migration,
///    or a firmware bug) we log a warning and prefer the measured value.
/// 3. When CPUID 0x15 is not available (pre-Skylake, AMD, hypervisor
///    masking) we fall back to the legacy 3×250 ms median regression.
#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
fn calibrate_tsc_hz_with_source() -> (u64, CalibrationSource) {
    #[cfg(target_arch = "x86_64")]
    if let Some(cpuid_hz) = cpuid_tsc_hz() {
        let measured = single_tsc_sample(20);
        let diff = cpuid_hz.max(measured) - cpuid_hz.min(measured);
        // 1 % tolerance — covers the 20 ms sample's scheduler jitter
        // while still flagging genuine mismatches from VM migration.
        let tolerance = cpuid_hz / 100;
        if diff <= tolerance {
            return (cpuid_hz, CalibrationSource::Cpuid15);
        }
        tracing::warn!(
            cpuid_hz,
            measured_hz = measured,
            diff,
            "blink-timestamps: CPUID 0x15 disagrees with CLOCK_MONOTONIC_RAW \
             by >1% — preferring measured value (possible stale CPUID after \
             VM migration)"
        );
        return (measured, CalibrationSource::MonotonicRaw);
    }

    (calibrate_tsc_hz(), CalibrationSource::MonotonicRaw)
}

/// Calibrate TSC frequency against `CLOCK_MONOTONIC_RAW` over a long window
/// (default 250 ms, overridable via `BLINK_TSC_CAL_MS`).
///
/// `CLOCK_MONOTONIC_RAW` is preferred over `Instant`/`CLOCK_MONOTONIC`
/// because the latter is slewed by NTP. 250 ms gives a calibration error
/// of roughly `±(CLOCK_MONOTONIC_RAW resolution / 250 ms)` ≈ low-ppm —
/// adequate for converting short (< minutes) stage intervals to
/// nanoseconds within measurement noise.
#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
fn calibrate_tsc_hz() -> u64 {
    let window_ms: u64 = std::env::var("BLINK_TSC_CAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);

    // Median of 3 samples shields against a single preemption during the
    // sleep. Keeps total calibration cost bounded (~3 × window_ms).
    let mut samples = [0u64; 3];
    for s in samples.iter_mut() {
        *s = single_tsc_sample(window_ms);
    }
    samples.sort_unstable();
    samples[1]
}

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
fn single_tsc_sample(window_ms: u64) -> u64 {
    let start_ns = monotonic_raw_ns();
    let t0 = read_tsc_fenced();
    std::thread::sleep(std::time::Duration::from_millis(window_ms));
    let t1 = read_tsc_fenced();
    let end_ns = monotonic_raw_ns();
    let elapsed_ns = end_ns.saturating_sub(start_ns).max(1);
    let ticks = t1.saturating_sub(t0);
    // hz = ticks * 1e9 / ns
    ((ticks as u128) * 1_000_000_000u128 / elapsed_ns as u128) as u64
}

#[cfg(all(unix, any(target_arch = "x86_64", target_arch = "x86")))]
fn monotonic_raw_ns() -> u64 {
    // SAFETY: `clock_gettime` writes to a well-initialised timespec on
    // success; we zero-initialise first. We ignore the return value only
    // after checking it's 0; otherwise we fall back to CLOCK_MONOTONIC
    // semantics via `Instant`.
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // CLOCK_MONOTONIC_RAW: not adjusted by NTP; best for calibrating a
    // hardware counter. Available on Linux ≥ 2.6.28 and recent macOS.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut ts) };
    if rc == 0 {
        (ts.tv_sec as u64).wrapping_mul(1_000_000_000).wrapping_add(ts.tv_nsec as u64)
    } else {
        // Extremely unlikely; degrade to slewed monotonic rather than panic.
        // The calibration error becomes whatever NTP is doing right now —
        // still far better than no timestamps at all.
        static ANCHOR: OnceLock<Instant> = OnceLock::new();
        let anchor = *ANCHOR.get_or_init(Instant::now);
        Instant::now().duration_since(anchor).as_nanos() as u64
    }
}

#[cfg(all(not(unix), any(target_arch = "x86_64", target_arch = "x86")))]
fn monotonic_raw_ns() -> u64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    let anchor = *ANCHOR.get_or_init(Instant::now);
    Instant::now().duration_since(anchor).as_nanos() as u64
}

// ─── rdtsc reads ──────────────────────────────────────────────────────────

/// `lfence; rdtsc` — the measurement-grade pattern. `lfence` waits for all
/// prior instructions to retire before the counter is read, eliminating
/// the out-of-order slack that plain `rdtsc` would otherwise admit.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn read_tsc_fenced() -> u64 {
    // SAFETY: `_mm_lfence` and `_rdtsc` have no memory effects beyond
    // ordering; they are always safe to execute on any x86_64 CPU (lfence
    // is part of SSE2 which is baseline for x86_64).
    unsafe {
        core::arch::x86_64::_mm_lfence();
        core::arch::x86_64::_rdtsc()
    }
}

#[cfg(target_arch = "x86")]
#[inline(always)]
fn read_tsc_fenced() -> u64 {
    // SAFETY: same rationale as the x86_64 path. SSE2 (and thus lfence)
    // is not guaranteed on 32-bit x86, but any target that builds this
    // crate for real HFT use is x86_64. We still emit the fence — Rust
    // requires SSE2 as a baseline when the target-feature is enabled by
    // the toolchain.
    unsafe {
        core::arch::x86::_mm_lfence();
        core::arch::x86::_rdtsc()
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
#[inline(always)]
fn read_tsc_fenced() -> u64 {
    0
}

#[inline(always)]
fn ticks_to_ns(ticks: u64, hz: u64) -> u64 {
    if hz == 0 {
        return 0;
    }
    // (ticks * 1e9) / hz, done in u128 to avoid overflow.
    ((ticks as u128) * 1_000_000_000u128 / hz as u128) as u64
}

#[inline(always)]
fn fallback_now_ns() -> u64 {
    let anchor = state().instant_anchor;
    Instant::now().duration_since(anchor).as_nanos() as u64
}

// ─── Cross-core skew self-test ────────────────────────────────────────────

/// Verdict category for [`run_skew_selftest`].
///
/// Thresholds are oriented around HFT stage-timing budgets — a few hundred
/// nanoseconds of skew silently corrupts per-stage attribution, whereas
/// microsecond-scale skew only starts corrupting end-to-end wall-clock
/// reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkewVerdict {
    /// Worst observed cross-core delta < 100 ns. Safe for all uses.
    Ok,
    /// 100 ns ≤ skew < 10 µs, or some cores could not be pinned.
    /// Pipeline must keep measurements on the originating core.
    Warn,
    /// Skew ≥ 10 µs. Do **not** compare cross-core timestamps; investigate
    /// (VM migration, hotplug, or firmware TSC re-sync failure).
    Fail,
}

/// Outcome of [`run_skew_selftest`].
#[derive(Debug, Clone, Copy)]
pub struct SkewReport {
    /// Largest observed (signed absolute) tick-delta between any two cores,
    /// converted to nanoseconds via the calibrated `tsc_hz`.
    pub max_skew_ns: u64,
    /// The `(core_a, core_b)` pair that produced `max_skew_ns`.
    pub worst_pair: (u32, u32),
    /// Number of cores that were successfully sampled.
    pub measured_cores: u32,
    /// Number of cores that were sampled *without* `sched_setaffinity`
    /// succeeding (e.g. inside a container without `CAP_SYS_NICE`). The
    /// measurement is best-effort; verdict is at most [`SkewVerdict::Warn`]
    /// when this is non-zero.
    pub unpinned_cores: u32,
    /// Categorical verdict; see [`SkewVerdict`].
    pub verdict: SkewVerdict,
}

/// Exercise the TSC cross-core skew self-test.
///
/// Spawns one thread per online CPU, pins each to its core, and collects
/// samples at rendezvous points established by a [`std::sync::Barrier`].
/// At each round every thread blocks on the barrier and then issues an
/// `lfence; rdtsc` as quickly as possible after release; the skew
/// estimate for a pair `(a, b)` is `min_round |tsc_a - tsc_b|` — the
/// minimum is dominated by the true hardware skew because scheduler
/// wake-up jitter can only *add* positive noise, never subtract.
///
/// **Slow (~50 ms, scales with core count).** Not called from [`init`];
/// intended for a startup health-check binary or a periodic probe.
///
/// Safe to call before or after [`init`]; the test uses the same
/// `lfence; rdtsc` pattern as [`Timestamp::now`] but does not consult
/// the calibration state until the very end (to convert ticks → ns).
pub fn run_skew_selftest() -> SkewReport {
    use std::sync::{Arc, Barrier};

    let n_cpus = num_cpus::get().max(1);
    // 128 barrier rounds gives us enough redundancy that scheduler jitter
    // on any individual round does not dominate the min estimator, while
    // keeping total cost well under 50 ms on typical CI runners.
    const ROUNDS: usize = 128;

    struct Sample {
        core: u32,
        ticks: Vec<u64>,
        pinned: bool,
    }

    let barrier = Arc::new(Barrier::new(n_cpus));
    let handles: Vec<_> = (0..n_cpus)
        .map(|core_id| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || -> Sample {
                let pinned = pin_to_core(core_id);
                // Yield once so migration (if any) settles before round 0.
                std::thread::yield_now();
                let mut ticks = Vec::with_capacity(ROUNDS);
                for _ in 0..ROUNDS {
                    barrier.wait();
                    ticks.push(read_tsc_fenced());
                }
                Sample {
                    core: core_id as u32,
                    ticks,
                    pinned,
                }
            })
        })
        .collect();

    let mut samples: Vec<Sample> = handles
        .into_iter()
        .filter_map(|h| h.join().ok())
        .collect();
    samples.sort_by_key(|s| s.core);

    let measured_cores = samples.len() as u32;
    let unpinned_cores = samples.iter().filter(|s| !s.pinned).count() as u32;

    let mut max_skew_ticks = 0u64;
    let mut worst_pair = (0u32, 0u32);
    for (i, a) in samples.iter().enumerate() {
        for b in &samples[i + 1..] {
            // Min |delta| over rounds ≈ true hardware skew; jitter can
            // only inflate |delta|, so taking the min filters it out.
            let mut pair_skew = u64::MAX;
            for (ta, tb) in a.ticks.iter().zip(b.ticks.iter()) {
                let d = ta.max(tb) - ta.min(tb);
                if d < pair_skew {
                    pair_skew = d;
                }
            }
            if pair_skew != u64::MAX && pair_skew > max_skew_ticks {
                max_skew_ticks = pair_skew;
                worst_pair = (a.core, b.core);
            }
        }
    }

    // Ticks → ns using the calibrated frequency when available; otherwise
    // treat the min-delta as already being in the `Instant` backend's ns.
    let max_skew_ns = match CALIBRATED.get() {
        Some(c) if c.backend == Backend::Tsc && c.tsc_hz > 0 => {
            ticks_to_ns(max_skew_ticks, c.tsc_hz)
        }
        Some(_) => max_skew_ticks, // Instant backend: already ns
        None => max_skew_ticks,    // no init: report raw ticks
    };

    let verdict = if max_skew_ns >= 10_000 {
        SkewVerdict::Fail
    } else if max_skew_ns >= 100 || unpinned_cores > 0 {
        SkewVerdict::Warn
    } else {
        SkewVerdict::Ok
    };

    SkewReport {
        max_skew_ns,
        worst_pair,
        measured_cores,
        unpinned_cores,
        verdict,
    }
}

/// Run [`run_skew_selftest`] and enforce a production-grade verdict.
///
/// Panics on [`SkewVerdict::Fail`] (cross-core timestamps would corrupt
/// latency analytics). Emits a `tracing::warn!` on [`SkewVerdict::Warn`]
/// so the operator can correlate with container-capability drops.
pub fn assert_skew_acceptable() {
    let report = run_skew_selftest();
    match report.verdict {
        SkewVerdict::Ok => {
            tracing::info!(
                max_skew_ns = report.max_skew_ns,
                measured_cores = report.measured_cores,
                "blink-timestamps: cross-core TSC skew OK"
            );
        }
        SkewVerdict::Warn => {
            tracing::warn!(
                max_skew_ns = report.max_skew_ns,
                worst_pair = ?report.worst_pair,
                unpinned_cores = report.unpinned_cores,
                "blink-timestamps: cross-core TSC skew elevated"
            );
        }
        SkewVerdict::Fail => panic!(
            "blink-timestamps: cross-core TSC skew {} ns between cores {:?} \
             exceeds 10 µs ceiling — refusing to start with corrupt clocks",
            report.max_skew_ns, report.worst_pair
        ),
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn pin_to_core(core_id: usize) -> bool {
    // SAFETY: `cpu_set_t` is a plain POSIX bitmap; zeroed-then-CPU_SET is
    // the documented initialisation idiom. `sched_setaffinity(0, ...)`
    // applies only to the calling thread when called from within a std
    // thread — no cross-thread effects. We treat any error (EPERM inside
    // unprivileged containers, EINVAL for offline CPUs) as "unpinned" and
    // still sample, which is what the `unpinned_cores` accounting is for.
    unsafe {
        let mut set: libc::cpu_set_t = core::mem::zeroed();
        libc::CPU_SET(core_id, &mut set);
        libc::sched_setaffinity(0, core::mem::size_of::<libc::cpu_set_t>(), &set) == 0
    }
}

#[cfg(not(all(unix, target_os = "linux")))]
fn pin_to_core(_core_id: usize) -> bool {
    false
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_with_fallback_always_works() {
        let backend = init_with_policy(InitPolicy::AllowFallback);
        assert!(matches!(backend, Backend::Tsc | Backend::Instant));
        let t0 = Timestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let t1 = Timestamp::now();
        let elapsed = t1.elapsed_ns_since(t0);
        assert!(elapsed >= 1_000_000, "elapsed was {elapsed} ns, expected ≥1ms");
        assert!(
            elapsed < 500_000_000,
            "elapsed was {elapsed} ns, expected <500ms (scheduler hiccup?)"
        );
    }

    #[test]
    fn saturating_elapsed_when_out_of_order() {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        let earlier = Timestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let later = Timestamp::now();
        // Swap arguments: `earlier` as "self" and `later` as the "earlier"
        // parameter is an out-of-order comparison that should clamp to 0.
        assert_eq!(earlier.elapsed_ns_since(later), 0);
    }

    #[test]
    fn duration_since_round_trips() {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        let t0 = Timestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let t1 = Timestamp::now();
        let d = t1.duration_since(t0);
        assert!(d.as_millis() >= 4);
    }

    #[test]
    fn backend_is_stable_after_init() {
        let first = init_with_policy(InitPolicy::AllowFallback);
        let second = init_with_policy(InitPolicy::RequireInvariantTsc);
        // Second call must NOT re-evaluate policy — it returns the sticky
        // initial choice (proves OnceLock publication semantics).
        assert_eq!(first, second);
    }

    #[test]
    fn tsc_hz_is_consistent_with_backend() {
        let b = init_with_policy(InitPolicy::AllowFallback);
        let hz = tsc_hz();
        match b {
            Backend::Tsc => assert!(
                hz > 100_000_000,
                "TSC backend but implausible hz={hz}"
            ),
            Backend::Instant => assert_eq!(hz, 0),
        }
    }

    #[test]
    fn calibration_source_is_set() {
        let b = init_with_policy(InitPolicy::AllowFallback);
        let src = calibration_source();
        match b {
            Backend::Tsc => assert!(
                matches!(src, CalibrationSource::Cpuid15 | CalibrationSource::MonotonicRaw),
                "TSC backend with unexpected source {src:?}"
            ),
            Backend::Instant => {
                assert_eq!(src, CalibrationSource::InstantFallback);
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn cpuid_path_or_fallback_works() {
        // Either the CPU enumerates leaf 0x15, or we fall back — in either
        // case init() must succeed with AllowFallback and produce a
        // plausible tsc_hz or the Instant backend.
        let b = init_with_policy(InitPolicy::AllowFallback);
        match b {
            Backend::Tsc => assert!(tsc_hz() > 100_000_000),
            Backend::Instant => {
                // cpuid_tsc_hz may still succeed in a VM even though the
                // backend fell back because of a missing cpuinfo flag.
                let _ = cpuid_tsc_hz();
            }
        }
    }

    #[test]
    fn skew_selftest_returns_ok_or_warn() {
        let _ = init_with_policy(InitPolicy::AllowFallback);
        let report = run_skew_selftest();
        assert!(report.measured_cores >= 1, "no cores measured");
        assert!(
            !matches!(report.verdict, SkewVerdict::Fail),
            "unexpected Fail verdict on CI: {report:?}"
        );
    }
}
