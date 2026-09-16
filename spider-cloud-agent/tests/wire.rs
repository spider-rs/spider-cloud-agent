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

//! The send loop against a socket that answers the way the service does.
//!
//! Every case here was written from a live call that went wrong, and every one
//! of them needs a wire: what is being asked is how many requests went out, what
//! the request said, and what the client did with the answer. The policy
//! simulator in `tests/policy_sim.rs` cannot see any of that, because nothing
//! there sends anything.
//!
//! The stub is a thread and a `TcpListener`. It reads one request, answers from
//! a script, and hands the request line and the request body back down a channel
//! so a test can assert where the call went and what it asked for as well as
//! what came back.

use spider_cloud_agent::ops::transform::Document;
use spider_cloud_agent::params::ReturnFormat;
use spider_cloud_agent::{Body, Credits, Spider};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use url::Url;

/// A stub of the service.
///
/// `seen` carries one entry per request, in order, so a test can assert what
/// went out. Locks are disallowed in this workspace, so the requests come back
/// down a channel rather than out of a shared vector.
struct Stub {
    base: Url,
    seen: Receiver<Sent>,
}

/// One request the stub answered.
struct Sent {
    /// The verb and the path, as the first line of the request carried them.
    line: String,
    /// The body, which is what the request asked for.
    body: String,
}

impl Stub {
    /// Every request that was sent, in order.
    fn sent(&self) -> Vec<Sent> {
        self.seen.try_iter().collect()
    }

    /// The bodies of the requests that were sent, in order.
    fn requests(&self) -> Vec<String> {
        self.sent().into_iter().map(|one| one.body).collect()
    }
}

/// Start a stub that answers each request from `script`.
///
/// Once the script runs out the last answer is repeated, so a test that expects
/// one request still gets somewhere to escalate to when the fix it pins is
/// broken, and the count is what fails rather than the connection.
fn serve(script: &[&str]) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("an address");
    let script: Vec<String> = script.iter().map(|body| (*body).to_string()).collect();
    let (sender, seen) = channel();

    std::thread::spawn(move || {
        for (answered, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { break };
            let Some(request) = read_request(&mut stream) else {
                break;
            };
            if sender.send(request).is_err() {
                break;
            }
            let reply = script
                .get(answered)
                .or_else(|| script.last())
                .map(String::as_str)
                .unwrap_or("[]");
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                reply.len()
            );
            if stream.write_all(head.as_bytes()).is_err() {
                break;
            }
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });

    Stub {
        base: Url::parse(&format!("http://{address}")).expect("a base url"),
        seen,
    }
}

/// Read one request and hand back its request line and its body.
fn read_request(stream: &mut TcpStream) -> Option<Sent> {
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
    Some(Sent {
        line: first.trim_end().to_string(),
        body: String::from_utf8(body).ok()?,
    })
}

/// A client pointed at the stub.
fn client(stub: &Stub) -> Spider {
    Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .build()
        .expect("a client")
}

/// What the links endpoint sends: addresses, and no page at all.
const LINKS_ANSWER: &str = r#"[{"url":"https://example.com/","status":200,
    "links":["https://www.iana.org/domains/example"],
    "costs":{"total_cost":0.00000087}}]"#;

