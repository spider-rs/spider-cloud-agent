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

use spider_cloud_agent::error::{AuthCause, BudgetKind, Recovery};
use spider_cloud_agent::ops::transform::Document;
use spider_cloud_agent::params::ReturnFormat;
use spider_cloud_agent::policy::StopReason;
use spider_cloud_agent::{Body, Budget, Credits, Error, Spider};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;
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

/// One scripted answer: the status, any extra headers, and the body.
#[derive(Clone)]
struct Answer {
    status: u16,
    /// Extra header lines, each ending in `\r\n`.
    headers: String,
    body: String,
}

impl Answer {
    /// A 200 carrying `body`, which is how the service answers when all is well.
    fn ok(body: &str) -> Answer {
        Answer::with(200, "", body)
    }

    fn with(status: u16, headers: &str, body: &str) -> Answer {
        Answer {
            status,
            headers: headers.to_string(),
            body: body.to_string(),
        }
    }
}

/// Start a stub that answers each request with a 200 and a body from `script`.
///
/// Once the script runs out the last answer is repeated, so a test that expects
/// one request still gets somewhere to escalate to when the fix it pins is
/// broken, and the count is what fails rather than the connection.
fn serve(script: &[&str]) -> Stub {
    let answers: Vec<Answer> = script.iter().map(|body| Answer::ok(body)).collect();
    serve_answers(&answers)
}

/// The same, with the status and the headers scripted too.
fn serve_answers(script: &[Answer]) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("an address");
    let script: Vec<Answer> = script.to_vec();
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
            let fallback = Answer::ok("[]");
            let reply = script
                .get(answered)
                .or_else(|| script.last())
                .unwrap_or(&fallback);
            let head = format!(
                "HTTP/1.1 {} Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{}\r\n",
                reply.status,
                reply.body.len(),
                reply.headers
            );
            if stream.write_all(head.as_bytes()).is_err() {
                break;
            }
            let _ = stream.write_all(reply.body.as_bytes());
            let _ = stream.flush();
        }
    });

    Stub {
        base: Url::parse(&format!("http://{address}")).expect("a base url"),
        seen,
    }
}

/// A stub that reads every request and answers none of them.
///
/// The connection is accepted and held open until the test drops the stub, so
/// what the client sees is a service that took the request and went quiet. This
/// is the shape of a hang: not a refused connection, which fails at once, and
/// not a slow answer, which arrives eventually.
struct Stalled {
    base: Url,
    seen: Receiver<Sent>,
    /// Dropping this is what lets the thread go.
    _release: Sender<()>,
}

fn stall() -> Stalled {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("an address");
    let (sender, seen) = channel();
    let (release, held) = channel::<()>();

    std::thread::spawn(move || {
        let mut open: Vec<TcpStream> = Vec::new();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Some(request) = read_request(&mut stream) else {
                break;
            };
            if sender.send(request).is_err() {
                break;
            }
            open.push(stream);
            // Blocks until the test drops its end, which is when every held
            // connection is let go.
            if held.recv().is_err() {
                break;
            }
        }
        drop(open);
    });

    Stalled {
        base: Url::parse(&format!("http://{address}")).expect("a base url"),
        seen,
        _release: release,
    }
}

impl Stalled {
    /// How many requests reached the stub.
    fn received(&self) -> usize {
        self.seen.try_iter().count()
    }
}

/// A client pointed at a stalled stub, with a wall budget on every operation.
fn client_with_wall(stub: &Stalled, wall: Duration) -> Spider {
    Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .budget(Budget::default().with_wall(wall))
        .build()
        .expect("a client")
}

/// Long enough that a hang is what it measures, short enough to fail fast.
const HANG: Duration = Duration::from_secs(5);

/// The wall the operations below run under.
const WALL: Duration = Duration::from_millis(300);

/// The bug this pins: nothing bounded a call. The default client had no
/// timeout, the wall budget was checked only before a sleep, and a service that
/// accepted the request and never answered held the caller forever. A budget
/// that names a wall now ends the call at that wall and says so.
#[tokio::test]
async fn a_scrape_that_never_answers_stops_at_the_wall_budget() {
    let stub = stall();
    let spider = client_with_wall(&stub, WALL);

    let outcome = tokio::time::timeout(HANG, spider.scrape("https://example.com").send())
        .await
        .expect("the call hung past the wall budget");

    match outcome {
        Err(Error::BudgetExceeded { kind, attempts }) => {
            assert_eq!(kind, BudgetKind::Time);
            assert_eq!(attempts.len(), 1, "attempts: {attempts:?}");
        }
        other => panic!("expected the wall budget to stop the call, got {other:?}"),
    }
    assert_eq!(stub.received(), 1);
}

