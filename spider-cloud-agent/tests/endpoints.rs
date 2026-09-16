// Integration tests are their own crate root, so the allow blocks inside the
// library's test modules do not reach here. A test that cannot set itself up
// should stop loudly rather than quietly measure nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::string_slice
)]

//! The addresses this crate is allowed to call.
//!
//! The list below is written out by hand and compared against the compiled route
//! table for equality in both directions. A subset check would let an internal
//! path be added without anyone noticing, which is the thing this file exists to
//! stop. Adding an endpoint means editing this list, and that edit is what shows
//! up in review.

use spider_cloud_agent::client::{Method, ROUTES};

#[test]
fn f1_base_validation_requires_tls_or_literal_loopback() {
    let loopback = std::net::Ipv4Addr::new(127, 0, 0, 1);
    let ftp = format!("ftp://{loopback}");
    let http = format!("http://{loopback}");
    for base in ["http://example.com", "http://localhost", &ftp] {
        assert!(matches!(
            spider_cloud_agent::Spider::builder()
                .key("not-a-real-key")
                .base_url(url::Url::parse(base).unwrap())
                .build(),
            Err(spider_cloud_agent::Error::Config(_))
        ));
    }
    for base in ["https://example.com", &http, "http://[::1]"] {
        assert!(spider_cloud_agent::Spider::builder()
            .key("not-a-real-key")
            .base_url(url::Url::parse(base).unwrap())
            .build()
            .is_ok());
    }
    assert!(spider_cloud_agent::Spider::builder()
        .key("not-a-real-key")
        .base_url(url::Url::parse("http://example.com").unwrap())
        .allow_insecure_http(true)
        .build()
        .is_ok());
}

/// Every path the crate can emit, with its verb.
const EXPECTED: &[(&str, &str)] = &[
    ("POST", "/scrape"),
    ("POST", "/crawl"),
    ("POST", "/links"),
    ("POST", "/search"),
    ("POST", "/screenshot"),
    ("POST", "/transform"),
    ("POST", "/v1/scrape"),
    ("POST", "/v1/crawl"),
    ("POST", "/v1/links"),
    ("POST", "/v1/search"),
    ("POST", "/v1/screenshot"),
    ("POST", "/v1/transform"),
    ("POST", "/unlimited/scrape"),
    ("POST", "/unlimited/crawl"),
    ("POST", "/unlimited/links"),
    ("POST", "/ai/scrape"),
    ("POST", "/ai/crawl"),
    ("POST", "/ai/search"),
    ("POST", "/ai/browser"),
    ("POST", "/ai/links"),
    ("POST", "/fetch/{domain}/{path}"),
    ("GET", "/data/credits"),
    ("GET", "/data/crawl_logs"),
    ("GET", "/data/{table}"),
];

/// The routes no builder reaches, and why each set is left alone.
///
/// A path in the table that nothing calls is how `/fetch` went two releases
/// posting to the wrong endpoint. This list is the other half of that fix: a
/// route is either reached by a builder or it is named here, and adding one to
/// neither fails the test below.
///
/// - the `/v1/` twins answer the same as the unversioned paths they mirror,
///   so a builder for each would double the surface and buy nothing
/// - the `unlimited` paths are for a plan that charges for seats, and sending
///   a per-page account down one is a 4xx rather than a cheaper page
/// - the `ai` paths run a prompt over the page and charge for a model this
///   crate does not otherwise call, which is a decision to make on purpose
///
/// All fourteen are reachable through `Spider::raw`, which is what the escape
/// hatch is for.
const NO_BUILDER: &[&str] = &[
    "/v1/scrape",
    "/v1/crawl",
    "/v1/links",
    "/v1/search",
    "/v1/screenshot",
    "/v1/transform",
    "/unlimited/scrape",
    "/unlimited/crawl",
    "/unlimited/links",
    "/ai/scrape",
    "/ai/crawl",
    "/ai/search",
    "/ai/browser",
    "/ai/links",
];

/// The routes a builder on [`spider_cloud_agent::Spider`] reaches.
const HAS_BUILDER: &[&str] = &[
    "/scrape",
    "/crawl",
    "/links",
    "/search",
    "/screenshot",
    "/transform",
    "/fetch/{domain}/{path}",
    "/data/credits",
    "/data/crawl_logs",
    "/data/{table}",
];

#[test]
fn every_route_either_has_a_builder_or_says_why_it_does_not() {
    for route in ROUTES {
        let built = HAS_BUILDER.contains(&route.path);
        let excused = NO_BUILDER.contains(&route.path);
        assert!(
            built || excused,
            "{route} is in the table and nothing reaches it. Give it a builder \
             or add it to NO_BUILDER with the reason."
        );
        assert!(!(built && excused), "{route} is in both lists");
    }
    assert_eq!(
        HAS_BUILDER.len() + NO_BUILDER.len(),
        ROUTES.len(),
        "a route was named in one of the lists and then removed from the table"
    );
}