/// The bug this pins: the links endpoint answers with no body, the send loop
/// read that as a blank page, and the walk climbed the whole ladder against an
/// answer it already had. Measured live on 2026-09-15 before the fix: five
/// attempts, 164 s and 0.4446 credits, ending in an error, for links the first
/// attempt returned in 820 ms for 0.0087.
///
/// The count is the assertion. A version of this test that only checked the
/// links came back passed against the bug.
#[tokio::test]
async fn the_links_endpoint_is_one_call_and_not_a_walk_up_the_ladder() {
    let stub = serve(&[LINKS_ANSWER]);
    let spider = client(&stub);

    let outcome = spider
        .links("https://example.com")
        .send()
        .await
        .expect("the links");

    assert_eq!(
        outcome.attempts.len(),
        1,
        "the links endpoint was asked {} times for one answer",
        outcome.attempts.len()
    );
    assert_eq!(outcome.value.len(), 1);
    let requests = stub.requests();
    assert_eq!(requests.len(), 1, "requests: {requests:?}");
    // And the reason it is one call: the request says a body is not wanted.
    assert!(
        requests[0].contains(r#""return_format":"empty""#),
        "the request never said it wanted no body: {}",
        requests[0]
    );
}

/// A need the caller stated is theirs, and defaulting one must not overwrite it.
#[tokio::test]
async fn a_need_the_caller_stated_survives_the_links_default() {
    let stub = serve(&[LINKS_ANSWER]);
    let spider = client(&stub);

    let outcome = spider
        .links("https://example.com")
        .need(spider_cloud_agent::Need::Metadata)
        .send_all()
        .await
        .expect("the links");

    assert_eq!(outcome.attempts.len(), 1);
    let requests = stub.requests();
    assert!(
        requests[0].contains(r#""metadata":true"#),
        "the stated need was dropped: {}",
        requests[0]
    );
}

/// The bug this pins: the transform endpoint answers with one object whose
/// `content` is a list of converted documents, one string per document sent.
/// Read as a page body that list asked for bytes, and the call failed with
/// `invalid type: string "# Title\nBody text.", expected u8`. Measured live on
/// 2026-09-15: every transform returned [`spider_cloud_agent::Error::Decode`].
#[tokio::test]
async fn a_converted_document_is_readable_rather_than_a_decode_error() {
    let stub = serve(&[r##"{"content":["# Title\nBody text."]}"##]);
    let spider = client(&stub);

    let outcome = spider
        .transform(vec![Document::html("<h1>Title</h1><p>Body text.</p>")])
        .format(ReturnFormat::Markdown)
        .send()
        .await
        .expect("the converted document");

    assert_eq!(
        outcome.value.body,
        Body::Markdown("# Title\nBody text.".into())
    );
}

/// Several documents in, several results out, rather than one page holding a
/// list.
#[tokio::test]
async fn every_converted_document_comes_back_as_its_own_result() {
    let stub = serve(&[r##"{"content":["# One","# Two"],"costs":{"total_cost":0.0001}}"##]);
    let spider = client(&stub);

    let outcome = spider
        .transform(vec![
            Document::html("<h1>One</h1>"),
            Document::html("<h1>Two</h1>"),
        ])
        .format(ReturnFormat::Markdown)
        .send_all()
        .await
        .expect("the converted documents");

    let texts: Vec<&str> = outcome.value.ok().filter_map(|page| page.text()).collect();
    assert_eq!(texts, vec!["# One", "# Two"]);
    // The wire sent one cost block for the call, so the call is charged once.
    assert_eq!(outcome.value.total_cost(), Credits::from_usd(0.0001));
}

/// The bug this pins: asked for nothing in particular, the screenshot endpoint
/// answers with the image as base64 text, so the body was a string, nothing
/// ever became [`Body::Screenshot`], and a caller asking for a picture got
/// base64 in a file named `.png`. Measured live on 2026-09-15: a 22,960
/// character text body.
///
/// The stub answers the way the service does, base64 for a request that named no
/// format and bytes for one that asked for bytes, so what the test really pins
/// is the request.
#[tokio::test]
async fn a_screenshot_asks_for_bytes_and_comes_back_a_picture() {
    // "png" as the service would send it, once asked properly.
    let stub = serve(&[r#"[{"url":"https://example.com/","status":200,
        "content":[137,80,78,71]}]"#]);
    let spider = client(&stub);

    let outcome = spider
        .screenshot("https://example.com")
        .send()
        .await
        .expect("the picture");

    let requests = stub.requests();
    assert!(
        requests[0].contains(r#""return_format":"bytes""#),
        "the request never asked for bytes: {}",
        requests[0]
    );
    match &outcome.value.body {
        Body::Screenshot(bytes) => assert_eq!(&bytes[..], &[137, 80, 78, 71]),
        other => panic!("a picture arrived as {other:?}"),
    }
}

/// The bug this pins: an answer that is not a page recorded a cost of zero
/// whatever the body said, so a search spent nothing as far as the report and
/// the budget were concerned.
///
/// The live service sends no cost block with a result list, which is its own
/// gap and not this one. What is fixed here is that the client reports whatever
/// does arrive.
#[tokio::test]
async fn a_search_reports_what_the_service_said_it_cost() {
    let stub = serve(&[
        r#"{"content":[{"title":"Example","url":"https://example.com/"}],
            "costs":{"total_cost":0.001,"compute_cost":0.001}}"#,
    ]);
    let spider = client(&stub);

    let outcome = spider
        .search("example")
        .limit(1)
        .send()
        .await
        .expect("results");

    assert_eq!(outcome.value.len(), 1);
    assert_eq!(outcome.cost, Credits::from_usd(0.001));
    assert_eq!(outcome.attempts[0].cost, Credits::from_usd(0.001));
}

/// A body that says nothing about cost still reads, and reports nothing spent
/// rather than guessing.
#[tokio::test]
async fn a_search_with_no_cost_block_reports_nothing_rather_than_failing() {
    let stub = serve(&[r#"{"content":[{"title":"Example","url":"https://example.com/"}]}"#]);
    let spider = client(&stub);

    let outcome = spider.search("example").send().await.expect("results");

    assert_eq!(outcome.cost, Credits::ZERO);
}

/// One page, the way the service answers a scrape and a fetch alike.
const PAGE_ANSWER: &str = r#"[{"url":"https://example.com/","status":200,
    "content":"<h1>Example</h1>","costs":{"total_cost":0.0000009}}]"#;

/// The bug this pins: the builder named `fetch` posted to `/scrape`, so the
/// operation did not do what its name said and `/fetch` could not be reached at
/// all. The assertion is the request line, because a version of this test that
/// only checked the call succeeded passed against the bug.
#[tokio::test]
async fn scrape_posts_to_the_scrape_path() {
    let stub = serve(&[PAGE_ANSWER]);
    let spider = client(&stub);

    spider
        .scrape("https://example.com")
        .send()
        .await
        .expect("the page");

    let sent = stub.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].line, "POST /scrape HTTP/1.1");
}

/// The other half: `fetch` names the host and the path in the address, and the
/// verb is a post. A get to `/fetch` is answered with a 400 before the handler
/// is reached, which is why the route as declared on 0.1.x never worked.
#[tokio::test]
async fn fetch_posts_to_the_fetch_path_with_the_target_in_it() {
    let stub = serve(&[PAGE_ANSWER]);
    let spider = client(&stub);

    spider
        .fetch("example.com", "/docs/index.html")
        .send()
        .await
        .expect("the page");

    let sent = stub.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].line,
        "POST /fetch/example.com/docs/index.html HTTP/1.1"
    );
}

/// A path handed in with a leading slash renders the same address as one
/// without, because the route template already carries that slash and a doubled
/// one is a different page.
#[tokio::test]
async fn a_leading_slash_on_the_fetch_path_is_not_doubled() {
    let stub = serve(&[PAGE_ANSWER, PAGE_ANSWER]);
    let spider = client(&stub);

    spider.fetch("example.com", "/").send().await.expect("one");
    spider.fetch("example.com", "").send().await.expect("two");

    let sent = stub.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].line, "POST /fetch/example.com/ HTTP/1.1");
    assert_eq!(sent[1].line, sent[0].line);
}

