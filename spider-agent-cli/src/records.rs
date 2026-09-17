//! The records this tool writes.
//!
//! Built by hand rather than derived off the library types, because the shape
//! is a contract a calling agent parses and the library types are free to
//! change underneath it. Every record carries a `type` field, and `schema`
//! prints the same list this module builds.

use serde_json::{json, Map, Value};
use url::Url;

// Every routing type reaches here through the client rather than through
// spider-route itself. The client re-exports them, and one dependency cannot
// drift out of step with another.
use spider_cloud_agent::auth::router::StoredRouter;
use spider_cloud_agent::response::{Body, FailedPage, Hint, Page, SearchEntry};
use spider_cloud_agent::thrift::ThriftReport;
use spider_cloud_agent::{Credits, RouteDecision};

use crate::emit::Item;

// What a served page and a refused one both carry: the body in the shape
// asked for, what came with it, how long it took and what it cost. One macro
// rather than a trait because the two page types share field names and
// nothing else, and the record is a hand-built contract, not a view of them.
macro_rules! shared_fields {
    ($value:expr, $page:expr, $shape:expr) => {{
        let value: &mut Map<String, Value> = $value;
        let page = $page;
        let shape: &BodyShape = $shape;
        value.insert("content".into(), json!(shape.kind));
        value.insert(
            "body".into(),
            match &shape.bytes {
                // Bytes are never inlined. A picture belongs in a file, and a
                // caller that has to decode one out of a log line is being
                // charged a third more for the base64.
                Some(_) => Value::Null,
                // Fields alone go under `fields`. Fields beside a page keep
                // the page here and the fields there.
                None if matches!(page.body, Body::Fields(_)) => Value::Null,
                None => shape.text.clone().map(Value::String).unwrap_or(Value::Null),
            },
        );
        if let Some(fields) = &shape.fields {
            value.insert("fields".into(), fields.clone());
        }
        if let Some(metadata) = &page.metadata {
            if let Ok(encoded) = serde_json::to_value(metadata) {
                value.insert("metadata".into(), encoded);
            }
        }
        if let Some(links) = &page.links {
            value.insert(
                "links".into(),
                Value::Array(links.iter().map(|l| json!(l.as_str())).collect()),
            );
        }
        if let Some(headers) = &page.headers {
            value.insert("headers".into(), json!(headers));
        }
        if let Some(cookies) = &page.cookies {
            value.insert("cookies".into(), json!(cookies));
        }
        if let Some(data) = &page.json_data {
            value.insert("json_data".into(), data.clone());
        }
        if let Some(data) = &page.request_map {
            value.insert("request_map".into(), data.clone());
        }
        if let Some(data) = &page.response_map {
            value.insert("response_map".into(), data.clone());
        }
        if let Some(data) = &page.trace {
            value.insert("trace".into(), data.clone());
        }
        value.insert("bytes".into(), json!(page.body.len()));
        value.insert(
            "duration_ms".into(),
            json!(page.duration.map(|d| d.as_millis() as u64)),
        );
        value.insert(
            "call_elapsed_ms".into(),
            json!(page.call_elapsed.as_millis() as u64),
        );
        value.insert("cost_credits".into(), json!(page.costs.total().get()));
        if let Some(vendor) = &page.costs.vendor {
            value.insert("vendor".into(), vendor_record(vendor));
        }
    }};
}

/// The vendor line of a bill, in the units the rest of the record uses.
fn vendor_record(vendor: &spider_cloud_agent::response::costs::VendorCosts) -> Value {
    json!({
        "provider": vendor.provider,
        "route": vendor.route,
        "vendor_cost_credits": vendor.charged_by_vendor().get(),
        "billed_credits": vendor.billed().get(),
        "byok": vendor.byok,
        "attempts": vendor.attempts,
    })
}

/// What a body is, and what it is worth writing to a file as.
struct BodyShape {
    kind: &'static str,
    text: Option<String>,
    bytes: Option<Vec<u8>>,
    extension: &'static str,
    fields: Option<Value>,
}

