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

//! Recorded responses, read into the types callers see.
//!
//! The fixtures in `tests/fixtures` are what the API sends, with every host
//! replaced by a documentation host. If a field moves or a shape changes, these
//! fail here rather than at a call site.
//!
//! Every fixture is the output of
//! `cargo run --locked -p xtask -- redact <recording> -o <fixture>`, run over a
//! recording held outside the repo. A recording is one reply: the route it came
//! from, its HTTP status, the response headers that carry meaning for the send
//! loop, and the JSON body, with the service hosts and the gateway session
//! cookie still in it. The redactor rewrites every host outside the
//! documentation set to example.com, replaces the cookie with REDACTED, and
//! pretty prints with sorted keys. Add a fixture by running that command,
//! never by writing the file, and run
//! `cargo run --locked -p xtask -- leakcheck --tree` before committing it.

use spider_cloud_agent::response::content::MultiBody;
use spider_cloud_agent::response::costs::Costs;
use spider_cloud_agent::response::metadata::Metadata;
use spider_cloud_agent::response::SearchResults;
use spider_cloud_agent::{Credits, Usd};

/// One recorded reply, read the way the client reads it: the JSON body with
/// an HTTP 200 around it, sorted into pages.
fn read_reply(name: &str, elapsed_ms: u64) -> spider_cloud_agent::response::Pages {
    use spider_cloud_agent::client::{RateLimit, Reply};
    use spider_cloud_agent::policy::engine::Reached;
    use spider_cloud_agent::policy::Observed;
    let Reached::Api(status) = Observed::seen(200, None).api else {
        panic!("api status")
    };
    let reply = Reply {
        status,
        rate_limit: RateLimit::default(),
        retry_after: None,
        elapsed: std::time::Duration::from_millis(elapsed_ms),
        content_type: Some("application/json".into()),
        body: bytes::Bytes::from(fixture(name)),
    };
    reply
        .read(&url::Url::parse("https://example.com").unwrap(), None)
        .unwrap()
}

// The three fixtures below were not captured from a live account. They were
// written to the key set and value types the service's page writer produces
// (`url`, `error`, `status`, `costs` with the formatted strings, whole
// millisecond `duration_elasped_ms`, `content` keyed by format, `links`,
// `headers`, `cookies`, `metadata`, `css_extracted`, `trace` as phase rows,
// `json_data` keyed by script kind, and the two event maps keyed by address),
// as read from the service on 2026-09-16, and then passed through
// `cargo run -p xtask -- redact`, which is why every host is example.com and
// the session cookie reads REDACTED. When a live capture of one of these
// shapes lands, it should replace the file and these tests should still pass.

/// `refused_markdown.json`: what a scrape answers for a page the site refused
/// with a 403, when markdown, links, headers, cookies and metadata were asked
/// for. The site's block page comes with it.
#[test]
fn a_refused_page_arrives_with_the_body_the_site_sent() {
    let pages = read_reply("refused_markdown.json", 900);
    assert_eq!(pages.len(), 1);
    let page = pages.failed().next().expect("a refusal");
    assert_eq!(page.status.code(), 403);
    assert_eq!(page.error.as_deref(), Some("the site refused the fetch"));
    assert!(page.text().unwrap().starts_with("# Access denied"));
    assert!(page.body.fields().is_none());
    assert_eq!(page.duration, Some(std::time::Duration::from_millis(412)));
    assert_eq!(page.call_elapsed.as_millis(), 900);
    let meta = page.metadata.as_ref().expect("metadata");
    assert_eq!(meta.title.as_deref(), Some("Access denied"));
    assert_eq!(
        meta.original_url.as_deref(),
        Some("https://example.com/account")
    );
    assert!(meta.final_url.is_none());
    assert!(meta.crawl_id.is_none());
    assert_eq!(page.links.as_ref().map(Vec::len), Some(2));
    assert_eq!(
        page.headers
            .as_ref()
            .and_then(|h| h.get("server"))
            .map(String::as_str),
        Some("nginx")
    );
    assert_eq!(
        page.cookies
            .as_ref()
            .and_then(|c| c.get("session"))
            .map(String::as_str),
        Some("REDACTED")
    );
    assert!(page.was_billed());
    assert_eq!(page.cost(), Credits::from_usd(0.00021301));
    assert!(page.costs.vendor.is_none());
    assert!(page.json_data.is_none() && page.request_map.is_none() && page.trace.is_none());
}

