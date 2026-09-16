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
use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
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
    let script: Vec<Vec<u8>> = script
        .iter()
        .map(|reply| {
            let chunked = reply.headers.contains("transfer-encoding: chunked\r\n");
            let length = if chunked {
                String::new()
            } else {
                format!("content-length: {}\r\n", reply.body.len())
            };
            let mut bytes = format!(
                "HTTP/1.1 {} Scripted\r\ncontent-type: application/json\r\n{length}connection: close\r\n{}\r\n",
                reply.status, reply.headers
            )
            .into_bytes();
            if chunked {
                for chunk in reply.body.as_bytes().chunks(16 * 1024) {
                    bytes.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                    bytes.extend_from_slice(chunk);
                    bytes.extend_from_slice(b"\r\n");
                }
                bytes.extend_from_slice(b"0\r\n\r\n");
            } else {
                bytes.extend_from_slice(reply.body.as_bytes());
            }
            bytes
        })
        .collect();
    serve_raw(&script)
}

/// Write exactly these bytes and close the connection, without repairing framing.
/// Repeat the final answer so an unexpected retry is counted too.
fn serve_raw(script: &[Vec<u8>]) -> Stub {
    assert!(!script.is_empty());
    let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0)).expect("a port");
    let address = listener.local_addr().expect("an address");
    let script = script.to_vec();
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
                .unwrap_or_else(|| script.last().unwrap());
            let _ = stream.write_all(reply);
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
    let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0)).expect("a port");
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

/// Drive `op` until the stub holds its request, then freeze the clock so that
/// only a timer can end the call, and let tokio jump straight to the next one.
///
/// This is how a fifteen minute wall is proved in a few milliseconds. The
/// connection is made on the real clock first, because the built-in client's
/// connect timeout is a timer too and a frozen clock would jump to it before
/// the loopback handshake was seen. The clock is thawed again on the way out.
///
/// Returns what the call ended with and how much clock it took. A call that
/// nothing ends is cut at `patience` of frozen time and reported as `None`.
async fn frozen<T>(
    stub: &Stalled,
    patience: Duration,
    op: impl Future<Output = T>,
) -> (Option<T>, Duration) {
    let mut op = std::pin::pin!(op);
    let mut held = 0;
    while held == 0 {
        tokio::select! {
            _ = &mut op => panic!("the call ended before the stub held its request"),
            _ = tokio::time::sleep(Duration::from_millis(2)) => held += stub.received(),
        }
    }
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(patience, op).await.ok();
    let took = started.elapsed();
    tokio::time::resume();
    (result, took)
}

/// Long enough that a hang is what it measures, short enough to fail fast.
const HANG: Duration = Duration::from_secs(5);

/// The wall the operations below run under.
const WALL: Duration = Duration::from_millis(300);

struct SlowObserver;

impl spider_route::Router for SlowObserver {
    fn route(&self, input: &spider_route::RouteInput<'_>) -> spider_route::RouteDecision {
        spider_route::Router::route(&spider_route::HeuristicRouter::new(), input)
    }

    fn observe(&self, _: &spider_route::RouteInput<'_>, _: &spider_route::AttemptOutcome) {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(200) {
            std::hint::spin_loop();
        }
    }
}