fn compiled() -> Vec<(String, String)> {
    ROUTES
        .iter()
        .map(|route| (route.method.as_str().to_string(), route.path.to_string()))
        .collect()
}

fn expected() -> Vec<(String, String)> {
    EXPECTED
        .iter()
        .map(|(method, path)| (method.to_string(), path.to_string()))
        .collect()
}

#[test]
fn the_route_table_is_exactly_the_list_above() {
    let mut compiled = compiled();
    let mut expected = expected();
    compiled.sort();
    expected.sort();

    let added: Vec<_> = compiled.iter().filter(|r| !expected.contains(r)).collect();
    let removed: Vec<_> = expected.iter().filter(|r| !compiled.contains(r)).collect();

    assert!(
        added.is_empty(),
        "the crate can call addresses this list does not name: {added:?}. \
         Add them here if they are meant to be public."
    );
    assert!(
        removed.is_empty(),
        "this list names addresses the crate cannot call: {removed:?}. \
         Remove them here if the endpoint is gone."
    );
    assert_eq!(compiled, expected);
}

#[test]
fn the_count_is_asserted_so_an_empty_table_cannot_pass() {
    // A comparison of two empty lists succeeds. This is the check that a table
    // which failed to compile in, or a list someone emptied, cannot read as a
    // pass.
    assert_eq!(EXPECTED.len(), 24);
    assert_eq!(ROUTES.len(), 24);
}

#[test]
fn nothing_names_the_pipeline_path() {
    // It is commented out server side. A client that can emit it would be asking
    // for something that does not answer.
    for route in ROUTES {
        assert!(
            !route.path.contains("pipeline"),
            "{route} names a path that is not part of the public api"
        );
    }
    assert!(!EXPECTED.iter().any(|(_, path)| path.contains("pipeline")));
}

#[test]
fn every_path_is_absolute_and_carries_no_host() {
    for route in ROUTES {
        assert!(
            route.path.starts_with('/'),
            "{route} is not an absolute path"
        );
        assert!(
            !route.path.contains("://"),
            "{route} carries a host, which belongs to the base address"
        );
        assert!(
            !route.path.contains('?'),
            "{route} carries a query string, which belongs to the call site"
        );
    }
}

#[test]
fn no_path_is_listed_twice() {
    let mut paths: Vec<_> = ROUTES.iter().map(|r| (r.method, r.path)).collect();
    let before = paths.len();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), before, "a route appears more than once");
}

/// Every route that reads a page is a post, and the only gets left are the
/// account reads under `/data`.
///
/// `/fetch` was declared a get on 0.1.x. The server answers a get to it with a
/// 400 before the handler is reached, so the route as shipped could not have
/// worked, and nothing in the crate called it, so nobody found out.
#[test]
fn reads_and_writes_are_split_the_way_the_api_splits_them() {
    let reads: Vec<_> = ROUTES
        .iter()
        .filter(|r| r.method == Method::Get)
        .map(|r| r.path)
        .collect();
    assert_eq!(
        reads,
        vec!["/data/credits", "/data/crawl_logs", "/data/{table}"]
    );
    for route in ROUTES {
        if route.path.starts_with("/data/") {
            continue;
        }
        assert_eq!(
            route.method,
            Method::Post,
            "{route} reads a page, and a content route that is not a post is a 400"
        );
    }
}

#[test]
fn a_templated_path_fills_in_and_refuses_the_wrong_count() {
    let table = ROUTES
        .iter()
        .find(|r| r.path == "/data/{table}")
        .expect("the table route");
    assert_eq!(table.render(&["crawl_logs"]).unwrap(), "/data/crawl_logs");
    assert!(table.render(&[]).is_err());
    assert!(table.render(&["a", "b"]).is_err());

    let fetch = ROUTES
        .iter()
        .find(|r| r.path == "/fetch/{domain}/{path}")
        .expect("the fetch route");
    assert_eq!(
        fetch.render(&["example.com", "docs/index.html"]).unwrap(),
        "/fetch/example.com/docs/index.html"
    );
}

/// A slash survives encoding so one argument can be a whole path, and the price
/// of that is a dot segment. `Url::join` resolves one before the request goes
/// out, so `/data/{table}` filled in with `../v1/scrape` would call
/// `/v1/scrape`: the route would no longer decide which part of the API the
/// call reaches.
#[test]
fn a_path_argument_cannot_walk_out_of_its_route() {
    let table = ROUTES
        .iter()
        .find(|r| r.path == "/data/{table}")
        .expect("the table route");
    for name in ["../v1/scrape", "..", ".", "a/../../b", "./credits"] {
        assert!(
            table.render(&[name]).is_err(),
            "{name} rendered into a path"
        );
    }
    // A dot inside a segment is a filename, not a climb.
    assert_eq!(table.render(&["crawl.logs"]).unwrap(), "/data/crawl.logs");

    let fetch = ROUTES
        .iter()
        .find(|r| r.path == "/fetch/{domain}/{path}")
        .expect("the fetch route");
    assert!(fetch.render(&["example.com", "../../v1/scrape"]).is_err());
    assert_eq!(
        fetch.render(&["example.com", "docs/a.b.html"]).unwrap(),
        "/fetch/example.com/docs/a.b.html"
    );
}