/// `page_diagnostics.json`: a served page with everything a request can ask
/// for beside the content: extraction fields next to markdown, the declared
/// JSON data, the event tracker's two maps, the phase trace, a redirect in the
/// metadata with an embedding and a crawl id, and a vendor line on the bill.
#[test]
fn a_page_with_every_extra_keeps_each_one_where_it_belongs() {
    let pages = read_reply("page_diagnostics.json", 1500);
    let page = pages.first_ok().expect("a page");
    assert_eq!(page.text(), Some("# Pricing\n\nStarter, Growth, Scale.\n"));
    let fields = page.body.fields().expect("fields beside the page");
    assert_eq!(
        fields["plans"],
        serde_json::json!(["Starter", "Growth", "Scale"])
    );
    assert!(page.error.is_none());
    assert_eq!(page.duration, Some(std::time::Duration::from_millis(1288)));
    assert_eq!(page.call_elapsed.as_millis(), 1500);

    let data = page.json_data.as_ref().expect("json data");
    assert_eq!(data["other_scripts"][0]["@type"], "Product");
    // The redactor rewrote the schema host in the recording too.
    assert_eq!(data["other_scripts"][0]["@context"], "https://example.com");
    let requests = page.request_map.as_ref().expect("request map");
    assert_eq!(requests["https://example.com/app.js"], 41.5);
    let responses = page.response_map.as_ref().expect("response map");
    assert_eq!(responses["https://example.com/pricing"], 48213.0);
    let trace = page.trace.as_ref().expect("trace");
    assert_eq!(trace[1][0], "fetch");
    assert_eq!(trace[1][2], 1201);

    let meta = page.metadata.as_ref().expect("metadata");
    assert_eq!(
        meta.original_url.as_deref(),
        Some("https://example.com/plans")
    );
    assert_eq!(
        meta.final_url.as_deref(),
        Some("https://example.com/pricing")
    );
    // The crawl id is a UUID, which reads as a credential to the redactor,
    // so the fixture carries the replacement and not the id. What matters
    // here is that the field lands typed rather than in the extra map.
    assert_eq!(meta.crawl_id.as_deref(), Some("REDACTED"));
    assert_eq!(meta.embedding.as_ref().map(Vec::len), Some(4));
    assert!(meta.extra.is_empty(), "{:?}", meta.extra);

    let vendor = page.costs.vendor.as_ref().expect("a vendor line");
    assert_eq!(vendor.provider.as_deref(), Some("example-vendor"));
    assert_eq!(vendor.route, "example-vendor.unlocker");
    assert_eq!(vendor.billed(), Credits::from_usd(0.003));
    assert_eq!(vendor.attempts, 2);
    assert!(!vendor.byok);
    assert_eq!(page.cost(), Credits::from_usd(0.00321));
    assert!(page.costs.total() > page.costs.sum_of_parts());
    assert_eq!(page.costs.sum_of_parts(), Credits::from_usd(0.00021));
}

/// `multi_format_metadata.json`: a page asked for as markdown and raw at
/// once. The content is keyed by format and so is the metadata, with one
/// whole block per format, and a transcript sits beside them.
#[test]
fn a_multi_format_reply_keeps_one_metadata_block_per_format() {
    let pages = read_reply("multi_format_metadata.json", 100);
    let page = pages.first_ok().expect("a page");
    let spider_cloud_agent::response::Body::Multi(multi) = &page.body else {
        panic!("expected both formats, got {:?}", page.body)
    };
    assert!(multi.markdown.is_some() && multi.raw.is_some());
    let meta = page.metadata.as_ref().expect("metadata");
    assert!(meta.title.is_none(), "a keyed block has no top level title");
    let markdown = meta.for_format("markdown").expect("markdown block");
    assert_eq!(markdown.title.as_deref(), Some("Unboxing the Growth plan"));
    assert_eq!(markdown.embedding.as_deref(), Some(&[0.0402, -0.0117][..]));
    let raw = meta.for_format("raw").expect("raw block");
    assert_eq!(raw.file_size, Some(154));
    assert_eq!(
        meta.extra["yt_transcript"][1]["text"],
        "First, the dashboard."
    );
    assert!(meta.for_format("text").is_none());
}

#[test]
fn the_cache_parameter_reads_and_writes_both_shapes() {
    use spider_cloud_agent::params::{Cache, CacheControl, RequestParams};
    for value in [
        serde_json::json!(true),
        serde_json::json!(false),
        serde_json::json!({"max_age": 60000, "allow_stale": true, "skip_browser": false,
                           "period": "2026-09-16T00:00:00Z", "next_year": 1}),
    ] {
        let cache: Cache = serde_json::from_value(value.clone()).unwrap();
        let params = RequestParams {
            cache: Some(cache),
            ..RequestParams::default()
        };
        assert_eq!(serde_json::to_value(params).unwrap()["cache"], value);
    }
    let Cache::Control(control) = serde_json::from_value::<Cache>(serde_json::json!({
        "max_age": 0, "period": "2026-09-16T00:00:00Z", "next_year": 1
    }))
    .unwrap() else {
        panic!("an object is a control")
    };
    assert_eq!(control.max_age, Some(0));
    assert!(control.allow_stale.is_none());
    assert_eq!(control.period.as_deref(), Some("2026-09-16T00:00:00Z"));
    assert_eq!(control.extra["next_year"], 1);
    assert_eq!(
        serde_json::to_value(Cache::from(CacheControl::default())).unwrap(),
        serde_json::json!({})
    );
    assert_eq!(serde_json::to_value(Cache::from(true)).unwrap(), true);
}

