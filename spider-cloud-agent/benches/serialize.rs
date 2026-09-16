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

//! What a request body costs to write, against how much of it there is.
//!
//! Defends the README claim that the crate returns and sends only what was asked for.
//! Every field on `RequestParams` skips itself when it is `None`, so a default body is
//! two bytes. The regression this guards against is a field losing its skip attribute,
//! or a default creeping in somewhere: that shows up here as the minimal body growing,
//! and as the time to write it climbing toward the populated one.
//!
//! The byte counts are printed beside the timings, because the size is the claim and
//! the time is only the consequence.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use spider_cloud_agent::params::{
    Cache, Country, ProxyPool, RequestMode, RequestParams, ReturnFormat, ReturnFormatHandling,
    WaitFor,
};
use spider_cloud_agent::params::{Timeout, Viewport};
use std::collections::BTreeMap;
use std::hint::black_box;

/// Nothing set at all. This is the body the service applies all of its own defaults to.
fn empty() -> RequestParams {
    RequestParams::default()
}

/// One address and nothing else, which is what most calls send.
fn minimal() -> RequestParams {
    RequestParams::url("https://example.com/pricing")
}

/// The body a caller builds when they want the page trimmed before it is sent back.
///
/// Still small, and the shape the "returns less" claim is really about: asking for
/// markdown of one selector is a handful of fields, not a configuration file.
fn trimmed() -> RequestParams {
    let mut params = RequestParams::url("https://example.com/pricing");
    params.return_format = Some(ReturnFormatHandling::Single(ReturnFormat::Markdown));
    params.root_selector = Some("main".into());
    params.readability = Some(true);
    params.metadata = Some(true);
    params
}

/// Most of the documented parameter set, filled in.
///
/// The upper bound, and the denominator the other two are read against. If the minimal
/// body ever serializes anywhere near this size, a skip attribute has gone missing.
fn populated() -> RequestParams {
    let mut params = trimmed();

    params.request = Some(RequestMode::Browser);
    params.proxy = Some(ProxyPool::Residential);
    params.country_code = Country::new("de");
    params.stealth = Some(true);
    params.fingerprint = Some(true);
    params.user_agent = Some("an agent string of roughly the usual length".into());
    params.viewport = Some(Viewport {
        width: 1280,
        height: 800,
        ..Viewport::default()
    });
    params.locale = Some("en-GB".into());
    params.cookies = Some("session=abc; Path=/".into());
    params.headers = Some(BTreeMap::from([
        ("accept-language".to_string(), "en-GB".to_string()),
        ("referer".to_string(), "https://example.com/".to_string()),
    ]));
    params.session = Some(true);
    params.request_timeout = Some(30);
    params.limit = Some(250);
    params.depth = Some(3);
    params.blacklist = Some(vec!["/admin".into(), "/cart".into()]);
    params.whitelist = Some(vec!["/docs".into()]);
    params.subdomains = Some(true);
    params.sitemap = Some(true);
    params.respect_robots = Some(true);
    params.wait_for = Some(WaitFor::idle_network(Timeout::from_secs(10)));
    params.scroll = Some(4);
    params.full_resources = Some(true);
    params.return_headers = Some(true);
    params.return_cookies = Some(true);
    params.return_page_links = Some(true);
    params.cache = Some(Cache::Enabled(true));

    params
}

fn serialize(c: &mut Criterion) {
    let bodies = [
        ("empty", empty()),
        ("minimal", minimal()),
        ("trimmed", trimmed()),
        ("populated", populated()),
    ];

    println!("\nrequest body sizes");
    println!("{:<12} {:>7}", "body", "bytes");
    for (name, params) in &bodies {
        let size = serde_json::to_string(params)
            .expect("a request body serializes")
            .len();
        println!("{name:<12} {size:>7}");
    }
    println!();

    let mut group = c.benchmark_group("serialize_request");

    for (name, params) in &bodies {
        let size = serde_json::to_string(params)
            .expect("a request body serializes")
            .len();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_function(*name, |b| {
            b.iter(|| black_box(serde_json::to_string(black_box(params)).expect("serialize")))
        });
    }

    group.finish();
}

/// Writing straight into a buffer, which is what the transport does.
///
/// Separate from the group above because `to_string` allocates the buffer and that
/// allocation is not part of what the body costs to write.
fn serialize_into(c: &mut Criterion) {
    let params = populated();
    let mut buffer = Vec::with_capacity(4096);

    c.bench_function("serialize_request/populated_into_buffer", |b| {
        b.iter(|| {
            buffer.clear();
            serde_json::to_writer(&mut buffer, black_box(&params)).expect("serialize");
            black_box(buffer.len())
        })
    });
}

criterion_group!(benches, serialize, serialize_into);
criterion_main!(benches);
