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

use spider_cloud_agent::response::content::MultiBody;
use spider_cloud_agent::response::costs::Costs;
use spider_cloud_agent::response::metadata::Metadata;
use spider_cloud_agent::response::SearchResults;
use spider_cloud_agent::{Credits, Usd};

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
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("the fixture directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("a file name")
                .to_string();
            out.push((name, std::fs::read_to_string(&path).expect("a fixture")));
        }
    }
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

#[test]
fn every_fixture_is_json() {
    for (name, text) in every_fixture() {
        serde_json::from_str::<serde_json::Value>(&text)
            .unwrap_or_else(|e| panic!("{name} is not json: {e}"));
    }
}

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