#[tokio::test]
async fn f1_settlement_cannot_succeed_after_the_deadline() {
    let stub = serve(&[PAGE_ANSWER]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .router(SlowObserver)
        .budget(Budget::default().with_wall(Duration::from_millis(100)))
        .build()
        .unwrap();
    let result = spider.scrape("https://example.com").send().await;
    assert!(
        matches!(
            result,
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Time,
                ..
            })
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn f1_retry_sleep_uses_the_operation_deadline() {
    let stub = serve_answers(&[Answer::with(503, "", r#"{"error":"busy"}"#)]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .router(SlowObserver)
        // Keep the 500 ms wait fixed: random jitter can legitimately leave
        // room for another call after the observer's 200 ms of work.
        .policy(spider_cloud_agent::policy::Policy::standard())
        .budget(Budget::default().with_wall(Duration::from_millis(600)))
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let result = spider.scrape("https://example.com").send().await;
    assert!(
        matches!(
            result,
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Time,
                ..
            })
        ),
        "{result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(680),
        "sleep ran past the deadline"
    );
    assert_eq!(stub.sent().len(), 1);
}

#[tokio::test]
async fn f1_wall_ends_every_operation() {
    let wall = Budget::default().wall.expect("a finite default wall");
    assert_eq!(wall, Duration::from_secs(900));
    let mut tasks = Vec::new();
    for operation in 0..10 {
        tasks.push(tokio::spawn(async move {
            let stub = stall();
            let spider = Spider::builder()
                .key("not-a-real-key")
                .base_url(stub.base.clone())
                .budget(Budget::default().with_wall(WALL))
                .build()
                .unwrap();
            let result = tokio::time::timeout(HANG, async {
                match operation {
                    0 => spider
                        .scrape("https://example.com")
                        .send_all()
                        .await
                        .map(|_| ()),
                    1 => spider
                        .crawl("https://example.com")
                        .send_all()
                        .await
                        .map(|_| ()),
                    2 => spider
                        .links("https://example.com")
                        .send_all()
                        .await
                        .map(|_| ()),
                    3 => spider
                        .screenshot("https://example.com")
                        .send_all()
                        .await
                        .map(|_| ()),
                    4 => spider
                        .fetch("example.com", "/")
                        .send_all()
                        .await
                        .map(|_| ()),
                    5 => spider
                        .transform(vec![Document::html("<p>hello</p>")])
                        .send_all()
                        .await
                        .map(|_| ()),
                    6 => spider.search("example").send().await.map(|_| ()),
                    7 => spider.credits().await.map(|_| ()),
                    8 => spider.crawl_logs().send().await.map(|_| ()),
                    _ => spider.table("pages").send().await.map(|_| ()),
                }
            })
            .await
            .expect("operation hung");
            assert!(
                matches!(
                    result,
                    Err(Error::BudgetExceeded {
                        kind: BudgetKind::Time,
                        ..
                    })
                ),
                "{result:?}"
            );
            assert_eq!(stub.received(), 1);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}

/// The default wall, exercised as a caller meets it: a client built with no
/// budget at all, against a service that takes the request and never answers,
/// on every page operation and on search. The clock is frozen once the stub
/// holds the request, so the test proves the fifteen minutes without waiting
/// them, and a build with no default wall fails here because nothing ends the
/// call before the patience runs out.
#[tokio::test]
async fn f1_the_default_wall_ends_every_page_operation_and_search() {
    let wall = Budget::default().wall.expect("a finite default wall");
    assert_eq!(wall, Duration::from_secs(15 * 60));
    let clock = std::time::Instant::now();
    for operation in 0..7 {
        let stub = stall();
        let spider = Spider::builder()
            .key("not-a-real-key")
            .base_url(stub.base.clone())
            .build()
            .unwrap();
        let (result, took) = frozen(&stub, wall * 2, async {
            match operation {
                0 => spider
                    .scrape("https://example.com")
                    .send_all()
                    .await
                    .map(|_| ()),
                1 => spider
                    .crawl("https://example.com")
                    .send_all()
                    .await
                    .map(|_| ()),
                2 => spider
                    .links("https://example.com")
                    .send_all()
                    .await
                    .map(|_| ()),
                3 => spider
                    .screenshot("https://example.com")
                    .send_all()
                    .await
                    .map(|_| ()),
                4 => spider
                    .fetch("example.com", "/")
                    .send_all()
                    .await
                    .map(|_| ()),
                5 => spider
                    .transform(vec![Document::html("<p>hello</p>")])
                    .send_all()
                    .await
                    .map(|_| ()),
                _ => spider.search("example").send().await.map(|_| ()),
            }
        })
        .await;
        let result = result.expect("no wall ended the call");
        assert!(
            matches!(
                result,
                Err(Error::BudgetExceeded {
                    kind: BudgetKind::Time,
                    ..
                })
            ),
            "operation {operation}: {result:?}"
        );
        assert!(took <= wall, "operation {operation} took {took:?}");
    }
    assert!(clock.elapsed() < HANG, "the frozen clock was not used");
}

/// An account read runs under a minute unless the client's wall is shorter,
/// whatever the budget says, and a budget with a longer wall does not stretch
/// it. Only `without_wall` on the builder lifts it.
#[tokio::test]
async fn f1_account_reads_end_under_the_read_wall_by_default() {
    let minute = Duration::from_secs(60);
    let default = || Spider::builder().key("not-a-real-key");
    let long = || {
        Spider::builder()
            .key("not-a-real-key")
            .budget(Budget::default().with_wall(Duration::from_secs(2 * 60 * 60)))
    };
    let builders: [&dyn Fn() -> spider_cloud_agent::client::SpiderBuilder; 2] = [&default, &long];
    for (which, builder) in builders.iter().enumerate() {
        for operation in 0..3 {
            let stub = stall();
            let spider = builder().base_url(stub.base.clone()).build().unwrap();
            let (result, took) = frozen(&stub, minute * 2, async {
                match operation {
                    0 => spider.credits().await.map(|_| ()),
                    1 => spider.crawl_logs().send().await.map(|_| ()),
                    _ => spider.table("pages").send().await.map(|_| ()),
                }
            })
            .await;
            let result = result.expect("no wall ended the read");
            assert!(
                matches!(
                    result,
                    Err(Error::BudgetExceeded {
                        kind: BudgetKind::Time,
                        ..
                    })
                ),
                "builder {which}, read {operation}: {result:?}"
            );
            assert!(
                took <= minute,
                "builder {which}, read {operation} took {took:?}"
            );
        }
    }
}

#[tokio::test]
async fn f1_without_wall_lets_an_account_read_wait() {
    for operation in 0..3 {
        let stub = stall();
        let spider = Spider::builder()
            .key("not-a-real-key")
            .base_url(stub.base.clone())
            .without_wall()
            .build()
            .unwrap();
        let (result, _) = frozen(&stub, Duration::from_secs(60 * 60), async {
            match operation {
                0 => spider.credits().await.map(|_| ()),
                1 => spider.crawl_logs().send().await.map(|_| ()),
                _ => spider.table("pages").send().await.map(|_| ()),
            }
        })
        .await;
        assert!(
            result.is_none(),
            "read {operation} ended with no wall: {result:?}"
        );
    }
}

#[tokio::test]
async fn f1_http_base_is_refused_before_sending() {
    let stub = serve(&[PAGE_ANSWER]);
    let mut base = stub.base.clone();
    base.set_host(Some("localhost")).unwrap();
    let built = Spider::builder()
        .key("not-a-real-key")
        .base_url(base)
        .build();
    assert!(matches!(built, Err(Error::Config(_))));
    assert!(stub.sent().is_empty());
}

#[tokio::test]
async fn f1_insecure_opt_in_allows_a_test_service() {
    let stub = serve(&[PAGE_ANSWER]);
    let mut base = stub.base.clone();
    base.set_host(Some("localhost")).unwrap();
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(base)
        .allow_insecure_http(true)
        .build()
        .unwrap();
    spider.scrape("https://example.com").send().await.unwrap();
    assert_eq!(stub.sent().len(), 1);
}

/// The opt-out is a method the caller has to name, and once named a page
/// operation waits past the default wall the way it always did.
#[tokio::test]
async fn f1_wall_opt_out_is_explicit() {
    let stub = stall();
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .without_wall()
        .build()
        .unwrap();
    assert_eq!(spider.budget().wall, None);
    let wall = Budget::default().wall.expect("a finite default wall");
    let (result, _) = frozen(&stub, wall * 2, spider.scrape("https://example.com").send()).await;
    assert!(result.is_none(), "the call ended with no wall: {result:?}");
}

/// One page, larger than the cap the crate used to carry. Nothing caps an
/// answer unless the caller built the client with a cap, because a crawl
/// answer is as large as the site and a limit nobody asked for turned real
/// pages into errors.
#[tokio::test]
async fn f1_no_answer_is_too_large_unless_the_client_says_so() {
    let body = format!(
        r#"[{{"url":"https://example.com/","status":200,"content":"{}","costs":{{"total_cost":0.0001}}}}]"#,
        "x".repeat(9 * 1024 * 1024)
    );
    let stub = serve(&[&body]);
    let page = client(&stub)
        .scrape("https://example.com")
        .send()
        .await
        .expect("a page of nine megabytes");
    assert_eq!(page.value.text().map(str::len), Some(9 * 1024 * 1024));
    assert_eq!(stub.sent().len(), 1);
}

/// A cap the client asked for cuts the answer off, and the walk that hit it
/// keeps its trail: a refusal that cost credits before it is still on the
/// error, and the cut-off call is recorded with the status it arrived with.
#[tokio::test]
async fn f1_a_cap_the_client_asked_for_keeps_the_spend() {
    let limit = 4096;
    let refused = r#"[{"url":"https://example.com/","status":403,"content":"",
        "costs":{"total_cost":0.0001}}]"#;
    let stub = serve_answers(&[Answer::ok(refused), Answer::ok(&" ".repeat(limit + 1))]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .max_response_bytes(limit)
        .build()
        .unwrap();
    let error = tokio::time::timeout(HANG, spider.scrape("https://example.com").send())
        .await
        .expect("the capped call hung")
        .unwrap_err();
    assert_eq!(error.spent(), Credits::from_usd(0.0001));
    match &error {
        Error::Exhausted {
            attempts, source, ..
        } => {
            assert_eq!(attempts.len(), 2);
            assert_eq!(attempts[1].api.code(), 200);
            assert!(
                matches!(
                    source.as_deref(),
                    Some(Error::ResponseTooLarge { limit: got, .. }) if *got == limit
                ),
                "{error:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(stub.sent().len(), 2);
}

/// The cap holds on a chunked answer too, where there is no length to read
/// ahead of the body, and on search, whose one call is recorded the same way.
#[tokio::test]
async fn f1_a_cap_holds_on_chunked_answers_and_on_search() {
    let limit = 4096;
    let chunked = serve_answers(&[Answer::with(
        200,
        "transfer-encoding: chunked\r\n",
        &" ".repeat(limit + 1),
    )]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(chunked.base.clone())
        .max_response_bytes(limit)
        .build()
        .unwrap();
    let result = tokio::time::timeout(HANG, spider.scrape("https://example.com").send())
        .await
        .expect("the streaming answer hung");
    assert!(
        matches!(
            &result,
            Err(Error::Exhausted { attempts, source, .. })
                if attempts.len() == 1
                    && matches!(source.as_deref(), Some(Error::ResponseTooLarge { limit: got, .. }) if *got == limit)
        ),
        "{result:?}"
    );
    assert_eq!(chunked.sent().len(), 1);

    let searched = serve(&[&" ".repeat(limit + 1)]);
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(searched.base.clone())
        .max_response_bytes(limit)
        .build()
        .unwrap();
    let result = tokio::time::timeout(HANG, spider.search("example").send())
        .await
        .expect("the search hung");
    assert!(
        matches!(
            &result,
            Err(Error::Exhausted { attempts, source, .. })
                if attempts.len() == 1
                    && matches!(source.as_deref(), Some(Error::ResponseTooLarge { limit: got, .. }) if *got == limit)
        ),
        "{result:?}"
    );
    assert_eq!(searched.sent().len(), 1);
}

#[tokio::test]
async fn f1_page_payment_is_not_an_account_balance_error() {
    let stub = serve_answers(&[Answer::with(
        402,
        "",
        r#"{"url":"https://example.com","status":402,"costs":{"total_cost":0.0001}}"#,
    )]);
    let error = client(&stub)
        .scrape("https://example.com")
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.spent(), Credits::from_usd(0.0001));
    assert!(matches!(error, Error::Exhausted { last: Some(_), .. }));
    assert_eq!(stub.sent().len(), 1);
}

#[tokio::test]
async fn f1_empty_204_is_empty_pages_and_no_single_page() {
    let stub = serve_answers(&[Answer::with(204, "", "")]);
    let spider = client(&stub);
    let all = spider
        .scrape("https://example.com")
        .send_all()
        .await
        .unwrap();
    assert!(all.value.is_empty());
    assert_eq!(all.attempts.len(), 1);
    assert!(matches!(
        spider.scrape("https://example.com").send().await,
        Err(Error::Exhausted { last: None, .. })
    ));
    assert_eq!(stub.sent().len(), 2);
}

#[tokio::test]
async fn f1_missing_target_status_is_unknown() {
    let stub = serve(&[r#"{"url":"https://example.com","content":"hello"}"#]);
    let error = client(&stub)
        .scrape("https://example.com")
        .send_all()
        .await
        .unwrap_err();
    match error {
        Error::Exhausted {
            last: Some(page), ..
        } => assert_eq!(page.status.code(), 0),
        other => panic!("{other:?}"),
    }
    assert_eq!(stub.sent().len(), 1);
}

#[tokio::test]
async fn f1_mirrored_login_walks_but_account_refusal_stops() {
    let stub = serve_answers(&[
        Answer::with(
            401,
            "",
            r#"{"url":"https://example.com","status":401,"costs":{"total_cost":0.0001}}"#,
        ),
        Answer::ok(PAGE_ANSWER),
    ]);
    let spider = client(&stub);
    let mut call = spider.scrape("https://example.com");
    call.params_mut().cookies = Some("session=placeholder".into());
    let result = call.send().await.unwrap();
    assert_eq!(result.attempts.len(), 2);
    assert_eq!(result.attempts[0].cost, Credits::from_usd(0.0001));
    assert_eq!(result.attempts[0].page.unwrap().code(), 401);
    assert_eq!(stub.sent().len(), 2);
    let stub = serve_answers(&[Answer::with(401, "", r#"{"error":"key refused"}"#)]);
    assert!(matches!(
        client(&stub)
            .scrape("https://example.com")
            .send()
            .await
            .unwrap_err()
            .cause(),
        Error::Auth {
            cause: AuthCause::Refused,
            ..
        }
    ));
    assert_eq!(stub.sent().len(), 1);
}

#[tokio::test]
async fn f1_cookie_map_keeps_the_page_and_cost() {
    let stub = serve(&[
        r#"{"url":"https://example.com","status":200,"content":"hello","cookies":{"session":"REDACTED"},"costs":{"total_cost":0.0001}}"#,
    ]);
    let page = client(&stub)
        .scrape("https://example.com")
        .send()
        .await
        .unwrap();
    assert!(page.value.cookies.is_some());
    assert_eq!(page.cost, Credits::from_usd(0.0001));
}

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

/// A server wait that cannot fit the wall stops the walk without retrying early.
#[tokio::test]
async fn a_huge_retry_after_stops_at_the_wall_without_retrying_early() {
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
    .expect("the policy must refuse the wait immediately")
    .unwrap_err();

    assert!(matches!(
        outcome,
        Error::BudgetExceeded {
            kind: BudgetKind::Time,
            ..
        }
    ));
    assert_eq!(stub.sent().len(), 1);
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

#[tokio::test]
async fn f2_zero_caps_send_nothing_on_scrape_and_search() {
    for (budget, kind) in [
        (Budget::default().with_attempts(0), BudgetKind::Attempts),
        (
            Budget::default().with_credits(Credits::ZERO),
            BudgetKind::Credits,
        ),
    ] {
        let stub = serve(&[PAGE_ANSWER]);
        let spider = client(&stub);
        let scrape = spider
            .scrape("https://example.com")
            .budget(budget)
            .send()
            .await
            .unwrap_err();
        let search = spider
            .search("example")
            .budget(budget)
            .send()
            .await
            .unwrap_err();
        for error in [scrape, search] {
            assert!(
                matches!(error, Error::BudgetExceeded { kind: got, ref attempts } if got == kind && attempts.is_empty())
            );
        }
        assert!(stub.sent().is_empty());
    }
}

#[tokio::test]
async fn f2_paid_success_keeps_the_page_and_reports_overrun() {
    let stub = serve(&[
        r#"[{"url":"https://example.com","status":200,"content":"paid","costs":{"total_cost":0.0002}}]"#,
    ]);
    let spider = client(&stub);
    let outcome = spider
        .scrape("https://example.com")
        .budget(Budget::default().with_credits(Credits(1.0)))
        .send()
        .await
        .unwrap();
    assert_eq!(outcome.text(), Some("paid"));
    let overrun = outcome.overrun.unwrap();
    assert_eq!(overrun.cap, Credits(1.0));
    assert_eq!(overrun.spent, Credits(2.0));
    assert_eq!(stub.sent().len(), 1);
}

#[tokio::test]
async fn f2_run_credits_carry_across_operations_and_clones() {
    let stub = serve(&[
        r#"[{"url":"https://example.com","status":200,"content":"paid","costs":{"total_cost":0.000031}}]"#,
    ]);
    let run = spider_cloud_agent::RunBudget::new(Credits(1.0));
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .run_budget(run.clone())
        .build()
        .unwrap();
    for _ in 0..3 {
        spider
            .clone()
            .scrape("https://example.com")
            .send()
            .await
            .unwrap();
    }
    let error = spider
        .scrape("https://example.com")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(
        error.cause(),
        Error::BudgetExceeded {
            kind: BudgetKind::Credits,
            ..
        }
    ));
    match error {
        Error::Accounted { run: Some(run), .. } => {
            assert_eq!(run.attempts, 3);
            assert!((run.spent.get() - 0.93).abs() < 1e-9);
        }
        other => panic!("missing run accounting: {other:?}"),
    }
    assert_eq!(stub.sent().len(), 3);
}

#[tokio::test]
async fn f2_concurrent_operations_reserve_before_sending() {
    let stub = stall();
    let run = spider_cloud_agent::RunBudget::new(Credits(0.1));
    let spider = Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .budget(Budget::default().with_wall(WALL))
        .run_budget(run.clone())
        .build()
        .unwrap();
    let (first, second) = tokio::join!(
        spider.scrape("https://example.com").send(),
        spider.search("example").send()
    );
    for error in [first.unwrap_err(), second.unwrap_err()] {
        assert!(matches!(error.cause(), Error::BudgetExceeded { .. }));
    }
    assert_eq!(stub.received(), 1);
    assert_eq!(run.snapshot().attempts, 1);
    assert_eq!(run.snapshot().unknown, 1);
    assert_eq!(run.remaining(), Credits::ZERO);
}

#[tokio::test]
async fn f2_decode_failure_keeps_previous_and_current_known_bills() {
    let paid = r#"[{"url":"https://example.com","status":403,"costs":{"total_cost":0.0001}}]"#;
    for malformed in [
        "not json",
        r#"{"status":"bad","costs":{"total_cost":0.0002}}"#,
    ] {
        let stub = serve(&[paid, malformed]);
        let run = spider_cloud_agent::RunBudget::new(Credits(100.0));
        let spider = Spider::builder()
            .key("not-a-real-key")
            .base_url(stub.base.clone())
            .run_budget(run.clone())
            .build()
            .unwrap();
        let error = spider
            .scrape("https://example.com")
            .send()
            .await
            .unwrap_err();
        assert!(matches!(error.cause(), Error::Decode(_)));
        assert_eq!(error.attempts().len(), 2);
        let expected = if malformed == "not json" { 1.0 } else { 3.0 };
        assert_eq!(error.spent(), Credits(expected));
        assert_eq!(run.snapshot().spent, Credits(expected));
        assert_eq!(error.attempts()[1].charge_unknown, malformed == "not json");
        assert_eq!(stub.sent().len(), 2);
    }
}

#[tokio::test]
async fn f2_search_decode_keeps_a_known_bill() {
    let stub = serve(&[r#"{"content":false,"costs":{"total_cost":0.0002}}"#]);
    let error = client(&stub).search("example").send().await.unwrap_err();
    assert!(matches!(error.cause(), Error::Decode(_)));
    assert_eq!(error.spent(), Credits(2.0));
    assert_eq!(error.attempts().len(), 1);
    assert!(!error.attempts()[0].charge_unknown);
}

#[tokio::test]
async fn f2_clients_add_different_jitter_after_the_same_429() {
    use spider_cloud_agent::policy::Backoff;
    let mut waits = Vec::new();
    for seed in [1, 3] {
        let stub = serve_answers(&[
            Answer::with(429, "retry-after: 0\r\n", r#"{"error":"slow down"}"#),
            Answer::ok(PAGE_ANSWER),
        ]);
        let spider = Spider::builder()
            .key("not-a-real-key")
            .base_url(stub.base.clone())
            .jitter_seed(seed)
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let outcome = spider.scrape("https://example.com").send().await.unwrap();
        let wait = started.elapsed().saturating_sub(outcome.elapsed());
        let expected = Backoff::seeded(seed).delay(0, Some(Duration::ZERO));
        assert!(wait >= expected, "{wait:?} shorter than {expected:?}");
        assert!(wait < expected + Duration::from_millis(200), "{wait:?}");
        waits.push(wait);
        assert_eq!(stub.sent().len(), 2);
    }
    assert!((waits[0].as_secs_f64() - waits[1].as_secs_f64()).abs() > 0.1);
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
    assert!(outcome.value.status.is_unknown());
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

    match failed.cause() {
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

/// Keep the standard rules and retry bounds, but avoid waiting on backoff in tests.
fn fast_client(stub: &Stub) -> Spider {
    use spider_cloud_agent::policy::backoff::Backoff;
    Spider::builder()
        .key("not-a-real-key")
        .base_url(stub.base.clone())
        .policy(
            spider_cloud_agent::Policy::standard().with_backoff(Backoff {
                base: Duration::ZERO,
                cap: Duration::ZERO,
                ..Backoff::default()
            }),
        )
        .build()
        .unwrap()
}

// Red with the 401 and 402 arms taken out of Reply::as_error, so both fell
// through to Error::Api: the 401 case came back
// Err(Api { status: ApiStatus(401), message: Some("account refused") }).
#[tokio::test]
async fn api_401_and_402_stop_after_one_wire_attempt() {
    for code in [401, 402] {
        let stub = serve_answers(&[Answer::with(code, "", r#"{"error":"account refused"}"#)]);
        let result = fast_client(&stub)
            .scrape("https://example.com")
            .send()
            .await;
        match code {
            401 => assert!(
                matches!(
                    result,
                    Err(Error::Auth {
                        cause: AuthCause::Refused,
                        ..
                    })
                ),
                "{result:?}"
            ),
            _ => assert!(
                matches!(result, Err(Error::InsufficientCredits)),
                "{result:?}"
            ),
        }
        assert_eq!(stub.sent().len(), 1);
    }
}

// Red with Reply::as_error returning None for 403, which hands a failed call
// back as a reply worth reading: the call ended
// Err(Exhausted { .. reason: Unhandled }) with one ApiStatus(403) attempt
// rather than Error::Api.
#[tokio::test]
async fn api_403_stops_after_one_wire_attempt() {
    let stub = serve_answers(&[Answer::with(403, "", r#"{"error":"forbidden"}"#)]);
    let result = fast_client(&stub)
        .scrape("https://example.com")
        .send()
        .await;
    assert!(
        matches!(result, Err(Error::Api { status, .. }) if status.code() == 403),
        "{result:?}"
    );
    assert_eq!(stub.sent().len(), 1);
}

/// API 500 retries once per ladder step, then climbs. The default attempt cap
/// ends the walk after five calls, retaining each API status in the budget error.
/// It does not share the retry-only rule used by API 429 and 503.
// Red with DEFAULT_ATTEMPTS in src/policy/budget.rs moved from 5 to 3: the
// five-attempt assertion failed with left 3, right 5. Turning the API
// ServerError rule in src/policy/rule.rs into Decision::Fail reds it further
// up, at Error::Api on the first call.
#[tokio::test]
async fn api_500_retries_and_climbs_until_the_attempt_cap_on_the_wire() {
    let stub = serve_answers(&[Answer::with(500, "", r#"{"error":"server failed"}"#)]);
    let result = fast_client(&stub)
        .scrape("https://example.com")
        .send()
        .await;
    let Err(Error::BudgetExceeded {
        kind: BudgetKind::Attempts,
        attempts,
    }) = result
    else {
        panic!("expected the attempt cap, got {result:?}");
    };
    assert_eq!(attempts.len(), 5);
    for attempt in attempts {
        assert_eq!(attempt.api.code(), 500);
        assert!(attempt.page.is_none());
    }
    let sent = stub.sent();
    assert_eq!(sent.len(), 5);
    assert_eq!(sent[0].body, sent[1].body, "retry must not climb");
    assert_ne!(sent[1].body, sent[2].body, "then climb one step");
    assert_eq!(sent[2].body, sent[3].body, "retry the new step once");
    assert_ne!(sent[3].body, sent[4].body, "then climb again");
}

// Red with the 401 and 402 exclusion put back into is_mirrored_page_status, the
// one F1 removed: the page 401 case came back
// Err(Auth { cause: Refused, message: "the key was missing or rejected" }),
// treating a login wall on the target site as a rejected Spider key.
#[tokio::test]
async fn account_and_server_codes_in_page_bodies_take_the_page_path() {
    for code in [401, 402, 403, 500] {
        let body =
            format!(r#"{{"url":"https://example.com","status":{code},"content":"refused"}}"#);
        let stub = serve_answers(&[Answer::with(code, "", &body)]);
        let spider = Spider::builder()
            .key("not-a-real-key")
            .base_url(stub.base.clone())
            .policy(spider_cloud_agent::Policy::standard().with_max_attempts(1))
            .build()
            .unwrap();
        let result = spider.scrape("https://example.com").send().await;
        // A page 403 or 500 asks for another attempt; the explicit cap stops it.
        // Login and payment pages stop without needing another attempt.
        let attempts = match result {
            Err(Error::Exhausted {
                attempts,
                last: Some(last),
                source: None,
                ..
            }) if code < 403 => {
                assert_eq!(last.status.code(), code);
                attempts
            }
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Attempts,
                attempts,
            }) if code >= 403 => attempts,
            other => panic!("page {code}: {other:?}"),
        };
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].api.code(), 0);
        assert_eq!(attempts[0].page.unwrap().code(), code);
        assert_eq!(stub.sent().len(), 1);
    }
}

// Red with the body read error swallowed in Transport::execute
// (response.bytes().await.unwrap_or_default()): the truncated message came back
// Err(Decode(Error("EOF while parsing a value", line: 1, column: 0))), a broken
// HTTP message reported as bad JSON.
#[tokio::test]
async fn a_body_shorter_than_content_length_is_a_transport_error() {
    // Complete JSON still fails when the HTTP message is incomplete.
    let raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{PAGE_ANSWER}",
        PAGE_ANSWER.len() + 10
    );
    let stub = serve_raw(&[raw.into_bytes()]);
    let result = fast_client(&stub)
        .scrape("https://example.com")
        .send()
        .await;
    assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
    assert_eq!(stub.sent().len(), 1);
}

// Red with the same swallowed body read error in Transport::execute: the
// half-written chunk came back
// Err(Decode(Error("EOF while parsing a value", line: 1, column: 0))) instead of
// Error::Transport.
#[tokio::test]
async fn a_connection_closed_mid_body_is_a_transport_error() {
    let raw = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n40\r\n{\"url\":";
    let stub = serve_raw(&[raw.to_vec()]);
    let result = fast_client(&stub)
        .scrape("https://example.com")
        .send()
        .await;
    assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
    assert_eq!(stub.sent().len(), 1);
}

// Red with Transport::execute reading only the first body chunk
// (response.chunk() in place of response.bytes()): the two-chunk page failed
// with Decode(Error("EOF while parsing a string", line: 1, column: 20)), which
// is the first chunk boundary.
#[tokio::test]
async fn chunked_transfer_decodes_a_page_once() {
    let (first, second) = PAGE_ANSWER.split_at(20);
    let raw = format!(
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
        first.len(),
        second.len()
    );
    let stub = serve_raw(&[raw.into_bytes()]);
    let page = fast_client(&stub)
        .scrape("https://example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(page.status.code(), 200);
    assert!(!page.body.is_empty());
    assert_eq!(stub.sent().len(), 1);
}

/// The built-in client has no gzip decoder enabled. A valid gzip member reaches
/// the JSON decoder as compressed bytes and ends in Error::Decode, without retry.
// Red with reqwest's "gzip" feature added to the workspace Cargo.toml, which
// turns its automatic decoder on (run without --locked, since that moves the
// lockfile): the member decoded to [], and the call walked the whole ladder to
// Err(Exhausted { .. reason: LadderExhausted }) over five HTTP 200s. Enabling
// that feature is therefore a behaviour change, not a build detail.
#[tokio::test]
async fn gzip_is_not_decoded_by_the_builtin_transport() {
    // A gzip member containing [], made with gzip.compress(b"[]", mtime=0).
    let body: &[u8] = &[
        31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 139, 142, 5, 0, 41, 187, 76, 13, 2, 0, 0, 0,
    ];
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    let stub = serve_raw(&[raw]);
    let result = fast_client(&stub)
        .scrape("https://example.com")
        .send()
        .await;
    assert!(matches!(result, Err(Error::Decode(_))), "{result:?}");
    assert_eq!(stub.sent().len(), 1);
}

/// Redirects must not turn an API POST into a GET or escape attempt accounting.
// Red with the custom redirect policy removed from
// Transport::with_base_options: reqwest followed the 302, sent a second request
// inside the same attempt, and the call returned Ok with the page from the
// address in the Location header. One attempt, two requests, and a body the
// service never saw.
#[tokio::test]
async fn a_302_redirect_is_a_transport_error_after_one_request() {
    let stub = serve_answers(&[
        Answer::with(302, "location: /redirected\r\n", ""),
        Answer::ok(PAGE_ANSWER),
    ]);
    let result = fast_client(&stub)
        .scrape("https://example.com")
        .send()
        .await;
    assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
    let sent = stub.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].line, "POST /scrape HTTP/1.1");
}

/// Replay one recorded response record as a scripted answer.
///
/// These records keep the HTTP envelope beside the JSON body, because the status
/// and the rate limit headers are half of what the send loop decides on. Each
/// one is the output of `cargo run --locked -p xtask -- redact` over a recording
/// held outside the repo, so the hosts and the gateway session cookie in it are
/// what the redactor left behind. They stand for the endpoint contracts, not for
/// any particular service revision.
fn recorded_answer(name: &str) -> Answer {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let headers: String = record["headers"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, value)| format!("{name}: {}\r\n", value.as_str().unwrap()))
        .collect();
    Answer::with(
        record["http_status"].as_u64().unwrap().try_into().unwrap(),
        &headers,
        &serde_json::to_string(&record["body"]).unwrap(),
    )
}

// Red with retry_after set to None on the Reply built by Transport::execute: the
// recorded 429 surfaced retry_after: Some(2s), the ratelimit-reset fallback,
// rather than the 1s the retry-after header asked for.
#[tokio::test]
async fn recorded_api_errors_keep_status_headers_and_retry_bounds() {
    for (file, code, count) in [
        ("api_401.json", 401, 1),
        ("api_402.json", 402, 1),
        ("api_429.json", 429, 4),
        ("api_503.json", 503, 4),
    ] {
        let stub = serve_answers(&[recorded_answer(file)]);
        let spider = fast_client(&stub);
        let result = spider.scrape("https://example.com").send().await;
        match code {
            401 => assert!(
                matches!(
                    result,
                    Err(Error::Auth {
                        cause: AuthCause::Refused,
                        ..
                    })
                ),
                "{result:?}"
            ),
            402 => assert!(
                matches!(result, Err(Error::InsufficientCredits)),
                "{result:?}"
            ),
            _ => {
                assert!(
                    matches!(result, Err(Error::Api { status, retry_after: Some(wait), .. })
                    if status.code() == code && wait == Duration::from_secs(1)),
                    "{result:?}"
                );
                let limits = spider.raw().rate_limit();
                assert_eq!(limits.limit, Some(100));
                assert_eq!(limits.remaining, Some(0));
                assert_eq!(limits.reset, Some(Duration::from_secs(2)));
            }
        }
        assert_eq!(stub.sent().len(), count, "{file}");
    }
}

// Red twice. With route::DATA_TABLE changed from Route::get to Route::post the
// table read failed with Config("POST /data/{table} is not a get route"). With
// Body::Html dropped from Body::as_str the transform answer read back as None
// instead of its converted markup.
#[tokio::test]
async fn recorded_endpoint_answers_reach_their_builders() {
    for (file, request) in [
        ("links.json", "POST /links HTTP/1.1"),
        ("transform.json", "POST /transform HTTP/1.1"),
        ("fetch.json", "POST /fetch/example.com/docs HTTP/1.1"),
        ("data_table.json", "GET /data/pages HTTP/1.1"),
    ] {
        let stub = serve_answers(&[recorded_answer(file)]);
        let spider = fast_client(&stub);
        match file {
            "links.json" => {
                let page = spider.links("https://example.com").send().await.unwrap();
                assert_eq!(page.value[0].as_str(), "https://example.com/docs");
            }
            "transform.json" => {
                let page = spider
                    .transform(vec![Document::html("<p>Converted</p>")])
                    .send()
                    .await
                    .unwrap();
                assert_eq!(
                    page.text(),
                    Some(
                        "<p>Converted from <a href=\"https://example.com/guide\">the guide</a></p>"
                    )
                );
            }
            "fetch.json" => {
                let page = spider.fetch("example.com", "docs").send().await.unwrap();
                assert_eq!(page.text(), Some("<p>Cached</p>"));
            }
            _ => {
                let rows = spider.table("pages").send().await.unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0]["url"], "https://example.com/docs");
            }
        }
        let sent = stub.sent();
        assert_eq!(sent.len(), 1, "{file}");
        assert_eq!(sent[0].line, request);
    }
}
