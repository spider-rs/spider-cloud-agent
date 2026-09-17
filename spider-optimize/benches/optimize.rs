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

//! What the optimizer costs on the path every request takes.
//!
//! `featurize_edit` runs once per candidate and `choose` once per request, so
//! both are held to zero allocations with no model compiled in. `generate`
//! builds the candidate list and `comparison_row` builds a line of text, and
//! both allocate by design; their counts are pinned so a change in either
//! direction is read before the record is updated.

use criterion::{criterion_group, criterion_main, Criterion};
use spider_optimize::{
    choose, comparison_row, featurize_edit, generate, summarize, Arm, ComparisonRow, Context,
    EditDescriptor, Gate, Key, MemoryState, NoModel, Observation, Params, Schema,
};
use spider_route::{
    featurize, DeclaredNeed, ExtClass, ProxyPool, RequestMode, RouteDecision, RouteInput,
    StatusClass,
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

/// Run `f` and report how many allocations it made, smallest of several runs,
/// since a global counter can only be inflated by another thread.
fn measure(f: impl Fn()) -> usize {
    let mut best = usize::MAX;
    for _ in 0..16 {
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        f();
        best = best.min(ALLOCATIONS.load(Ordering::Relaxed) - before);
    }
    best
}

/// The fields an edit reads and writes, standing in for the client's request
/// type, which this crate cannot depend on.
#[derive(Clone, Default)]
struct Request {
    request: Option<RequestMode>,
    proxy: Option<ProxyPool>,
    idle_wait: Option<u32>,
    flags: [Option<bool>; 6],
    network_blacklist: Option<Vec<String>>,
}

const FLAGS: [Key; 6] = [
    Key::DisableIntercept,
    Key::FullResources,
    Key::BlockAds,
    Key::BlockAnalytics,
    Key::BlockStylesheets,
    Key::DisableHints,
];

impl Params for Request {
    fn is_set(&self, key: Key) -> bool {
        match key {
            Key::Request => self.request.is_some(),
            Key::Proxy => self.proxy.is_some(),
            Key::WaitIdleMillis => self.idle_wait.is_some(),
            Key::NetworkBlacklist => self.network_blacklist.is_some(),
            key => self.flag(key).is_some(),
        }
    }
    fn request(&self) -> Option<RequestMode> {
        self.request
    }
    fn proxy(&self) -> Option<ProxyPool> {
        self.proxy
    }
    fn idle_wait_millis(&self) -> Option<u32> {
        self.idle_wait
    }
    fn flag(&self, key: Key) -> Option<bool> {
        FLAGS
            .iter()
            .position(|k| *k == key)
            .and_then(|at| self.flags[at])
    }
    fn list(&self, key: Key) -> Option<&[String]> {
        match key {
            Key::NetworkBlacklist => self.network_blacklist.as_deref(),
            _ => None,
        }
    }
    fn set_request(&mut self, mode: RequestMode) {
        self.request = Some(mode);
    }
    fn set_proxy(&mut self, pool: ProxyPool) {
        self.proxy = Some(pool);
    }
    fn set_idle_wait(&mut self, millis: u32) {
        self.idle_wait = Some(millis);
    }
    fn set_flag(&mut self, key: Key, value: bool) {
        if let Some(at) = FLAGS.iter().position(|k| *k == key) {
            self.flags[at] = Some(value);
        }
    }
    fn set_list(&mut self, key: Key, list: Option<Vec<String>>) {
        if key == Key::NetworkBlacklist {
            self.network_blacklist = list;
        }
    }
}

/// What each call allocated when these numbers were taken.
///
/// `generate` makes 75 on this fixture, a smart fetch with four identifiers
/// that fills the list to its cap of 24:
///
/// - 1 for the list, sized to the cap up front.
/// - 19 for the singles: 8 lists of alternative values, one per key, and 11
///   one-edit sets.
/// - 49 for the pairs: 16 for their alternative lists, of which 9 are the
///   blacklist's (the list, and a `Vec` and a `String` per identifier), then
///   17 two-edit sets, 8 of which clone a blacklist append's `Vec` and
///   `String` as well (9 + 8 * 3). Four of those are refused, because an
///   append does nothing on plain HTTP, and are built all the same.
/// - 6 for the two appends that fit before the cap, a set, a `Vec` and a
///   `String` each. Nothing is built once the list is full.
///
/// `row_write` makes 1: the line is written into one `String` sized for it.
/// `featurize_edit` and `choose_no_model` must stay at zero.
const EXPECTED: &[(&str, usize)] = &[
    ("generate", 75),
    ("featurize_edit", 0),
    ("choose_no_model", 0),
    ("row_write", 1),
];

fn optimize(c: &mut Criterion) {
    let page = Url::parse("https://www.example.com/news/world/a-long-story").unwrap();
    let resources: Vec<(Url, u64)> = [
        ("https://www.example.com/app.js", 40_000),
        ("https://tracker-alpha.example/t.js", 90_000),
        ("https://cdn-beta.example/big.js", 300_000),
        ("https://ads-gamma.example/a.gif", 20_000),
        ("https://fonts-delta.example/f.woff2", 5_000),
    ]
    .iter()
    .map(|(url, bytes)| (Url::parse(url).unwrap(), *bytes))
    .collect();
    let observation = Observation {
        resources: Some(summarize(&page, &resources)),
        ..Observation::cold()
    };
    let current = Request {
        request: Some(RequestMode::Smart),
        flags: [None, None, None, None, None, Some(true)],
        ..Request::default()
    };
    let caller = Request::default();
    let routed = RouteDecision::default();
    let ctx = Context {
        url: &page,
        need: DeclaredNeed::Markdown,
        current: &current,
        caller: &caller,
        routed: &routed,
        observation: &observation,
        multiplier_cap: 8.0,
    };
    let schema = Schema::v1();
    let gate = Gate::default();
    let cands = generate(&schema, &ctx);
    let append = cands
        .iter()
        .find(|c| c.edits.get(Key::NetworkBlacklist).is_some())
        .unwrap()
        .clone();
    let base = featurize(&RouteInput::new(&page, DeclaredNeed::Markdown));
    let feats = featurize_edit(&ctx, &append);
    let row = ComparisonRow {
        pair: 7,
        arm: Arm::Candidate,
        day: 20_000,
        domain_key: 42,
        need: DeclaredNeed::Markdown,
        ext: ExtClass::None,
        tld: 0,
        memory: MemoryState::Cold,
        routed: &routed,
        edit: EditDescriptor::describe(&append.edits, &observation),
        pinned: 0,
        pinned_hi: 0,
        success: true,
        status: StatusClass::Ok,
        millis: 1_200,
        bytes: 40_000,
        credits: 3.5,
        attempts: 1,
        multiplier: append.multiplier,
        fields_requested: 0,
        fields_present: 0,
        content_ok: Some(true),
        fields_ok: None,
        shingle_jaccard: Some(0.98),
        byte_ratio: Some(0.9),
        base: &base,
        edit_feats: &feats,
    };

    println!("\nallocations per optimizer call");
    println!("{:<18} {:>6}", "call", "allocs");

    let counts = [
        (
            "generate",
            measure(|| {
                let _ = black_box(generate(black_box(&schema), black_box(&ctx)));
            }),
        ),
        (
            "featurize_edit",
            measure(|| {
                let _ = black_box(featurize_edit(black_box(&ctx), black_box(&append)));
            }),
        ),
        (
            "choose_no_model",
            measure(|| {
                let _ = black_box(choose(&NoModel, black_box(&ctx), black_box(&cands), &gate));
            }),
        ),
        (
            "row_write",
            measure(|| {
                let _ = black_box(comparison_row(black_box(&row)));
            }),
        ),
    ];

    // Print every count before checking any, so one stale record does not
    // hide the others.
    for (name, count) in counts {
        println!("{name:<18} {count:>6}");
    }
    println!();

    for (name, count) in counts {
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

    let mut group = c.benchmark_group("optimize");
    group.bench_function("generate", |b| {
        b.iter(|| black_box(generate(black_box(&schema), black_box(&ctx))))
    });
    group.bench_function("featurize_edit", |b| {
        b.iter(|| black_box(featurize_edit(black_box(&ctx), black_box(&append))))
    });
    group.bench_function("choose_no_model", |b| {
        b.iter(|| black_box(choose(&NoModel, black_box(&ctx), black_box(&cands), &gate)))
    });
    group.bench_function("row_write", |b| {
        b.iter(|| black_box(comparison_row(black_box(&row))))
    });
    group.finish();
}

criterion_group!(benches, optimize);
criterion_main!(benches);
