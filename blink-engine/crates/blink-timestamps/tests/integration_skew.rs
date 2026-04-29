//! End-to-end exercise of the calibration + skew self-test paths.
//!
//! Kept fast (< 200 ms) so it runs in every CI pass.

use blink_timestamps::{
    calibration_source, init_with_policy, run_skew_selftest, tsc_hz, Backend,
    CalibrationSource, InitPolicy, SkewVerdict, Timestamp,
};

#[test]
fn init_exposes_calibration_source_and_timestamps_advance() {
    let backend = init_with_policy(InitPolicy::AllowFallback);
    let src = calibration_source();

    match backend {
        Backend::Tsc => {
            assert!(matches!(
                src,
                CalibrationSource::Cpuid15 | CalibrationSource::MonotonicRaw
            ));
            assert!(tsc_hz() > 100_000_000, "implausible tsc_hz={}", tsc_hz());
        }
        Backend::Instant => {
            assert_eq!(src, CalibrationSource::InstantFallback);
            assert_eq!(tsc_hz(), 0);
        }
    }

    let t0 = Timestamp::now();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let t1 = Timestamp::now();
    assert!(t1.elapsed_ns_since(t0) >= 1_000_000);
}

#[test]
fn skew_selftest_under_200ms_and_not_fail() {
    let _ = init_with_policy(InitPolicy::AllowFallback);
    let start = std::time::Instant::now();
    let report = run_skew_selftest();
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "skew selftest took {:?}, CI budget is 200ms",
        elapsed
    );
    assert!(report.measured_cores >= 1);
    assert!(
        !matches!(report.verdict, SkewVerdict::Fail),
        "unexpected Fail on CI: {report:?}"
    );
}