/// The service refuses a `url` override on `/fetch` outright, so sending one is
/// bytes paid for and dropped. The target is the address and nowhere else.
#[tokio::test]
async fn a_fetch_sends_no_url_field() {
    let stub = serve(&[PAGE_ANSWER]);
    let spider = client(&stub);

    spider
        .fetch("example.com", "/")
        .send()
        .await
        .expect("the page");

    let requests = stub.requests();
    assert!(
        !requests[0].contains(r#""url""#),
        "the target went in the body as well as the path: {}",
        requests[0]
    );
}

/// The curated settings reach the wire on a fetch, where the service reads them
/// as overrides on top of the config it holds for that path.
#[tokio::test]
async fn a_fetch_carries_the_settings_as_overrides() {
    let stub = serve(&[PAGE_ANSWER]);
    let spider = client(&stub);

    spider
        .fetch("example.com", "/")
        .mode(spider_cloud_agent::RequestMode::Browser)
        .send()
        .await
        .expect("the page");

    let requests = stub.requests();
    assert!(
        requests[0].contains(r#""request":"browser""#),
        "the setting never reached the wire: {}",
        requests[0]
    );
}

/// A page and the links on it, the way the service answers a scrape that asked
/// for both.
const PAGE_AND_LINKS_ANSWER: &str = r##"[{"url":"https://example.com/","status":200,
    "content":"# Example\nBody text.",
    "links":["https://example.com/a","https://example.com/b"],
    "costs":{"total_cost":0.00035305315}}]"##;

/// The gap this closes: links were reachable only from the links operation, so
/// a caller who wanted a page and its links paid for two calls. Measured live
/// on 2026-09-15 against the service, one scrape asking for both came back with
/// 6,365 bytes of markdown and 91 links for 0.00035305315 credits.
///
/// The assertions are the request line and the request body. A version of this
/// that only read the links off the answer passed while the links still cost a
/// second call.
#[tokio::test]
async fn a_page_and_its_links_come_back_in_one_call() {
    let stub = serve(&[PAGE_AND_LINKS_ANSWER]);
    let spider = client(&stub);

    let outcome = spider
        .scrape("https://example.com")
        .need(spider_cloud_agent::Need::Markdown)
        .page_links(true)
        .send()
        .await
        .expect("the page");

    let sent = stub.sent();
    assert_eq!(sent.len(), 1, "the links cost a second call");
    // The links are asked for without the request being moved to the links
    // endpoint, which would have dropped the content.
    assert_eq!(sent[0].line, "POST /scrape HTTP/1.1");
    assert!(
        sent[0].body.contains(r#""return_page_links":true"#),
        "the request never asked for the links: {}",
        sent[0].body
    );
    assert!(
        sent[0].body.contains(r#""return_format":"markdown""#),
        "asking for links suppressed the body: {}",
        sent[0].body
    );

    assert_eq!(outcome.value.text(), Some("# Example\nBody text."));
    let links = outcome.value.links.as_deref().expect("the links");
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].as_str(), "https://example.com/a");
}

