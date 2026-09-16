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

//! What a response costs to read into the types a caller holds.
//!
//! Defends the claim that decoding scales with the response and nothing else. The
//! inputs are the recorded fixtures in `tests/fixtures`, the same files `tests/deser.rs`
//! asserts the shapes of, so a fixture change moves both at once and neither drifts
//! from what the API actually sends.
//!
//! `MultiBody` gets a group of its own. It is the one type here with a hand written
//! visitor rather than a derive, because a body arrives as a bare string in some
//! responses and as an object of named formats in others. Hand written means a change
//! to it is a change somebody typed, which is exactly the kind of change worth timing.
//!
//! The fixtures are compiled in rather than read at run time, so no file system work
//! lands inside a measured region.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use spider_cloud_agent::response::content::MultiBody;
use spider_cloud_agent::response::costs::Costs;
use spider_cloud_agent::response::metadata::Metadata;
use spider_cloud_agent::response::SearchResults;
use std::hint::black_box;

/// One page of markdown with its metadata and its full cost breakdown.
const SINGLE_PAGE: &str = include_str!("../tests/fixtures/scrape_markdown.json");

/// Three pages, one served, one refused, one that answered with nothing.
const MIXED_CRAWL: &str = include_str!("../tests/fixtures/crawl_mixed.json");

/// One page returned in three formats at once, which is the widest `MultiBody` shape.
const MULTI_FORMAT: &str = include_str!("../tests/fixtures/multi_format.json");

/// A list of results rather than a page, which decodes through a derive end to end.
const SEARCH_RESULTS: &str = include_str!("../tests/fixtures/search_results.json");

/// Read every page in a response into the typed pieces a caller sees.
///
/// This is the work a caller pays for on every response, and it is the whole of it:
/// the content, the costs and the metadata, one page at a time.
fn decode_pages(text: &str) -> usize {
    let value: serde_json::Value = serde_json::from_str(text).expect("a fixture is json");
    let pages = match value.as_array() {
        Some(pages) => pages.clone(),
        None => vec![value],
    };

    let mut fields = 0;

    for page in pages {
        if let Some(content) = page.get("content") {
            if !content.is_null() {
                let body: MultiBody =
                    serde_json::from_value(content.clone()).expect("a content field");
                fields += body.present().len();
            }
        }
        if let Some(costs) = page.get("costs") {
            let costs: Costs = serde_json::from_value(costs.clone()).expect("a costs field");
            fields += usize::from(costs.total().get() > 0.0);
        }
        if let Some(metadata) = page.get("metadata") {
            let metadata: Metadata =
                serde_json::from_value(metadata.clone()).expect("a metadata field");
            fields += usize::from(!metadata.is_empty());
        }
    }

    fields
}

fn decode(c: &mut Criterion) {
    let fixtures = [
        ("single_page", SINGLE_PAGE),
        ("mixed_crawl", MIXED_CRAWL),
        ("multi_format", MULTI_FORMAT),
    ];

    println!("\nfixture sizes");
    println!("{:<14} {:>7}", "fixture", "bytes");
    for (name, text) in fixtures {
        println!("{name:<14} {:>7}", text.len());
    }
    println!("{:<14} {:>7}", "search_results", SEARCH_RESULTS.len());
    println!();

    let mut group = c.benchmark_group("decode_response");

    for (name, text) in fixtures {
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_function(name, |b| {
            b.iter(|| black_box(decode_pages(black_box(text))))
        });
    }

    group.throughput(Throughput::Bytes(SEARCH_RESULTS.len() as u64));
    group.bench_function("search_results", |b| {
        b.iter(|| {
            black_box(
                serde_json::from_str::<SearchResults>(black_box(SEARCH_RESULTS))
                    .expect("search results"),
            )
        })
    });

    group.finish();
}

/// The hand written visitor on its own, with the json parse taken out of the timing.
///
/// The content object is lifted out of each fixture in setup and handed over as text,
/// so what is measured is the visitor reading it rather than `serde_json` finding it.
fn multi_body(c: &mut Criterion) {
    let cases = [
        ("markdown_only", SINGLE_PAGE),
        ("three_formats", MULTI_FORMAT),
        ("blank", MIXED_CRAWL),
    ];

    let mut group = c.benchmark_group("multi_body");

    for (name, fixture) in cases {
        let value: serde_json::Value = serde_json::from_str(fixture).expect("a fixture is json");
        // The blank case is the third page of the mixed crawl, the 200 that served
        // nothing. The others are the first page of their file.
        let index = usize::from(name == "blank") * 2;
        let page = match value.as_array() {
            Some(pages) => pages[index].clone(),
            None => value,
        };
        let content = serde_json::to_string(&page["content"]).expect("a content field");

        group.throughput(Throughput::Bytes(content.len() as u64));
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(
                    serde_json::from_str::<MultiBody>(black_box(&content))
                        .expect("a content field"),
                )
            })
        });
    }

    group.finish();
}

criterion_group!(benches, decode, multi_body);
criterion_main!(benches);
