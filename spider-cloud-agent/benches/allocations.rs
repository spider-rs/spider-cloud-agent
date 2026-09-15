// Benchmarks opt out of the crate's panic and unsafe rules deliberately.
// The counting allocator needs `unsafe impl GlobalAlloc`, and a benchmark that
// cannot set itself up should stop loudly rather than measure nothing.
#![allow(
    unsafe_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::string_slice
)]

//! What the local decision costs, in allocations.
//!
//! Defends the same README claim as `decide.rs`, from the side a wall clock number
//! hides. A decision that allocates per call shows up in somebody's profile as churn
//! long before it shows up as latency, and the way that regression arrives is quiet:
//! a clone added to a hot branch, a `String` where a `&'static str` was.
//!
//! So this file counts. The allocator below wraps the system one and adds a counter.
//! Every count in `EXPECTED` was measured, not chosen, and the assertions fail on a
//! change in either direction, because a drop is also news worth reading.
//!
//! Three of these numbers are not zero. Two of them are fine: escalating clones the
//! ladder step it hands back, because the step travels to the caller as data they can
//! log and act on, and the geographic step carries a country code that is a `String`.
//!
//! The third is not fine, and it is here rather than tuned away. `budget_stops`
//! allocates once even though the answer is a refusal, because `Policy::escalate`
//! clones the step first and asks the budget about it second. Nothing is wrong with the
//! answer, and one allocation on a path that ends an operation is not urgent, but it is
//! work bought and thrown away and the number says so. Ordering the check before the
//! clone would take it to zero, and that is a change to `src/policy/engine.rs` rather
//! than to this file.

use criterion::{criterion_group, criterion_main, Criterion};
use spider_cloud_agent::credits::Credits;
use spider_cloud_agent::policy::engine::{AttemptState, Observed};
use spider_cloud_agent::policy::{Budget, Policy};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Every allocation made since the process started.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Every byte handed out since the process started.
static BYTES: AtomicUsize = AtomicUsize::new(0);

/// The system allocator with a tally on the way in.
///
/// Reallocation counts as one more allocation and as the growth in bytes, which is
/// what a caller reading a profile would see. Frees are not counted: the question here
/// is how much a decision asks for, not how long it holds it.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// What one call asked for: allocations, then bytes.
type Cost = (usize, usize);

/// Run `f` once and report what it allocated.
///
/// The counter is global, so another thread allocating during the window would inflate
/// the reading. Taking the smallest of several runs removes that: noise can only add.
/// Nothing here is warm-up sensitive, since the measured calls touch no cache and no
/// lazily built table.
fn measure(f: impl Fn()) -> Cost {
    let mut best = (usize::MAX, usize::MAX);

    for _ in 0..16 {
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        f();
        let run = (
            ALLOCATIONS.load(Ordering::Relaxed) - allocations,
            BYTES.load(Ordering::Relaxed) - bytes,
        );
        best = best.min(run);
    }

    best
}

/// A fresh operation under no caps.
fn open_state() -> AttemptState {
    AttemptState::new(Budget::unlimited())
}

/// The cases, and what each one allocated when these numbers were taken.
///
/// See the machine and date in `benches/README.md`. A count here is a promise about
/// the shape of the code, not about the machine, so it holds anywhere the same source
/// is compiled.
const EXPECTED: &[(&str, usize)] = &[
    ("accept", 0),
    ("blocked_escalates", 1),
    ("rate_limited_waits", 0),
    ("rate_limited_jumps_geo", 2),
    ("budget_stops", 1),
];

/// The cases, built the same way `decide.rs` builds them.
fn cases() -> Vec<(&'static str, Observed, AttemptState)> {
    let mut accepted = open_state();
    let served = Observed::seen(200, Some(200)).costing(Credits::new(1.0));
    accepted.record(&served);

    let mut blocked_state = open_state();
    let blocked = Observed::seen(200, Some(403)).costing(Credits::new(1.0));
    blocked_state.record(&blocked);

    let mut limited_state = open_state();
    let limited = Observed::seen(200, Some(429)).costing(Credits::new(1.0));
    limited_state.record(&limited);

    // The same refusal once the one retry at this step is spent. A site that is still
    // rate limiting the fetch is not helped by a heavier request, so this jumps to the
    // geographic step rather than walking the ladder, and that is the branch that
    // clones a step carrying a country code.
    let mut jumped_state = open_state();
    jumped_state.record(&limited);
    jumped_state.retries_at_step = 1;

    let mut spent = AttemptState::new(Budget::default().with_credits(Credits::new(2.0)));
    let over_cap = Observed::seen(200, Some(403)).costing(Credits::new(2.0));
    spent.record(&over_cap);

    vec![
        ("accept", served, accepted),
        ("blocked_escalates", blocked, blocked_state),
        ("rate_limited_waits", limited.clone(), limited_state),
        ("rate_limited_jumps_geo", limited, jumped_state),
        ("budget_stops", over_cap, spent),
    ]
}

fn allocations(c: &mut Criterion) {
    let policy = Policy::standard();
    let cases = cases();

    println!("\nallocations per decide call");
    println!("{:<24} {:>6} {:>8}", "case", "allocs", "bytes");

    for (name, observed, state) in &cases {
        let (allocations, bytes) = measure(|| {
            drop(black_box(
                policy.decide(black_box(observed), black_box(state)),
            ))
        });

        println!("{name:<24} {allocations:>6} {bytes:>8}");

        let expected = EXPECTED
            .iter()
            .find(|(case, _)| case == name)
            .map(|(_, count)| *count)
            .expect("every case has a recorded allocation count");

        assert_eq!(
            allocations, expected,
            "{name} allocated {allocations} times, and it was recorded at {expected}. \
             Either the decision changed or the record is stale, and both are worth reading."
        );
    }
    println!();

    // Timed as well as counted, so the counter's own overhead is visible against the
    // same groups in `decide.rs`, which run without it.
    let mut group = c.benchmark_group("decide_counted");

    for (name, observed, state) in &cases {
        group.bench_function(*name, |b| {
            b.iter(|| black_box(policy.decide(black_box(observed), black_box(state))))
        });
    }

    group.finish();
}

criterion_group!(benches, allocations);
criterion_main!(benches);