#[test]
fn f1_cookie_map_fixture_decodes_as_a_page() {
    use spider_cloud_agent::client::{RateLimit, Reply};
    use spider_cloud_agent::policy::engine::Reached;
    use spider_cloud_agent::policy::Observed;
    let Reached::Api(status) = Observed::seen(200, None).api else {
        panic!("api status")
    };
    let reply = Reply {
        status,
        rate_limit: RateLimit::default(),
        retry_after: None,
        elapsed: std::time::Duration::ZERO,
        content_type: Some("application/json".into()),
        body: bytes::Bytes::from(fixture("cookie_map.json")),
    };
    let pages = reply
        .read(&url::Url::parse("https://example.com").unwrap(), None)
        .unwrap();
    let page = pages.first_ok().unwrap();
    let cookies = page.cookies.as_ref().unwrap();
    // The session value looked like a credential, so the redactor dropped it.
    // The theme did not, so it survived. Both names came through as a map.
    assert_eq!(cookies.get("session").map(String::as_str), Some("REDACTED"));
    assert_eq!(cookies.get("theme").map(String::as_str), Some("dark"));
    assert_eq!(page.cost(), Credits::from_usd(0.0001));
    assert_eq!(page.text(), Some("<h1>Your account</h1>"));
}

/// One fixture, by file name.
fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Every fixture in the directory, so a new one is covered the moment it lands.
fn every_fixture() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("the fixture directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string();
                out.push((name, std::fs::read_to_string(&path).expect("a fixture")));
            }
        }
    }
    let mut out = Vec::new();
    walk(&dir, &dir, &mut out);
    out.sort();
    out
}

#[test]
fn there_are_fixtures_to_read() {
    // A loop over an empty directory passes and says nothing. This is what stops
    // that reading as a green suite.
    assert!(
        every_fixture().len() >= 8,
        "expected the recorded fixtures to still be here, found {}",
        every_fixture().len()
    );
}

// Red with the closing brace dropped from thrift/product_page.json: the failure
// read "thrift/product_page.json is not json: EOF while parsing an object at
// line 35 column 0".
#[test]
fn every_fixture_is_json() {
    for (name, text) in every_fixture() {
        serde_json::from_str::<serde_json::Value>(&text)
            .unwrap_or_else(|e| panic!("{name} is not json: {e}"));
    }
}

// Red with one example.com in thrift/product_page.json swapped for
// shop.acme-retail-internal.net: the failure named the host and said it is not a
// documentation host.
#[test]
fn every_fixture_names_only_documentation_hosts() {
    // The leak checker enforces this at publish time. Asserting it here means a
    // bad fixture fails the moment it is added rather than at release.
    const ALLOWED: &[&str] = &[
        "example.com",
        "example.org",
        "example.net",
        "httpbin.org",
        "spider.cloud",
    ];
    for (name, text) in every_fixture() {
        for piece in text.split("://").skip(1) {
            let host: String = piece
                .chars()
                .take_while(|c| *c != '/' && *c != '"' && *c != ':')
                .collect();
            assert!(
                ALLOWED
                    .iter()
                    .any(|a| host == *a || host.ends_with(&format!(".{a}"))),
                "{name} names the host {host}, which is not a documentation host"
            );
        }
    }
}

#[test]
fn a_markdown_page_reads_its_content_metadata_and_costs() {
    let value: serde_json::Value =
        serde_json::from_str(&fixture("scrape_markdown.json")).expect("json");
    let page = &value.as_array().expect("an array")[0];

    let content: MultiBody =
        serde_json::from_value(page["content"].clone()).expect("the content field");
    assert_eq!(content.present(), vec!["markdown"]);
    assert!(content.as_str().expect("text").starts_with("# Pricing"));
    assert!(!content.is_empty());

    let metadata: Metadata =
        serde_json::from_value(page["metadata"].clone()).expect("the metadata field");
    assert_eq!(metadata.title.as_deref(), Some("Pricing"));
    assert_eq!(metadata.domain.as_deref(), Some("example.com"));
    assert!(!metadata.is_empty());

    // The cost block is in dollars, so 0.6 on the wire is 6,000 credits.
    let costs: Costs = serde_json::from_value(page["costs"].clone()).expect("the costs field");
    assert_eq!(costs.total(), Credits(6_000.0));
    assert_eq!(costs.sum_of_parts(), Credits(6_000.0));
}