/// The same for an answer that is not a page. The search path has its own,
/// shorter loop, and it had the same hole.
#[tokio::test]
async fn a_search_that_never_answers_stops_at_the_wall_budget() {
    let stub = stall();
    let spider = client_with_wall(&stub, WALL);

    let outcome = tokio::time::timeout(HANG, spider.search("example").send())
        .await
        .expect("the call hung past the wall budget");

    assert!(
        matches!(
            outcome,
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Time,
                ..
            })
        ),
        "{outcome:?}"
    );
}

/// The account reads take no builder budget, so the client's wall is what
/// bounds them. Without it a balance check against a quiet service never
/// returned.
#[tokio::test]
async fn an_account_read_that_never_answers_stops_at_the_client_wall() {
    let stub = stall();
    let spider = client_with_wall(&stub, WALL);

    let credits = tokio::time::timeout(HANG, spider.credits())
        .await
        .expect("the balance read hung past the wall budget");
    assert!(
        matches!(
            credits,
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Time,
                ..
            })
        ),
        "{credits:?}"
    );

    let logs = tokio::time::timeout(HANG, spider.crawl_logs().limit(1).send())
        .await
        .expect("the log read hung past the wall budget");
    assert!(
        matches!(
            logs,
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Time,
                ..
            })
        ),
        "{logs:?}"
    );
}

