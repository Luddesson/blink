//! Zero-allocation test for `V1Kernel::decide`. Uses `stats_alloc` to
//! wrap the global allocator and count allocations observed during a
//! tight loop of 10 000 `decide` calls.

#![cfg(feature = "test-support")]

use std::alloc::System;

use blink_kernel::test_support::fixture;
use blink_kernel::{DecisionKernel, KernelStats, V1Kernel};
use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn decide_is_zero_alloc_over_10k_iterations() {
    let f = fixture();
    let kernel = V1Kernel::new();
    let mut stats = KernelStats::new();

    // Warm up once outside the region.
    {
        let snap = f.snapshot();
        let _ = kernel.decide(&snap, &mut stats);
    }

    let region = Region::new(GLOBAL);

    for _ in 0..10_000 {
        let snap = f.snapshot();
        let v = kernel.decide(&snap, &mut stats);
        std::hint::black_box(v);
    }

    let c = region.change();
    assert_eq!(
        c.allocations, 0,
        "V1Kernel::decide allocated {} times over 10k iterations ({c:?})",
        c.allocations
    );
    assert_eq!(c.reallocations, 0, "unexpected reallocations: {c:?}");
}
