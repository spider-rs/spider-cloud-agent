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

//! What the routing vocabulary costs to read and write.
//!
//! Defends this crate's README line that it answers locally and in microseconds, and
//! the repository README's reason for a model rather than a language model: the whole
//! point is that the decision is cheap enough to run on every request. Everything a
//! routing answer is expressed in passes through the types below, so if any of this
//! stops being free the sentence stops being true.
//!
//! The routing groups are the ones that matter. `featurize` and
//! `HeuristicRouter::route` run before every request the client makes, so they are
//! held to zero allocations: the url arrives parsed, the feature vector is a fixed
//! array on the stack, and a decision with no country pinned owns nothing. A pinned
//! country is the one exception and it is measured separately, because a country is a
//! `String` and copying it is the whole cost.
//!
//! The counts are printed as well as timed, because `RequestMode::from_wire` lowercases
//! its input and that is an allocation per call. See `benches/README.md` in the client
//! crate for the recorded figure and what it means.

use criterion::{criterion_group, criterion_main, Criterion};
use spider_route::{
    featurize, CallerPins, Country, DeclaredNeed, HeuristicRouter, ProxyPool, RequestMode,
    RouteInput, Router, SiteMemory, StatusClass,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use url::Url;

/// Every allocation made since the process started.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// The system allocator with a tally on the way in.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Run `f` once and report how many allocations it made.
///
/// Smallest of several runs, since a global counter can only be inflated by another
/// thread and never deflated.
fn measure(f: impl Fn()) -> usize {
    let mut best = usize::MAX;

    for _ in 0..16 {
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        f();
        best = best.min(ALLOCATIONS.load(Ordering::Relaxed) - before);
    }

    best
}

/// What each call allocated when these numbers were taken.
///
/// `as_str` returns a `&'static str` and owns nothing, so it is zero and has to stay
/// zero. The other two build an owned `String`, one of them only so it can compare
/// case insensitively, and they are recorded here rather than hidden.
const EXPECTED: &[(&str, usize)] = &[
    ("mode_as_str", 0),
    ("mode_from_wire", 1),
    ("country_new", 1),
];

fn vocabulary(c: &mut Criterion) {
    println!("\nallocations per routing vocabulary call");
    println!("{:<18} {:>6}", "call", "allocs");

    let counts = [
        (
            "mode_as_str",
            measure(|| {
                let _ = black_box(black_box(RequestMode::Browser).as_str());
            }),
        ),
        (
            "mode_from_wire",
            measure(|| {
                let _ = black_box(RequestMode::from_wire(black_box("BROWSER")));
            }),
        ),
        (
            "country_new",
            measure(|| {
                let _ = black_box(Country::new(black_box("DE")));
            }),
        ),
    ];

    for (name, count) in counts {
        println!("{name:<18} {count:>6}");

        let expected = EXPECTED
            .iter()
            .find(|(call, _)| *call == name)
            .map(|(_, count)| *count)
            .expect("every call has a recorded allocation count");

        assert_eq!(
            count, expected,
            "{name} allocated {count} times, and it was recorded at {expected}. \
             Either the call changed or the record is stale, and both are worth reading."
        );
    }
    println!();

    let mut group = c.benchmark_group("route_vocabulary");

    group.bench_function("mode_as_str", |b| {
        b.iter(|| black_box(black_box(RequestMode::Browser).as_str()))
    });
    group.bench_function("mode_from_wire_exact", |b| {
        b.iter(|| black_box(RequestMode::from_wire(black_box("browser"))))
    });
    group.bench_function("mode_from_wire_alias", |b| {
        b.iter(|| black_box(RequestMode::from_wire(black_box("  SmartMode  "))))
    });
    group.bench_function("mode_from_wire_unknown", |b| {
        b.iter(|| black_box(RequestMode::from_wire(black_box("teapot"))))
    });
    group.bench_function("country_new", |b| {
        b.iter(|| black_box(Country::new(black_box("DE"))))
    });
    group.bench_function("country_new_rejects", |b| {
        b.iter(|| black_box(Country::new(black_box("deutschland"))))
    });
    group.bench_function("pool_as_str", |b| {
        b.iter(|| black_box(black_box(ProxyPool::Residential).as_str()))
    });

    group.finish();
}

/// What a routing call allocated when these numbers were taken.
///
/// Zero is the requirement for everything except the pinned country, which copies the
/// code the caller fixed. If any of these moves off zero, something on the hot path
/// started building a string.
const EXPECTED_ROUTES: &[(&str, usize)] = &[
    ("featurize_page", 0),
    ("featurize_query", 0),
    ("featurize_memory", 0),
    ("route_cold", 0),
    ("route_data_file", 0),
    ("route_memory", 0),
    ("route_pinned_country", 1),
];

fn routing(c: &mut Criterion) {
    let page = match Url::parse("https://www.example.com/news/world/a-long-story-about-things") {
        Ok(url) => url,
        Err(error) => panic!("the benchmark fixtures have to parse: {error}"),
    };
    let query = match Url::parse(
        "https://shop.example.co.uk/products/air-max-90?color=red&size=10&utm_source=mail&page=2",
    ) {
        Ok(url) => url,
        Err(error) => panic!("the benchmark fixtures have to parse: {error}"),
    };
    let data = match Url::parse("https://example.com/api/v2/items.json?page=3") {
        Ok(url) => url,
        Err(error) => panic!("the benchmark fixtures have to parse: {error}"),
    };

    let memory = SiteMemory {
        observations: 12,
        success_rate: 0.1,
        streak: -3,
        last_status: StatusClass::Blocked,
    };
    let country = match Country::new("de") {
        Some(country) => country,
        None => panic!("de is a country code"),
    };

    let router = HeuristicRouter::new();
    let cold = RouteInput::new(&page, DeclaredNeed::Markdown);
    let with_query = RouteInput::new(&query, DeclaredNeed::Markdown);
    let with_memory = RouteInput::new(&page, DeclaredNeed::Markdown).with_memory(&memory);
    let data_file = RouteInput::new(&data, DeclaredNeed::Markdown);
    let pinned = RouteInput::new(&page, DeclaredNeed::Markdown).with_pins(CallerPins {
        mode: Some(RequestMode::Browser),
        proxy: Some(ProxyPool::Residential),
        country: Some(&country),
    });

    println!("\nallocations per routing call");
    println!("{:<22} {:>6}", "call", "allocs");

    let counts = [
        (
            "featurize_page",
            measure(|| {
                let _ = black_box(featurize(black_box(&cold)));
            }),
        ),
        (
            "featurize_query",
            measure(|| {
                let _ = black_box(featurize(black_box(&with_query)));
            }),
        ),
        (
            "featurize_memory",
            measure(|| {
                let _ = black_box(featurize(black_box(&with_memory)));
            }),
        ),
        (
            "route_cold",
            measure(|| {
                let _ = black_box(router.route(black_box(&cold)));
            }),
        ),
        (
            "route_data_file",
            measure(|| {
                let _ = black_box(router.route(black_box(&data_file)));
            }),
        ),
        (
            "route_memory",
            measure(|| {
                let _ = black_box(router.route(black_box(&with_memory)));
            }),
        ),
        (
            "route_pinned_country",
            measure(|| {
                let _ = black_box(router.route(black_box(&pinned)));
            }),
        ),
    ];

    for (name, count) in counts {
        println!("{name:<22} {count:>6}");

        let expected = EXPECTED_ROUTES
            .iter()
            .find(|(call, _)| *call == name)
            .map(|(_, count)| *count)
            .expect("every call has a recorded allocation count");

        assert_eq!(
            count, expected,
            "{name} allocated {count} times, and it was recorded at {expected}. \
             Routing runs before every request, so an allocation that appeared here is \
             worth reading before the record is updated."
        );
    }
    println!();

    let mut group = c.benchmark_group("route_decision");

    group.bench_function("featurize_page", |b| {
        b.iter(|| black_box(featurize(black_box(&cold))))
    });
    group.bench_function("featurize_query", |b| {
        b.iter(|| black_box(featurize(black_box(&with_query))))
    });
    group.bench_function("featurize_memory", |b| {
        b.iter(|| black_box(featurize(black_box(&with_memory))))
    });
    group.bench_function("route_cold", |b| {
        b.iter(|| black_box(router.route(black_box(&cold))))
    });
    group.bench_function("route_data_file", |b| {
        b.iter(|| black_box(router.route(black_box(&data_file))))
    });
    group.bench_function("route_memory", |b| {
        b.iter(|| black_box(router.route(black_box(&with_memory))))
    });
    group.bench_function("route_pinned_country", |b| {
        b.iter(|| black_box(router.route(black_box(&pinned))))
    });

    group.finish();
}

criterion_group!(benches, vocabulary, routing);
criterion_main!(benches);
