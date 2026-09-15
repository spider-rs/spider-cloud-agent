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
use spider_cloud_agent::response::{Body, FailedPage, Hint, Page, SearchEntry};
use spider_cloud_agent::thrift::ThriftReport;
use spider_cloud_agent::{Credits, RouteDecision};

use crate::emit::Item;

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
    value.insert("content".into(), json!(shape.kind));
    value.insert(
        "body".into(),
        match &shape.bytes {
            // Bytes are never inlined. A picture belongs in a file, and a
            // caller that has to decode one out of a log line is being charged
            // a third more for the base64.
            Some(_) => Value::Null,
            None => match &shape.fields {
                Some(_) => Value::Null,
                None => shape.text.clone().map(Value::String).unwrap_or(Value::Null),
            },
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
    value.insert("bytes".into(), json!(page.body.len()));
    value.insert(
        "duration_ms".into(),
        json!(page.duration.as_millis() as u64),
    );
    value.insert("cost_credits".into(), json!(page.cost().get()));

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
pub fn failed(failed: &FailedPage) -> Item {
    let value = json!({
        "type": "failed",
        "url": failed.url.as_str(),
        "status": failed.status.code(),
        "error": failed.error,
        "hint": hint(failed.hint),
        "billed": failed.was_billed(),
        "cost_credits": failed.cost().get(),
    });
    Item::structured(value).with_url(failed.url.clone())
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