/// Read a body into the parts a record and a file both need.
fn shape(body: &Body) -> BodyShape {
    let plain = |kind: &'static str, text: &str, extension: &'static str| BodyShape {
        kind,
        text: Some(text.to_string()),
        bytes: None,
        extension,
        fields: None,
    };
    match body {
        Body::Text(text) => plain("text", text, "txt"),
        Body::Markdown(text) => plain("markdown", text, "md"),
        Body::Html(text) => plain("html", text, "html"),
        Body::Xml(text) => plain("xml", text, "xml"),
        Body::WithFields { content, fields } => {
            let mut shaped = shape(content);
            shaped.fields = serde_json::to_value(fields).ok();
            shaped
        }
        Body::Bytes(bytes) => BodyShape {
            kind: "bytes",
            text: None,
            bytes: Some(bytes.to_vec()),
            extension: "bin",
            fields: None,
        },
        Body::Screenshot(bytes) => BodyShape {
            kind: "screenshot",
            text: None,
            bytes: Some(bytes.to_vec()),
            extension: "png",
            fields: None,
        },
        Body::Fields(fields) => {
            let value = Value::Object(fields.clone().into_iter().collect::<Map<String, Value>>());
            BodyShape {
                kind: "fields",
                text: serde_json::to_string(&value).ok(),
                bytes: None,
                extension: "json",
                fields: Some(value),
            }
        }
        Body::Multi(multi) => BodyShape {
            kind: "multi",
            text: multi.as_str().map(str::to_string),
            bytes: None,
            extension: "txt",
            fields: None,
        },
        Body::Empty => BodyShape {
            kind: "empty",
            text: None,
            bytes: None,
            extension: "txt",
            fields: None,
        },
        // New body shapes arrive with the API. An unknown one is still a body
        // with a length, which is better than refusing to print it.
        _ => BodyShape {
            kind: "other",
            text: None,
            bytes: None,
            extension: "bin",
            fields: None,
        },
    }
}

/// A served page.
pub fn page(page: &Page) -> Item {
    let shape = shape(&page.body);
    let mut value = Map::new();
    value.insert("type".into(), json!("page"));
    value.insert("url".into(), json!(page.url.as_str()));
    value.insert("status".into(), json!(page.status.code()));
    if let Some(error) = &page.error {
        value.insert("error".into(), json!(error));
    }
    shared_fields!(&mut value, page, &shape);

    let mut item = Item {
        value: Value::Object(value),
        text: shape.text,
        bytes: shape.bytes,
        url: Some(page.url.clone()),
        extension: shape.extension,
    };
    if item.bytes.is_some() {
        item.text = None;
    }
    item
}

/// A page the site did not serve.
///
/// The site's answer is in the record the same way it is on a served page,
/// because a block page and a login wall are told apart by reading them. A
/// refused body still goes to a file under `--output-dir`, named as the page
/// would have been.
pub fn failed(failed: &FailedPage) -> Item {
    let shape = shape(&failed.body);
    let mut value = Map::new();
    value.insert("type".into(), json!("failed"));
    value.insert("url".into(), json!(failed.url.as_str()));
    value.insert("status".into(), json!(failed.status.code()));
    value.insert("error".into(), json!(failed.error));
    value.insert("hint".into(), json!(hint(failed.hint)));
    value.insert("billed".into(), json!(failed.was_billed()));
    shared_fields!(&mut value, failed, &shape);

    let mut item = Item {
        value: Value::Object(value),
        text: shape.text,
        bytes: shape.bytes,
        url: Some(failed.url.clone()),
        extension: shape.extension,
    };
    if item.bytes.is_some() {
        item.text = None;
    }
    item
}