/// The bug this pins: the rate limit snapshot is kept for the life of the
/// client, and a reply reported the snapshot as its own headers. One answer
/// carrying `ratelimit-reset: 20` then made every later retry on that client
/// wait twenty seconds, whatever the later answer said, because the send loop
/// reads a reply's reset as the wait the service asked for. A reply now
/// reports the headers it arrived with and nothing older.
#[tokio::test]
async fn a_reset_header_on_one_answer_does_not_set_the_wait_for_a_later_one() {
    let stub = serve_answers(&[
        Answer::with(200, "ratelimit-reset: 20\r\n", PAGE_ANSWER),
        Answer::with(503, "", r#"{"error":"draining"}"#),
        Answer::ok(PAGE_ANSWER),
    ]);
    let spider = client(&stub);

    spider
        .scrape("https://example.com")
        .send()
        .await
        .expect("the first page");

    // The second operation meets a 503 and retries. The wait it takes is the
    // curve, half a second, and not the twenty the earlier answer named.
    let outcome = tokio::time::timeout(
        Duration::from_secs(3),
        spider.scrape("https://example.com").send(),
    )
    .await
    .expect("the retry waited on a header from an earlier answer")
    .expect("the second page");

    assert_eq!(outcome.attempts.len(), 2, "{:?}", outcome.attempts);
    assert_eq!(stub.sent().len(), 3);
}

/// A service asking for a wait of 999999 seconds is not a reason to wait that
/// long. The curve's ceiling holds the figure down, and this pins that the
/// send loop honours the ceiling rather than the header.
#[tokio::test]
async fn a_huge_retry_after_is_held_to_the_backoff_ceiling() {
    use spider_cloud_agent::policy::Backoff;
    use spider_cloud_agent::Policy;

    let stub = serve_answers(&[
        Answer::with(429, "retry-after: 999999\r\n", r#"{"error":"slow down"}"#),
        Answer::ok(PAGE_ANSWER),
    ]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .policy(Policy::standard().with_backoff(Backoff {
            cap: Duration::from_millis(200),
            ..Backoff::default()
        }))
        .build()
        .expect("a client");

    let outcome = tokio::time::timeout(
        Duration::from_secs(3),
        spider.scrape("https://example.com").send(),
    )
    .await
    .expect("the retry waited on the header rather than the ceiling")
    .expect("the page");

    assert_eq!(outcome.attempts.len(), 2, "{:?}", outcome.attempts);
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

/// A key the service refused, with no page in the answer, is the call error
/// itself: one request, and the caller sees the auth failure and not a walk
/// that ran out. The cause says the service refused it, which is what tells
/// an agent that signing in again is the move.
#[tokio::test]
async fn a_refused_key_with_no_page_is_an_auth_error_after_one_request() {
    let stub = serve_answers(&[Answer::with(401, "", r#"{"error":"invalid api key"}"#)]);
    let spider = client(&stub);

    let failed = spider
        .scrape("https://example.com")
        .send()
        .await
        .expect_err("a refused key");

    match &failed {
        Error::Auth { cause, message } => {
            assert_eq!(*cause, AuthCause::Refused);
            assert_eq!(message, "invalid api key");
        }
        other => panic!("expected the auth error itself, got {other:?}"),
    }
    assert!(
        failed
            .to_string()
            .starts_with("authentication: invalid api key"),
        "{failed}"
    );
    assert_eq!(failed.recovery(), Recovery::Reauthenticate);
    assert_eq!(stub.sent().len(), 1, "the refused key was sent again");
}

/// A page that came back blank sends the walk up a step, and the service then
/// rate limits every retry. The walk reached the site, so what the caller gets
/// is an exhausted walk carrying the rate limit as its source, and the recovery
/// reads the wait the service asked for straight off it.
#[tokio::test]
async fn a_rate_limit_that_runs_out_after_a_page_is_an_exhausted_walk() {
    use spider_cloud_agent::policy::Backoff;
    use spider_cloud_agent::Policy;

    let blank = r#"[{"url":"https://example.com/","status":200,"content":"",
        "costs":{"total_cost":0.0001}}]"#;
    let limited = Answer::with(429, "retry-after: 1\r\n", r#"{"error":"slow down"}"#);
    let stub = serve_answers(&[
        Answer::ok(blank),
        limited.clone(),
        limited.clone(),
        limited.clone(),
        limited,
    ]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .budget(Budget::default().with_attempts(10))
        .policy(
            Policy::standard()
                .with_max_attempts(10)
                .with_backoff(Backoff {
                    cap: Duration::from_millis(50),
                    ..Backoff::default()
                }),
        )
        .build()
        .expect("a client");

    let failed = tokio::time::timeout(
        Duration::from_secs(5),
        spider.scrape("https://example.com").send(),
    )
    .await
    .expect("the retries waited on the header rather than the ceiling")
    .expect_err("a walk the rate limit stopped");

    match &failed {
        Error::Exhausted {
            reason,
            source,
            attempts,
            ..
        } => {
            assert_eq!(*reason, StopReason::RetriesExhausted);
            match source.as_deref() {
                Some(Error::Api { status, .. }) => assert_eq!(status.code(), 429),
                other => panic!("the source is not the rate limit: {other:?}"),
            }
            assert_eq!(attempts.len(), stub.sent().len(), "{attempts:?}");
            assert!(attempts.len() >= 4, "{attempts:?}");
        }
        other => panic!("expected an exhausted walk, got {other:?}"),
    }
    assert_eq!(
        failed.recovery(),
        Recovery::Wait(Some(Duration::from_secs(1)))
    );
    assert!(
        failed.to_string().contains("api status 429"),
        "the error does not say what stopped it: {failed}"
    );
}

/// A site that refuses every attempt walks the whole ladder and ends in an
/// exhausted walk whose reason is the policy's own, and never the reason for
/// an attempt no rule matched.
#[tokio::test]
async fn a_site_that_refuses_every_attempt_ends_with_the_policy_reason() {
    use spider_cloud_agent::Policy;

    let refused = r#"[{"url":"https://example.com/","status":403,"content":"",
        "costs":{"total_cost":0.0001}}]"#;
    let stub = serve(&[refused]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .budget(Budget::default().with_attempts(20))
        .policy(Policy::standard().with_max_attempts(20))
        .build()
        .expect("a client");

    let failed = tokio::time::timeout(
        Duration::from_secs(10),
        spider.scrape("https://example.com").send(),
    )
    .await
    .expect("the walk hung")
    .expect_err("a site that refused every attempt");

    match &failed {
        Error::Exhausted {
            reason,
            source,
            last,
            attempts,
            ..
        } => {
            assert!(
                matches!(
                    reason,
                    StopReason::LadderExhausted | StopReason::Rejected { .. }
                ),
                "{reason:?}"
            );
            assert!(source.is_none(), "{source:?}");
            let last = last.as_ref().expect("the last refused page");
            assert_eq!(last.status.code(), 403);
            assert!(attempts.len() > 1, "nothing escalated: {attempts:?}");
        }
        other => panic!("expected an exhausted walk, got {other:?}"),
    }
    assert!(
        failed.to_string().starts_with("no usable page after"),
        "{failed}"
    );
}
