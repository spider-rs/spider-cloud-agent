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

//! What the local decision costs, in time.
//!
//! Defends the claim in the crate README that the router and the escalation policy
//! run on every call because they cost microseconds. If any group in here lands in
//! milliseconds, that sentence has to be rewritten.
//!
//! The allocation side of the same claim is in `allocations.rs`.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use spider_cloud_agent::credits::Credits;
use spider_cloud_agent::params::{Country, RequestParams};
use spider_cloud_agent::policy::engine::{AttemptState, Observed};
use spider_cloud_agent::policy::{Budget, Ladder, Policy, Step};
use std::hint::black_box;
use std::time::Duration;

/// A fresh operation under no caps.
fn open_state() -> AttemptState {
    AttemptState::new(Budget::unlimited())
}

/// An operation that has already spent past a credit cap, so the next decision is a
/// stop rather than a climb.
fn spent_state() -> AttemptState {
    let mut state = AttemptState::new(Budget::default().with_credits(Credits::new(2.0)));
    state.record(&Observed::seen(200, Some(403)).costing(Credits::new(2.0)));
    state
}

/// The decisions worth timing, each named by what it answers.
///
/// A clean accept is the common case and the one that has to be free. A refused fetch
/// is the headline path, and the one that clones a ladder step. A rate limited fetch
/// waits the first time and then jumps to the geographic step rather than walking the
/// ladder, so those are two different amounts of work under one status code. A stop on
/// a cap is the path that must not cost more than the work it refuses.
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

    let over_cap = Observed::seen(200, Some(403)).costing(Credits::new(2.0));

    vec![
        ("accept", served, accepted),
        ("blocked_escalates", blocked, blocked_state),
        ("rate_limited_waits", limited.clone(), limited_state),
        ("rate_limited_jumps_geo", limited, jumped_state),
        ("budget_stops", over_cap, spent_state()),
    ]
}

fn decide(c: &mut Criterion) {
    let policy = Policy::standard();
    let mut group = c.benchmark_group("decide");

    for (name, observed, state) in cases() {
        group.bench_function(name, |b| {
            b.iter(|| black_box(policy.decide(black_box(&observed), black_box(&state))))
        });
    }

    group.finish();
}

/// Writing a step onto a request, which is what an escalation does once it has decided.
///
/// Two steps, because they are not the same shape. The first rung of the ladder sets
/// one parameter. The geographic step sets four, one of which clones a country code, so
/// it is the upper bound on what applying a step costs.
fn apply(c: &mut Criterion) {
    let ladder = Ladder::standard();
    let first = ladder.get(0).expect("the ladder has a first step").clone();
    let geo = ladder
        .get(ladder.len() - 1)
        .expect("the ladder has a last step")
        .clone();

    let mut group = c.benchmark_group("step_apply");

    for (name, step) in [("first_rung", &first), ("residential_geo", &geo)] {
        group.bench_function(name, |b| {
            b.iter_batched_ref(
                || RequestParams::url("https://example.com/pricing"),
                |params| step.apply(black_box(params)),
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

/// Building the ladder, which happens once per operation rather than once per attempt.
///
/// Timed anyway because `Policy::for_target` reads the country rotation out of the
/// address, and a parse that crept onto the per attempt path would be visible as this
/// number moving into `decide`.
fn build(c: &mut Criterion) {
    let mut group = c.benchmark_group("policy_build");

    group.bench_function("standard", |b| b.iter(|| black_box(Policy::standard())));
    group.bench_function("for_target", |b| {
        b.iter(|| {
            black_box(Policy::for_target(black_box(
                "https://example.co.uk/pricing",
            )))
        })
    });
    group.bench_function("step_new", |b| {
        b.iter(|| {
            black_box(Step::new(
                black_box("custom"),
                black_box(4.0),
                black_box(Vec::new()),
            ))
        })
    });
    group.bench_function("country_parse", |b| {
        b.iter(|| black_box(Country::new(black_box("DE"))))
    });

    group.finish();
}

/// One attempt folded into the running totals, which the send loop does between every
/// call and a decision.
fn record(c: &mut Criterion) {
    let observed = Observed::seen(200, Some(403))
        .costing(Credits::new(1.0))
        .taking(Duration::from_millis(820));

    c.bench_function("attempt_state_record", |b| {
        b.iter_batched_ref(
            open_state,
            |state| state.record(black_box(&observed)),
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, decide, apply, build, record);
criterion_main!(benches);
