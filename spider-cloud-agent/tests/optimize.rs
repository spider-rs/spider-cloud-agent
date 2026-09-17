#![cfg(feature = "optimize")]
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

//! The optimizer in the send loop, checked on the wire.
//!
//! Every test here asserts on the body a stub of the service received, because
//! an optimizer that decides and then writes nothing looks the same as one that
//! works from anywhere else.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use spider_cloud_agent::optimize::{
    summarize, ApplyMode, ComparisonRecorder, Gate, NoModel, Optimizer, ResourceSource,
    ResourceSummary, Scorer,
};
use spider_cloud_agent::policy::{Budget, ASSUMED_MINIMUM_COST};
use spider_cloud_agent::{Credits, Explorer, Need, ProxyPool, RequestMode, Spider};
use spider_optimize::features::{EDIT_KEY, VALUE_BUCKET};
use spider_optimize::schema::learnable_slot;
use spider_optimize::{Input, Key, ModelVersion, Score, MULTIPLIERS};
use spider_route::{Action, RouteDecision, RouteSource};
use url::Url;

/// A served page, billed one credit.
const SERVED: &str = r#"[{"url":"https://example.com/a","status":200,"content":"a page with something to read on it","costs":{"total_cost":0.0001}}]"#;

/// A page the site refused, billed one credit.
const REFUSED: &str = r#"[{"url":"https://example.com/a","status":403,"content":"denied","costs":{"total_cost":0.0001}}]"#;

/// The third party group a resource source reports.
const TRACKER: &str = "tracker-alpha.example";

/// A stub of the service that answers from a script, repeating the last answer.
struct Stub {
    base: Url,
    seen: Receiver<String>,
}