/// What to change before asking for the same page again, as a stable name.
pub fn hint(hint: Hint) -> &'static str {
    match hint {
        Hint::TryBrowser => "try_browser",
        Hint::TryResidentialProxy => "try_residential_proxy",
        Hint::TryDifferentCountry => "try_different_country",
        Hint::NeedsSession => "needs_session",
        Hint::Permanent => "permanent",
        Hint::SlowDown { .. } => "slow_down",
        _ => "unknown",
    }
}

/// One link found on a page.
///
/// A record per link rather than a page record with a list in it, because the
/// answer to `links` is the addresses, and one per line is what a caller pipes
/// into the next command.
pub fn link(found: &Url, from: &Url) -> Item {
    let value = json!({
        "type": "link",
        "url": found.as_str(),
        "from": from.as_str(),
    });
    Item::structured(value)
        .with_text(found.as_str().to_string(), "txt")
        .with_url(found.clone())
}

/// One search result.
pub fn result(entry: &SearchEntry) -> Item {
    let value = json!({
        "type": "result",
        "url": entry.url,
        "title": entry.title,
        "description": entry.description,
    });
    let item = Item::structured(value).with_text(entry.url.clone(), "txt");
    match entry.url() {
        Some(url) => item.with_url(url),
        None => item,
    }
}

/// One row off the account.
pub fn row(row: &Value) -> Item {
    let value = json!({ "type": "row", "data": row });
    let text = serde_json::to_string(row).unwrap_or_default();
    Item::structured(value).with_text(text, "json")
}

/// What the router decided, with nothing sent.
pub fn route(url: &Url, decision: &RouteDecision) -> Item {
    let value = json!({
        "type": "route",
        "url": url.as_str(),
        "mode": decision.mode().as_str(),
        "proxy": decision.proxy().as_str(),
        "wait_ms": decision.wait().millis(),
        "country": decision.country.as_ref().map(|c| c.as_str()),
        "start_rung": decision.start_rung,
        "confidence": decision.confidence,
        "source": source(decision),
    });
    let text = format!(
        "{}\t{}\t{}",
        url.as_str(),
        decision.mode().as_str(),
        decision.proxy().as_str()
    );
    Item::structured(value)
        .with_text(text, "txt")
        .with_url(url.clone())
}

/// Shown in place of every key and every provider setting.
const REDACTED: &str = "<redacted>";

/// The stored provider fallback, with every secret replaced.
///
/// Credential names and provider setting names are kept, because a caller
/// checking what is stored needs them. Every value under them is replaced:
/// a credential is a key, and a provider setting can be one.
pub fn router(stored: Option<&StoredRouter>) -> Item {
    let Some(stored) = stored else {
        return Item::structured(json!({ "type": "router", "stored": false }));
    };
    let router = &stored.router;
    let credentials: Option<Map<String, Value>> = router.credentials.as_ref().map(|held| {
        held.keys()
            .map(|name| (name.clone(), json!(REDACTED)))
            .collect()
    });
    let options: Option<Map<String, Value>> = stored.provider_options.as_ref().map(|held| {
        held.iter()
            .map(|(provider, settings)| {
                let redacted = match settings {
                    Value::Object(map) => Value::Object(
                        map.keys()
                            .map(|key| (key.clone(), json!(REDACTED)))
                            .collect(),
                    ),
                    _ => json!(REDACTED),
                };
                (provider.clone(), redacted)
            })
            .collect()
    });

    let mut lines = Vec::new();
    for (name, value) in [
        ("mode", &router.mode),
        ("provider", &router.provider),
        ("funding", &router.funding),
    ] {
        if let Some(value) = value {
            lines.push(format!("{name}\t{value}"));
        }
    }
    if router.token.is_some() {
        lines.push(format!("token\t{REDACTED}"));
    }
    for name in credentials.iter().flat_map(Map::keys) {
        lines.push(format!("credential\t{name}\t{REDACTED}"));
    }
    for (provider, settings) in options.iter().flatten() {
        match settings {
            Value::Object(map) => {
                for key in map.keys() {
                    lines.push(format!("option\t{provider}.{key}\t{REDACTED}"));
                }
            }
            _ => lines.push(format!("option\t{provider}\t{REDACTED}")),
        }
    }

    let value = json!({
        "type": "router",
        "stored": true,
        "mode": router.mode,
        "provider": router.provider,
        "funding": router.funding,
        "token": router.token.as_ref().map(|_| REDACTED),
        "credentials": credentials,
        "provider_options": options,
    });
    Item::structured(value).with_text(lines.join("\n"), "txt")
}

