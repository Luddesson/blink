//! No-allocation test for `BreakerSet::admit_submit`. Wraps the global
//! allocator in `stats_alloc` and counts allocations across 10 000
//! hot-path admits while all breakers are `Closed`.

use std::alloc::System;

use blink_breakers::{Admission, BreakerSet, BreakerSetConfig};
use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn admit_submit_is_zero_alloc_over_10k_iterations() {
    let s = BreakerSet::new(BreakerSetConfig::default());

    // Warm up outside the region.
    let _ = s.admit_submit(1);

    let region = Region::new(GLOBAL);

    for i in 0..10_000u64 {
        let v = s.admit_submit(1_000_000 + i);
        std::hint::black_box(v);
    }

    let c = region.change();
    assert_eq!(
        c.allocations, 0,
        "admit_submit allocated {} times over 10k iterations ({c:?})",
        c.allocations
    );
    assert_eq!(c.reallocations, 0, "unexpected reallocations: {c:?}");

    // Sanity-check: everything admitted.
    assert!(matches!(s.admit_submit(999_999_999), Admission::Ok));
}