/// What the report counts. Links handed to the caller are payload, and a report
/// that ignored them called a request that returned ninety addresses a total
/// saving, the same way the body-only count did before extractions and metadata
/// were added to it.
#[tokio::test]
async fn the_report_counts_the_links_it_handed_back() {
    let stub = serve(&[PAGE_AND_LINKS_ANSWER]);
    let spider = client(&stub);

    let outcome = spider
        .scrape("https://example.com")
        .need(spider_cloud_agent::Need::Markdown)
        .page_links(true)
        .send()
        .await
        .expect("the page");

    let text = outcome.value.text().unwrap_or_default().len();
    let addresses: usize = outcome
        .value
        .links
        .iter()
        .flatten()
        .map(|link| link.as_str().len())
        .sum();
    assert!(addresses > 0);
    assert_eq!(
        outcome.thrift.returned_bytes,
        text + addresses,
        "the report counted {} bytes for {text} of text and {addresses} of links",
        outcome.thrift.returned_bytes
    );
}

/// The links operation still answers in one call, and the whole set is readable
/// off the pages.
#[tokio::test]
async fn the_links_operation_still_answers_from_the_links_endpoint() {
    let stub = serve(&[LINKS_ANSWER]);
    let spider = client(&stub);

    let outcome = spider
        .links("https://example.com")
        .send_all()
        .await
        .expect("the links");

    let sent = stub.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].line, "POST /links HTTP/1.1");
    assert_eq!(outcome.value.links().len(), 1);
}

/// The three spellings the service reads. An unrecognised value becomes plain
/// HTTP at the service without saying so, so what goes out has to be exact.
#[tokio::test]
async fn every_request_mode_reaches_the_wire_with_the_spelling_the_service_reads() {
    use spider_cloud_agent::RequestMode;

    for (mode, spelling) in [
        (RequestMode::Http, r#""request":"http""#),
        (RequestMode::Smart, r#""request":"smart""#),
        (RequestMode::Browser, r#""request":"browser""#),
    ] {
        let stub = serve(&[PAGE_ANSWER]);
        let spider = client(&stub);

        spider
            .scrape("https://example.com")
            .mode(mode)
            .send()
            .await
            .expect("the page");

        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].contains(spelling),
            "{mode:?} reached the wire as {}",
            requests[0]
        );
    }
}

/// The router fills in what the caller left alone and never argues with what
/// they set. Three modes against one address, and the router has one answer, so
/// two of the three prove the caller won.
#[tokio::test]
async fn the_router_never_argues_with_a_mode_the_caller_named() {
    use spider_cloud_agent::RequestMode;

    let stub = serve(&[PAGE_ANSWER]);
    let spider = client(&stub);
    spider
        .scrape("https://example.com")
        .send()
        .await
        .expect("the page");
    let routed = stub.requests().remove(0);

    let mut differed = 0;
    for (mode, spelling) in [
        (RequestMode::Http, r#""request":"http""#),
        (RequestMode::Smart, r#""request":"smart""#),
        (RequestMode::Browser, r#""request":"browser""#),
    ] {
        let stub = serve(&[PAGE_ANSWER]);
        let spider = client(&stub);
        spider
            .scrape("https://example.com")
            .mode(mode)
            .send()
            .await
            .expect("the page");
        let sent = stub.requests().remove(0);
        assert!(sent.contains(spelling), "{mode:?} went out as {sent}");
        if !routed.contains(spelling) {
            differed += 1;
        }
    }
    assert_eq!(differed, 2, "the router answered {routed}");
}

/// The bug this pins: every rung of the ladder renders the page, and the rung
/// was written over the request after the caller's own settings, so a caller
/// who asked for plain HTTP to hold the bill down was sent up to a browser and
/// billed for it. The router already respected the same pin, which is what made
/// the gap easy to miss.
#[tokio::test]
async fn an_escalation_never_argues_with_a_mode_the_caller_named() {
    use spider_cloud_agent::{Budget, RequestMode};

    // A page the site refused, which is what sends the walk up the ladder.
    let refused = r#"[{"url":"https://example.com/","status":403,"content":"",
        "costs":{"total_cost":0.0001}}]"#;
    let stub = serve(&[refused]);
    let spider = client(&stub);

    let _ = spider
        .scrape("https://example.com")
        .mode(RequestMode::Http)
        .budget(Budget::default().with_attempts(3))
        .send()
        .await;

    let requests = stub.requests();
    assert!(
        requests.len() > 1,
        "nothing escalated, so nothing was proved: {requests:?}"
    );
    for (step, request) in requests.iter().enumerate() {
        assert!(
            request.contains(r#""request":"http""#),
            "attempt {step} went out as {request}"
        );
    }
}