/// Which layer answered the routing question.
fn source(decision: &RouteDecision) -> &'static str {
    use spider_cloud_agent::RouteSource;
    match decision.source {
        RouteSource::Heuristic => "heuristic",
        RouteSource::Model => "model",
        RouteSource::Caller => "caller",
        RouteSource::Memory => "memory",
        RouteSource::Explore => "explore",
        _ => "unknown",
    }
}

/// What a run did and what it cost.
///
/// Always the last record. A caller that reads nothing else can read this one
/// and know whether the run finished, what stopped it, and what it was
/// charged.
pub struct Report {
    /// Addresses the run was given.
    pub targets: usize,
    /// Pages the sites served.
    pub served: usize,
    /// Pages the sites refused.
    pub refused: usize,
    /// Calls made, escalations included.
    pub attempts: usize,
    /// What the run cost.
    pub cost: Credits,
    /// How long it took, end to end.
    pub elapsed_ms: u64,
    /// What the responses weighed and what the trimming took off.
    pub thrift: ThriftReport,
    /// What stopped the run, when something did.
    pub stopped: Option<&'static str>,
}

impl Report {
    /// The record.
    pub fn item(&self) -> Item {
        let value = json!({
            "type": "report",
            "targets": self.targets,
            "served": self.served,
            "refused": self.refused,
            "attempts": self.attempts,
            "cost_credits": self.cost.get(),
            "elapsed_ms": self.elapsed_ms,
            "wire_bytes": self.thrift.wire_bytes,
            "returned_bytes": self.thrift.returned_bytes,
            "approx_tokens_in": self.thrift.approx_tokens_in,
            "approx_tokens_out": self.thrift.approx_tokens_out,
            "stopped": self.stopped,
        });
        Item::structured(value)
    }

    /// The same facts in one line, for a person reading stderr.
    pub fn line(&self) -> String {
        let stopped = match self.stopped {
            Some(reason) => format!(", stopped on {reason}"),
            None => String::new(),
        };
        format!(
            "{} served, {} refused, {} calls, {:.3} credits, {} ms{stopped}",
            self.served, self.refused, self.attempts, self.cost.0, self.elapsed_ms
        )
    }

    /// Add one operation's outcome to the totals.
    pub fn add_thrift(&mut self, thrift: &ThriftReport) {
        self.thrift.wire_bytes += thrift.wire_bytes;
        self.thrift.returned_bytes += thrift.returned_bytes;
        self.thrift.approx_tokens_in += thrift.approx_tokens_in;
        self.thrift.approx_tokens_out += thrift.approx_tokens_out;
    }
}

impl Default for Report {
    fn default() -> Report {
        Report {
            targets: 0,
            served: 0,
            refused: 0,
            attempts: 0,
            cost: Credits::ZERO,
            elapsed_ms: 0,
            thrift: ThriftReport::default(),
            stopped: None,
        }
    }
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;

