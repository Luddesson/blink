use blink_timestamps::*;
fn main() {
    let b = init_with_policy(InitPolicy::AllowFallback);
    println!("backend={:?} source={:?} tsc_hz={}", b, calibration_source(), tsc_hz());
    let r = run_skew_selftest();
    println!("skew: {:?}", r);
}