/// Response coverage for the builder inventory above. Each entry names a
/// readable JSON fixture: either one of the older bare bodies or one of the
/// newer records that keep the HTTP envelope beside the body. Both shapes are
/// checked against the route they are claimed for, so naming the wrong file
/// cannot pass as coverage.
const FIXTURE_COVERAGE: &[(&str, &str)] = &[
    ("/scrape", "scrape_markdown.json"),
    ("/crawl", "crawl_mixed.json"),
    ("/links", "links.json"),
    ("/search", "search_results.json"),
    ("/screenshot", "screenshot.json"),
    ("/transform", "transform.json"),
    ("/fetch/{domain}/{path}", "fetch.json"),
    ("/data/credits", "credits.json"),
    ("/data/crawl_logs", "crawl_logs.json"),
    ("/data/{table}", "data_table.json"),
];

// Red with ("/scrape", "credits.json") in place of the scrape line: the test
// failed with "credits.json does not hold a /scrape answer". The shape rules
// are what make that swap visible. Reading an envelope route key alone left
// every older bare-body fixture out of the manifest entirely.
#[test]
fn every_builder_route_has_a_fixture() {
    let mut covered: Vec<_> = FIXTURE_COVERAGE.iter().map(|(route, _)| *route).collect();
    let mut built = HAS_BUILDER.to_vec();
    covered.sort_unstable();
    built.sort_unstable();
    assert!(!built.is_empty());
    assert_eq!(covered, built);
    for (route, file) in FIXTURE_COVERAGE {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        // A record with an envelope names its own route, status and content
        // type. A bare body is the answer on its own.
        let body = match value.get("route") {
            Some(named) => {
                assert_eq!(named, route, "{file}");
                assert_eq!(value["http_status"], 200, "{file}");
                assert_eq!(
                    value["headers"]["content-type"], "application/json",
                    "{file}"
                );
                value["body"].clone()
            }
            None => value,
        };
        assert!(
            holds_the_answer_for(route, &body),
            "{file} does not hold a {route} answer"
        );
    }
}

/// Whether a fixture body is the answer this route gives, told apart by the
/// fields only that route's answer carries.
///
/// Without this a manifest entry could name any readable fixture and still
/// count, which is how a route ends up recorded by somebody else's response.
fn holds_the_answer_for(route: &str, body: &serde_json::Value) -> bool {
    let pages = body.as_array();
    let first = body.get(0).unwrap_or(&serde_json::Value::Null);
    let rows = body.get("data").and_then(|d| d.as_array());
    let row = rows
        .and_then(|r| r.first())
        .unwrap_or(&serde_json::Value::Null);
    match route {
        // One page, asked for as markdown.
        "/scrape" => {
            pages.is_some_and(|p| p.len() == 1) && first["content"]["markdown"].is_string()
        }
        // Several pages from one call, and not all of them answered.
        "/crawl" => {
            pages.is_some_and(|p| p.len() > 1)
                && pages.is_some_and(|p| p.iter().any(|page| page["status"] != 200))
        }
        // Addresses rather than content.
        "/links" => pages.is_some() && first["links"].is_array(),
        // Titled results under their own wrapper, not a page list.
        "/search" => body
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .is_some_and(|hit| hit["title"].is_string() && hit["url"].is_string()),
        // Bytes, not text.
        "/screenshot" => pages.is_some() && first["content"]["screenshot"].is_array(),
        // Converted markup with no address, because nothing was fetched.
        "/transform" => {
            pages.is_some() && first["content"]["raw"].is_string() && first["url"].is_null()
        }
        // Cached markup, which does name the address it was stored under.
        "/fetch/{domain}/{path}" => {
            pages.is_some() && first["content"]["raw"].is_string() && first["url"].is_string()
        }
        // A balance is one number, not a row set.
        "/data/credits" => body
            .get("data")
            .is_some_and(|data| data["credits"].is_number()),
        // Crawl records are keyed by domain.
        "/data/crawl_logs" => {
            rows.is_some() && row["domain"].is_string() && row["pages"].is_number()
        }
        // An arbitrary table comes back as page rows.
        "/data/{table}" => rows.is_some() && row["url"].is_string() && row["domain"].is_null(),
        other => panic!("{other} has no fixture shape rule"),
    }
}