    /// The record for one reply, read the way the transport reads it.
    fn record_for(reply: serde_json::Value) -> Item {
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
            elapsed: std::time::Duration::from_millis(90),
            content_type: None,
            body: serde_json::to_vec(&reply).unwrap().into(),
        };
        let pages = reply
            .read(&Url::parse("https://example.com").unwrap(), None)
            .unwrap();
        match &pages.0[0] {
            spider_cloud_agent::response::PageResult::Ok(p) => page(p),
            spider_cloud_agent::response::PageResult::Failed(p) => failed(p),
        }
    }

    #[test]
    fn a_served_and_a_refused_page_write_the_same_fields() {
        for code in [200, 403] {
            let item = record_for(json!({
                "url": "https://example.com", "status": code,
                "content": {"markdown": "# Example"},
                "css_extracted": {"title": ["Example"]},
                "duration_elasped_ms": 412, "json_data": {"other_scripts": []},
                "request_map": {"https://example.com/": 0.0},
                "error": "partial",
                "costs": {"total_cost": 0.003, "compute_cost": 0.001,
                    "vendor": {"provider": "vendor", "route": "vendor.unlocker",
                               "vendor_cost": 0.002, "billed_cost": 0.002,
                               "byok": true, "attempts": 1}}
            }));
            let value = &item.value;
            assert_eq!(value["type"], if code == 200 { "page" } else { "failed" });
            assert_eq!(value["content"], "markdown");
            assert_eq!(value["body"], "# Example");
            assert_eq!(value["fields"]["title"][0], "Example");
            assert_eq!(value["duration_ms"], 412);
            assert_eq!(value["call_elapsed_ms"], 90);
            assert_eq!(value["json_data"]["other_scripts"], json!([]));
            assert_eq!(value["request_map"]["https://example.com/"], 0.0);
            assert!(value.get("response_map").is_none());
            assert_eq!(value["error"], "partial");
            assert_eq!(value["cost_credits"], 30.0);
            assert_eq!(value["vendor"]["provider"], "vendor");
            assert_eq!(value["vendor"]["billed_credits"], 20.0);
            assert_eq!(value["vendor"]["byok"], true);
            assert_eq!(item.text.as_deref(), Some("# Example"));
            assert_eq!(item.extension, "md");
        }
    }

    #[test]
    fn a_page_without_extras_writes_none_of_them() {
        let item = record_for(json!({
            "url": "https://example.com", "status": 200,
            "content": {"markdown": "# Example"},
            "costs": {"total_cost": 0.001}
        }));
        let value = &item.value;
        for absent in [
            "error",
            "fields",
            "metadata",
            "links",
            "headers",
            "cookies",
            "json_data",
            "request_map",
            "response_map",
            "trace",
            "vendor",
        ] {
            assert!(value.get(absent).is_none(), "{absent} was written: {value}");
        }
        assert_eq!(value["duration_ms"], Value::Null);
    }

    #[test]
    fn a_refused_page_with_no_body_still_says_so() {
        let item = record_for(json!({
            "url": "https://example.com", "status": 403,
            "content": null, "error": "the site refused the fetch",
            "costs": {"total_cost": 0.001}
        }));
        let value = &item.value;
        assert_eq!(value["type"], "failed");
        assert_eq!(value["content"], "empty");
        assert_eq!(value["body"], Value::Null);
        assert_eq!(value["bytes"], 0);
        assert_eq!(value["hint"], "try_residential_proxy");
        assert!(item.text.is_none());
    }

    #[test]
    fn a_hint_has_a_stable_name() {
        assert_eq!(hint(Hint::TryBrowser), "try_browser");
        assert_eq!(hint(Hint::Permanent), "permanent");
        assert_eq!(
            hint(Hint::SlowDown {
                after: std::time::Duration::from_secs(5)
            }),
            "slow_down"
        );
    }

    #[test]
    fn a_report_says_what_stopped_it() {
        let report = Report {
            served: 2,
            refused: 1,
            attempts: 4,
            stopped: Some("budget"),
            ..Report::default()
        };
        let value = report.item().value;
        assert_eq!(value["type"], json!("report"));
        assert_eq!(value["stopped"], json!("budget"));
        assert_eq!(value["served"], json!(2));
        assert!(
            report.line().contains("stopped on budget"),
            "{}",
            report.line()
        );
    }

    #[test]
    fn a_finished_run_reports_no_stop() {
        let value = Report::default().item().value;
        assert_eq!(value["stopped"], Value::Null);
    }
}