impl Stub {
    fn serve(script: &[&str]) -> Stub {
        let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0)).expect("a port");
        let address = listener.local_addr().expect("an address");
        let script: Vec<String> = script.iter().map(|body| body.to_string()).collect();
        let (sender, seen) = channel();
        std::thread::spawn(move || {
            for (answered, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { break };
                let Some(body) = read_body(&mut stream) else {
                    break;
                };
                if sender.send(body).is_err() {
                    break;
                }
                let reply = script
                    .get(answered)
                    .unwrap_or_else(|| script.last().unwrap());
                let head = format!(
                    "HTTP/1.1 200 Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    reply.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        Stub {
            base: Url::parse(&format!("http://{address}")).expect("a base url"),
            seen,
        }
    }

    /// Every body received so far, parsed, in order.
    fn bodies(&self) -> Vec<serde_json::Value> {
        self.seen
            .try_iter()
            .map(|body| serde_json::from_str(&body).expect("a JSON body"))
            .collect()
    }
}

fn read_body(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut length = 0usize;
    let mut first = String::new();
    if reader.read_line(&mut first).ok()? == 0 {
        return None;
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// Rows, handed back down a channel since locks are not allowed here.
struct Rows(Sender<String>);

impl ComparisonRecorder for Rows {
    fn observe(&self, row: &str) {
        let _ = self.0.send(row.to_string());
    }

    fn salt(&self) -> u64 {
        0x5eed
    }
}

fn rows() -> (Rows, Receiver<String>) {
    let (sender, receiver) = channel();
    (Rows(sender), receiver)
}

/// A scorer that likes what `favour` ranks, and scores keep plainly.
///
/// A rank of zero is keep's score. A higher rank is cheaper and more likely to
/// work, so the gate prefers it.
struct Favour(fn(&Input) -> u8);

impl Scorer for Favour {
    fn score(&self, input: &Input) -> Score {
        match (self.0)(input) {
            0 => Score {
                p_success: 0.95,
                latency_ms: 900.0,
                credits: 4.0,
                support: 50.0,
            },
            rank => Score {
                p_success: 0.99,
                latency_ms: 500.0,
                credits: 2.0 / f32::from(rank),
                support: 50.0,
            },
        }
    }

    fn version(&self) -> Option<ModelVersion> {
        Some(ModelVersion(1))
    }
}

/// Whether the candidate edits this key.
fn touches(input: &Input, key: Key) -> bool {
    input.edit.get(EDIT_KEY + learnable_slot(key).unwrap()) == Some(1.0)
}

/// How many keys the candidate edits.
fn keys(input: &Input) -> usize {
    (EDIT_KEY..EDIT_KEY + 9)
        .filter(|slot| input.edit.get(*slot) == Some(1.0))
        .count()
}

/// Whether the candidate sets a value in this bucket.
fn bucket(input: &Input, index: usize) -> bool {
    input.edit.get(VALUE_BUCKET + index) == Some(1.0)
}

/// Any edit at all.
fn anything(input: &Input) -> u8 {
    u8::from(keys(input) > 0)
}

/// Blacklist appends, a lone one first.
fn blacklist(input: &Input) -> u8 {
    match (touches(input, Key::NetworkBlacklist), keys(input)) {
        (true, 1) => 2,
        (true, _) => 1,
        _ => 0,
    }
}

/// A browser fetch first, then letting stylesheets through.
fn browser_or_stylesheets(input: &Input) -> u8 {
    if keys(input) != 1 {
        0
    } else if touches(input, Key::Request) && bucket(input, 2) {
        2
    } else if touches(input, Key::BlockStylesheets) && bucket(input, 0) {
        1
    } else {
        0
    }
}

/// One third party group, the same for every page.
struct Tracked;

impl ResourceSource for Tracked {
    fn resources(&self, url: &Url) -> Option<ResourceSummary> {
        let resources = [
            (Url::parse("https://example.com/app.js").unwrap(), 40_000),
            (
                Url::parse(&format!("https://{TRACKER}/t.js")).unwrap(),
                90_000,
            ),
        ];
        Some(summarize(url, &resources))
    }
}

fn client(stub: &Stub) -> spider_cloud_agent::SpiderBuilder {
    Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
}

/// Scrape `url` for markdown through `spider`, letting `adjust` change the
/// request first, and hand back every body the stub received.
async fn scrape(
    spider: Spider,
    stub: &Stub,
    url: &str,
    adjust: impl FnOnce(&mut spider_cloud_agent::RequestParams),
) -> Vec<serde_json::Value> {
    let mut call = spider.scrape(url).need(Need::Markdown);
    adjust(call.params_mut());
    tokio::time::timeout(Duration::from_secs(10), call.send())
        .await
        .expect("the walk hung")
        .expect("a page");
    stub.bodies()
}

const PAGE: &str = "https://example.com/a";

#[tokio::test]
async fn shadow_mode_sends_the_baseline_request_unchanged() {
    let plain = Stub::serve(&[SERVED]);
    let baseline = scrape(client(&plain).build().unwrap(), &plain, PAGE, |_| {}).await;

    let shadowed = Stub::serve(&[SERVED]);
    let (recorder, written) = rows();
    let spider = client(&shadowed)
        .optimizer(Optimizer::shadow(Favour(anything)))
        .comparison_recorder(recorder)
        .build()
        .unwrap();
    let sent = scrape(spider, &shadowed, PAGE, |_| {}).await;
    assert_eq!(sent, baseline);

    let row: serde_json::Value = serde_json::from_str(&written.try_recv().unwrap()).unwrap();
    assert_eq!(row["arm"], "shadow");
    assert!(!row["edit"].is_null(), "the shadowed pick is in the row");
    assert!(written.try_recv().is_err(), "one row per operation");

    // The same scorer in apply mode does change the body, so the equality
    // above is the mode at work and not a scorer that picked nothing.
    let applied = Stub::serve(&[SERVED]);
    let spider = client(&applied)
        .optimizer(Optimizer::new(
            Favour(anything),
            Gate::default(),
            ApplyMode::Apply,
        ))
        .build()
        .unwrap();
    assert_ne!(scrape(spider, &applied, PAGE, |_| {}).await, baseline);
}

#[tokio::test]
async fn apply_mode_writes_only_unpinned_keys() {
    let optimizer = || {
        Optimizer::new(
            Favour(browser_or_stylesheets),
            Gate::default(),
            ApplyMode::Apply,
        )
    };

    let pinned = Stub::serve(&[SERVED]);
    let (recorder, written) = rows();
    let spider = client(&pinned)
        .optimizer(optimizer())
        .comparison_recorder(recorder)
        .build()
        .unwrap();
    let sent = scrape(spider, &pinned, PAGE, |params| {
        params.request = Some(RequestMode::Http);
    })
    .await;
    assert_eq!(sent[0]["request"], "http");
    assert!(sent[0].get("block_stylesheets").is_none(), "{}", sent[0]);
    // A pinned request is never passed to the optimizer, so there is no row.
    assert!(written.try_recv().is_err());

    let open = Stub::serve(&[SERVED]);
    let (recorder, written) = rows();
    let spider = client(&open)
        .optimizer(optimizer())
        .comparison_recorder(recorder)
        .build()
        .unwrap();
    let sent = scrape(spider, &open, PAGE, |_| {}).await;
    assert_eq!(sent[0]["request"], "browser", "{}", sent[0]);
    let row: serde_json::Value = serde_json::from_str(&written.try_recv().unwrap()).unwrap();
    assert_eq!(row["arm"], "candidate");
    assert_eq!(row["edit"]["key"], Key::Request.index());
    assert_eq!(row["multiplier"], 4.0);

    // With the mode taken out of the running, the stylesheet switch is next.
    let rendered = Stub::serve(&[SERVED]);
    let spider = client(&rendered)
        .optimizer(Optimizer::new(
            Favour(|input| {
                u8::from(
                    keys(input) == 1 && touches(input, Key::BlockStylesheets) && bucket(input, 0),
                )
            }),
            Gate::default(),
            ApplyMode::Apply,
        ))
        .build()
        .unwrap();
    let sent = scrape(spider, &rendered, PAGE, |_| {}).await;
    assert_eq!(sent[0]["block_stylesheets"], false, "{}", sent[0]);
}

fn blacklisting(stub: &Stub) -> Spider {
    client(stub)
        .optimizer(
            Optimizer::new(Favour(blacklist), Gate::default(), ApplyMode::Apply)
                .with_resources(Tracked),
        )
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_caller_supplied_blacklist_is_never_appended_to() {
    let listed = Stub::serve(&[SERVED]);
    let sent = scrape(blacklisting(&listed), &listed, PAGE, |params| {
        params.disable_hints = Some(true);
        params.network_blacklist = Some(vec!["caller.example".into()]);
    })
    .await;
    assert_eq!(
        sent[0]["network_blacklist"],
        serde_json::json!(["caller.example"])
    );

    // Without the caller's list the same optimizer does append, so the list
    // above survived because it was the caller's.
    let open = Stub::serve(&[SERVED]);
    let sent = scrape(blacklisting(&open), &open, PAGE, |params| {
        params.disable_hints = Some(true);
    })
    .await;
    assert_eq!(sent[0]["network_blacklist"], serde_json::json!([TRACKER]));
    assert!(sent[0].get("event_tracker").is_none(), "{}", sent[0]);
}

#[tokio::test]
async fn a_blacklist_edit_needs_disable_hints() {
    let hinted = Stub::serve(&[SERVED]);
    let sent = scrape(blacklisting(&hinted), &hinted, PAGE, |_| {}).await;
    assert!(sent[0].get("network_blacklist").is_none(), "{}", sent[0]);

    let quiet = Stub::serve(&[SERVED]);
    let sent = scrape(blacklisting(&quiet), &quiet, PAGE, |params| {
        params.disable_hints = Some(true);
    })
    .await;
    assert_eq!(sent[0]["network_blacklist"], serde_json::json!([TRACKER]));
    // Nothing was switched on to see what the page loads.
    assert!(sent[0].get("event_tracker").is_none(), "{}", sent[0]);
}

#[tokio::test]
async fn an_applied_blacklist_survives_escalation() {
    let stub = Stub::serve(&[REFUSED, SERVED]);
    let sent = scrape(blacklisting(&stub), &stub, PAGE, |params| {
        params.disable_hints = Some(true);
    })
    .await;
    assert_eq!(sent.len(), 2, "{sent:?}");
    for body in &sent {
        assert_eq!(body["network_blacklist"], serde_json::json!([TRACKER]));
        assert!(body.get("event_tracker").is_none(), "{body}");
    }
    assert_eq!(sent[1]["request"], "browser");
}

#[tokio::test]
async fn the_comparison_row_carries_cost_attempts_and_content_labels() {
    // A refusal the ladder climbs from, then the fields. A 402 in either plane
    // ends the walk at once, so the first answer here is a 403.
    let fields = r#"[{"url":"https://example.com/a","status":200,"content":"","css_extracted":{"title":["Hello"],"price":[]},"costs":{"total_cost":0.0002}}]"#;
    let stub = Stub::serve(&[REFUSED, fields]);
    let (recorder, written) = rows();
    let spider = client(&stub)
        .optimizer(Optimizer::new(NoModel, Gate::default(), ApplyMode::Apply))
        .comparison_recorder(recorder)
        .build()
        .unwrap();
    let outcome = spider
        .scrape(PAGE)
        .need(Need::fields([("title", "h1"), ("price", ".price")]))
        .send()
        .await
        .unwrap();
    assert_eq!(outcome.attempts.len(), 2);
    assert_eq!(stub.bodies().len(), 2);

    let row: serde_json::Value = serde_json::from_str(&written.try_recv().unwrap()).unwrap();
    assert_eq!(row["attempts"], 2);
    // Both attempts, as the fixture billed them.
    assert_eq!(
        row["credits"],
        Credits::from_usd(0.0001).get() + Credits::from_usd(0.0002).get()
    );
    assert_eq!(row["fields_requested"], 2);
    assert_eq!(row["fields_present"], 1);
    assert_eq!(row["success"], true);
    assert_eq!(row["status"], "ok");
    assert_eq!(row["need"], "fields");
    assert_eq!(row["arm"], "baseline", "no model keeps the request");
    assert!(row["edit"].is_null());
    for label in ["content_ok", "fields_ok", "shingle_jaccard", "byte_ratio"] {
        assert!(row[label].is_null(), "{label} is the collector's: {row}");
    }
    assert!(written.try_recv().is_err(), "one row per operation");
}

#[tokio::test]
async fn the_row_never_carries_the_host() {
    let stub = Stub::serve(&[SERVED]);
    let (recorder, written) = rows();
    let spider = client(&stub)
        .optimizer(Optimizer::shadow(Favour(anything)).with_resources(Tracked))
        .comparison_recorder(recorder)
        .build()
        .unwrap();
    scrape(
        spider,
        &stub,
        "https://a-very-distinct-host.example/x",
        |params| {
            params.disable_hints = Some(true);
        },
    )
    .await;

    let row = written.try_recv().expect("a row was written");
    serde_json::from_str::<serde_json::Value>(&row).expect("a row is JSON");
    for needle in ["distinct", "tracker", "example", "http", "\n"] {
        assert!(!row.contains(needle), "{needle:?} reached the row: {row}");
    }
}

/// The explorer's prices can only be seen through what a budget lets it pick,
/// so this finds each arm's price by the smallest credit cap that admits it.
#[test]
fn optimizer_multipliers_match_the_explorer_arms() {
    let floor = ASSUMED_MINIMUM_COST.get();
    // No arm repeats this action, so every arm is eligible to be drawn.
    let routed = RouteDecision::new(
        Action::new(RequestMode::Http).with_proxy(ProxyPool::Residential),
        RouteSource::Heuristic,
        0.5,
    );
    let explorer = Explorer::new(1.0);
    let pages: Vec<Url> = (0..600)
        .map(|n| Url::parse(&format!("https://example.com/p/{n}")).unwrap())
        .collect();
    let drawn = |budget: Budget| -> Vec<(RequestMode, ProxyPool, u32)> {
        let mut arms: Vec<_> = pages
            .iter()
            .filter_map(|page| explorer.choose(page, &routed, &budget))
            .map(|chosen| (chosen.mode(), chosen.proxy(), chosen.wait().millis()))
            .collect();
        arms.sort_by_key(|arm| format!("{arm:?}"));
        arms.dedup();
        arms
    };

    let mut table: Vec<_> = MULTIPLIERS
        .iter()
        .map(|(mode, proxy, wait, _)| (*mode, *proxy, *wait))
        .collect();
    table.sort_by_key(|arm| format!("{arm:?}"));
    assert_eq!(
        drawn(Budget::unlimited()),
        table,
        "the explorer and the optimizer price different arms"
    );

    for (mode, proxy, wait, multiplier) in MULTIPLIERS {
        let arm = (mode, proxy, wait);
        let at = Budget::default().with_credits(Credits(floor * f64::from(multiplier)));
        let under = Budget::default().with_credits(Credits(floor * f64::from(multiplier) * 0.999));
        assert!(
            drawn(at).contains(&arm),
            "{arm:?} is dearer than {multiplier}"
        );
        assert!(
            !drawn(under).contains(&arm),
            "{arm:?} is cheaper than {multiplier}"
        );
    }
}