#[test]
fn a_crawl_carries_a_refusal_and_a_blank_alongside_a_page() {
    let value: serde_json::Value =
        serde_json::from_str(&fixture("crawl_mixed.json")).expect("json");
    let pages = value.as_array().expect("an array");
    assert_eq!(pages.len(), 3);

    let statuses: Vec<u64> = pages
        .iter()
        .map(|p| p["status"].as_u64().expect("a status"))
        .collect();
    assert_eq!(statuses, vec![200, 403, 200]);

    // The third is the failure that looks like a success: a 200 with nothing in
    // it.
    let blank: MultiBody = serde_json::from_value(pages[2]["content"].clone()).expect("content");
    assert!(blank.is_empty());
}

#[test]
fn a_refusal_carries_the_cost_it_still_burned() {
    let value: serde_json::Value =
        serde_json::from_str(&fixture("blocked_page.json")).expect("json");
    assert_eq!(value["status"].as_u64(), Some(403));
    let costs: Costs = serde_json::from_value(value["costs"].clone()).expect("costs");
    assert_eq!(costs.total(), Credits(5_000.0));
}

#[test]
fn a_picture_reads_as_bytes_rather_than_as_text() {
    let value: serde_json::Value = serde_json::from_str(&fixture("screenshot.json")).expect("json");
    let content: MultiBody =
        serde_json::from_value(value[0]["content"].clone()).expect("the content field");
    assert_eq!(content.present(), vec!["screenshot"]);
    assert_eq!(content.as_str(), None);
    assert_eq!(content.screenshot.as_ref().expect("bytes").len(), 8);
}

#[test]
fn several_formats_at_once_all_arrive() {
    let value: serde_json::Value =
        serde_json::from_str(&fixture("multi_format.json")).expect("json");
    let content: MultiBody =
        serde_json::from_value(value[0]["content"].clone()).expect("the content field");
    assert_eq!(content.present(), vec!["raw", "text", "markdown"]);
    // The most processed form wins when several are present.
    assert_eq!(content.as_str(), Some("Herman Melville"));
}

#[test]
fn a_result_list_reads_into_search_results() {
    let results: SearchResults =
        serde_json::from_str(&fixture("search_results.json")).expect("search results");
    assert_eq!(results.len(), 2);
    assert_eq!(
        results.entries()[0].title.as_deref(),
        Some("Example Domain")
    );
    assert_eq!(results.urls().len(), 2);
    assert_eq!(results.urls()[0].as_str(), "https://example.com/");
}

/// The two endpoints do not agree on a unit, and the crate has to. A fetch
/// reports dollars, `/data/credits` reports credits, and reading the first as
/// the second is what made every cost the crate printed 10,000 times too small.
#[test]
fn a_cost_and_a_balance_arrive_in_different_units() {
    let costs: Costs = serde_json::from_str(r#"{"total_cost":1.0}"#).expect("costs");
    assert_eq!(costs.total_cost, Usd::new(1.0));
    assert_eq!(costs.total(), Credits(10_000.0));

    let value: serde_json::Value = serde_json::from_str(&fixture("credits.json")).expect("json");
    let balance = value["data"]["credits"].as_f64().expect("a number");
    assert_eq!(Credits(balance).to_usd(), 12.50005);
}

#[test]
fn a_balance_reads_out_of_its_wrapper() {
    let value: serde_json::Value = serde_json::from_str(&fixture("credits.json")).expect("json");
    let credits = value["data"]["credits"].as_f64().expect("a number");
    assert_eq!(Credits(credits), Credits(125_000.5));
    assert_eq!(Credits(credits).to_usd(), 12.50005);
}

#[test]
fn crawl_records_stay_json_because_their_columns_are_the_services_own() {
    let value: serde_json::Value = serde_json::from_str(&fixture("crawl_logs.json")).expect("json");
    let rows = value["data"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["domain"].as_str(), Some("example.com"));
}

// Red with the walker put back to the starting commit's flat read_dir, which
// skipped subdirectories: the nested list came back empty against the two names
// below.
#[test]
fn the_nested_fixture_inventory_is_included() {
    let nested: Vec<_> = every_fixture()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with("thrift/"))
        .collect();
    assert_eq!(
        nested,
        ["thrift/crawl_widgets.json", "thrift/product_page.json"]
    );
}
