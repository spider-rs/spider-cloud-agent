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

//! The router, in the loop.
//!
//! Everything here runs against a socket on the loopback address that answers
//! with whatever the test wrote down, so what is checked is the request that
//! left rather than a decision inspected in place. A router that is consulted
//! and then ignored looks identical from the outside otherwise.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use spider_cloud_agent::policy::{Budget, ASSUMED_MINIMUM_COST};
use spider_cloud_agent::record::JsonlRecorder;
use spider_cloud_agent::{Explorer, ProxyPool, RequestMode, Spider};
use spider_route::{
    Action, AttemptOutcome, HeuristicRouter, RouteDecision, RouteInput, RouteSource, Router,
    RouterVersion,
};
use url::Url;

/// A service on the loopback address that answers from a script.
struct Fake {
    base: Url,
    sent: Receiver<String>,
}

impl Fake {
    /// Answer every call with this status and this body.
    fn always(status: u16, body: &str) -> Fake {
        Fake::scripted(vec![(status, body.to_string())])
    }

    /// Answer in order, repeating the last entry once the script runs out.
    fn scripted(script: Vec<(u16, String)>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port on the loopback address");
        let address = listener.local_addr().expect("the port it bound");
        let (sender, sent) = channel();

        std::thread::spawn(move || {
            for (nth, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { break };
                let body = read_request(&mut stream);
                if sender.send(body).is_err() {
                    break;
                }

                let (status, payload) = script
                    .get(nth)
                    .or_else(|| script.last())
                    .cloned()
                    .unwrap_or((200, "[]".to_string()));
                let _ = write_reply(&mut stream, status, &payload);
            }
        });

        Fake {
            base: Url::parse(&format!("http://{address}/")).expect("a loopback address"),
            sent,
        }
    }

    /// The body of the next request that arrived.
    fn next_request(&self) -> String {
        self.sent
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a request to have arrived")
    }

    /// Whether anything else arrived.
    fn quiet(&self) -> bool {
        self.sent
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err()
    }
}

/// Read one HTTP request off a stream and hand back its body.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];

    // Headers first, one byte at a time, which is slow and completely fine for
    // a handful of requests.
    while !buffer.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return String::new(),
            Ok(_) => buffer.extend_from_slice(&byte),
        }
    }

    let headers = String::from_utf8_lossy(&buffer).to_lowercase();
    let length = headers
        .split("content-length:")
        .nth(1)
        .and_then(|rest| rest.split("\r\n").next())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    let mut body = vec![0u8; length];
    if stream.read_exact(&mut body).is_err() {
        return String::new();
    }

    String::from_utf8_lossy(&body).into_owned()
}

/// Write one reply and close, so every request gets its own connection and the
/// order they arrive in is the order they were sent in.
fn write_reply(
    stream: &mut std::net::TcpStream,
    status: u16,
    payload: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

/// A page the site served.
const SERVED: &str = r#"[{"url":"https://example.com/a","status":200,"content":{"markdown":"hello, something to read here."},"costs":{"total_cost":0.6}}]"#;

/// A page the site refused.
const REFUSED: &str = r#"[{"url":"https://example.com/a","status":403,"content":null,"costs":{"total_cost":0.5},"error":"the site refused the fetch"}]"#;

/// A client pointed at a fake, with everything else left at its default.
fn client(fake: &Fake) -> spider_cloud_agent::SpiderBuilder {
    Spider::builder()
        .key("sk-test-not-a-real-key")
        .base_url(fake.base.clone())
}

/// A router that always answers the same way, and counts.
struct Fixed {
    decision: RouteDecision,
    asked: AtomicUsize,
    told: AtomicUsize,
}

impl Fixed {
    fn new(decision: RouteDecision) -> Arc<Fixed> {
        Arc::new(Fixed {
            decision,
            asked: AtomicUsize::new(0),
            told: AtomicUsize::new(0),
        })
    }
}

impl Router for Fixed {
    fn route(&self, _input: &RouteInput<'_>) -> RouteDecision {
        self.asked.fetch_add(1, Ordering::Relaxed);
        self.decision.clone()
    }

    fn observe(&self, _input: &RouteInput<'_>, _outcome: &AttemptOutcome) {
        self.told.fetch_add(1, Ordering::Relaxed);
    }

    fn version(&self) -> RouterVersion {
        RouterVersion::Model(3)
    }
}

#[tokio::test]
async fn the_router_is_consulted_before_the_first_attempt() {
    let fake = Fake::always(200, SERVED);
    let router = Fixed::new(RouteDecision::new(
        Action::new(RequestMode::Browser).with_proxy(ProxyPool::Residential),
        RouteSource::Model,
        0.9,
    ));

    let spider = client(&fake)
        .router(Arc::clone(&router))
        .build()
        .expect("a client");
    let page = spider
        .scrape("https://example.com/a")
        .send()
        .await
        .expect("a page");

    let sent = fake.next_request();
    assert!(sent.contains("\"request\":\"browser\""), "{sent}");
    assert!(sent.contains("\"proxy\":\"residential\""), "{sent}");
    assert_eq!(router.asked.load(Ordering::Relaxed), 1);
    assert_eq!(router.told.load(Ordering::Relaxed), 1);

    let decision = page.route.as_ref().expect("the decision on the outcome");
    assert_eq!(decision.mode(), RequestMode::Browser);
    assert_eq!(decision.source, RouteSource::Model);
}

#[tokio::test]
async fn a_caller_pin_beats_the_router() {
    let fake = Fake::always(200, SERVED);
    let router = Fixed::new(RouteDecision::new(
        Action::new(RequestMode::Browser).with_proxy(ProxyPool::Residential),
        RouteSource::Model,
        0.9,
    ));

    let spider = client(&fake)
        .router(Arc::clone(&router))
        .build()
        .expect("a client");
    spider
        .scrape("https://example.com/a")
        .mode(RequestMode::Http)
        .proxy(ProxyPool::Isp)
        .send()
        .await
        .expect("a page");

    let sent = fake.next_request();
    assert!(
        sent.contains("\"request\":\"http\""),
        "the router overrode a caller's mode: {sent}"
    );
    assert!(
        sent.contains("\"proxy\":\"isp\""),
        "the router overrode a caller's pool: {sent}"
    );
    assert_eq!(
        router.asked.load(Ordering::Relaxed),
        1,
        "a pinned call still asks, so the answer is on the record"
    );
}

#[tokio::test]
async fn a_start_rung_skips_the_steps_below_it() {
    let fake = Fake::always(200, REFUSED);
    let router = Fixed::new(
        RouteDecision::new(Action::new(RequestMode::Smart), RouteSource::Model, 0.9).starting_at(2),
    );

    let spider = client(&fake)
        .router(Arc::clone(&router))
        .budget(Budget::default().with_attempts(2))
        .build()
        .expect("a client");
    let refused = spider.scrape("https://example.com/a").send().await;
    assert!(refused.is_err(), "a refused page is not a page");

    let first = fake.next_request();
    assert!(first.contains("\"request\":\"smart\""), "{first}");

    let second = fake.next_request();
    assert!(
        second.contains("\"proxy\":\"residential\""),
        "the walk did not start at the step the router named: {second}"
    );
}

#[tokio::test]
async fn without_a_start_rung_the_walk_begins_at_the_first_step() {
    let fake = Fake::always(200, REFUSED);
    let router = Fixed::new(RouteDecision::new(
        Action::new(RequestMode::Smart),
        RouteSource::Model,
        0.9,
    ));

    let spider = client(&fake)
        .router(Arc::clone(&router))
        .budget(Budget::default().with_attempts(2))
        .build()
        .expect("a client");
    let _ = spider.scrape("https://example.com/a").send().await;

    let _first = fake.next_request();
    let second = fake.next_request();
    assert!(second.contains("\"request\":\"browser\""), "{second}");
    assert!(
        !second.contains("\"proxy\":\"residential\""),
        "the walk skipped steps nobody asked it to skip: {second}"
    );
}

#[tokio::test]
async fn what_this_process_remembers_changes_the_next_call_to_the_same_site() {
    let fake = Fake::always(200, REFUSED);
    let spider = client(&fake)
        .router(HeuristicRouter::new())
        .budget(Budget::default().with_attempts(2))
        .build()
        .expect("a client");

    let _ = spider.scrape("https://example.com/a").send().await;
    let first = fake.next_request();
    assert!(
        !first.contains("\"proxy\":\"residential\""),
        "nothing was known yet, so nothing dear should have been reached for: {first}"
    );
    let _escalated = fake.next_request();

    let remembered = spider
        .site_memory()
        .get(&Url::parse("https://example.com/a").unwrap())
        .expect("a record of the site");
    assert_eq!(remembered.observations, 2);
    assert_eq!(remembered.streak, -2);

    // A different page on the same site, so only the record can be what
    // changed the answer.
    let _ = spider.scrape("https://example.com/b").send().await;
    let again = fake.next_request();
    assert!(
        again.contains("\"proxy\":\"residential\""),
        "two refusals did not change the first attempt: {again}"
    );
    assert!(again.contains("\"request\":\"browser\""), "{again}");
}

#[tokio::test]
async fn zero_capacity_keeps_routing_cold_while_sixteen_adapts() {
    for capacity in [0, 16] {
        let fake = Fake::always(200, REFUSED);
        let spider = client(&fake)
            .site_memory_capacity(capacity)
            .router(HeuristicRouter::new())
            .budget(Budget::default().with_attempts(2))
            .build()
            .expect("a client");
        assert!(spider.scrape("https://example.com/a").send().await.is_err());
        let first: serde_json::Value = serde_json::from_str(&fake.next_request()).unwrap();
        let _ = fake.next_request();
        assert!(spider.scrape("https://example.com/a").send().await.is_err());
        let second: serde_json::Value = serde_json::from_str(&fake.next_request()).unwrap();
        assert_eq!(spider.site_memory().capacity(), capacity);
        if capacity == 0 {
            assert_eq!(first, second);
            assert!(spider.site_memory().is_empty());
        } else {
            assert_ne!(first["proxy"], second["proxy"]);
            assert_eq!(second["proxy"], "residential");
            assert_eq!(second["request"], "browser");
            assert!(!spider.site_memory().is_empty());
        }
    }
}

#[tokio::test]
async fn a_recorded_row_from_a_real_call_holds_no_host() {
    const DISTINCTIVE: &str = "zqxwvutsrqponmlk";
    let path =
        std::env::temp_dir().join(format!("spider-routing-rows-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let fake = Fake::always(200, SERVED);
    let spider = client(&fake)
        .recorder(JsonlRecorder::create(&path).expect("a file"))
        .build()
        .expect("a client");

    spider
        .scrape(format!(
            "https://shop.{DISTINCTIVE}.com/checkout/{DISTINCTIVE}"
        ))
        .send()
        .await
        .expect("a page");
    let _ = fake.next_request();

    let rows = std::fs::read_to_string(&path).expect("the rows back");
    assert_eq!(rows.lines().count(), 1, "one attempt, one row");
    assert!(
        !rows.contains(DISTINCTIVE),
        "the host reached a row: {rows}"
    );
    assert!(!rows.contains("shop"), "the host reached a row: {rows}");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn only_the_routed_attempt_is_recorded() {
    let path =
        std::env::temp_dir().join(format!("spider-routing-once-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let fake = Fake::scripted(vec![(200, REFUSED.to_string()), (200, SERVED.to_string())]);
    let spider = client(&fake)
        .recorder(JsonlRecorder::create(&path).expect("a file"))
        .build()
        .expect("a client");

    spider
        .scrape("https://example.com/a")
        .send()
        .await
        .expect("the second attempt served the page");
    let _ = fake.next_request();
    let _ = fake.next_request();

    let rows = std::fs::read_to_string(&path).expect("the rows back");
    assert_eq!(
        rows.lines().count(),
        1,
        "two attempts wrote {} rows, and only the first was routed",
        rows.lines().count()
    );
    let _ = std::fs::remove_file(&path);
}

/// An address that exploration sends down the plain fetch arm, found rather
/// than assumed, so this test does not depend on the draw.
fn explores_into_http(explorer: &Explorer, routed: &RouteDecision) -> Url {
    for n in 0..10_000 {
        let url = Url::parse(&format!("https://example.com/p/{n}")).expect("a url");
        let chosen = explorer.choose(&url, routed, &Budget::unlimited());
        if chosen.map(|decision| decision.mode()) == Some(RequestMode::Http) {
            return url;
        }
    }
    panic!("no address in ten thousand explored into a plain fetch");
}

#[tokio::test]
async fn exploring_sends_a_different_action_and_never_does_it_twice() {
    let routed = RouteDecision::new(Action::new(RequestMode::Smart), RouteSource::Heuristic, 0.5);
    let explorer = Explorer::new(1.0);
    let target = explores_into_http(&explorer, &routed);

    let fake = Fake::always(200, REFUSED);
    let spider = client(&fake)
        .router(Fixed::new(routed.clone()))
        .explore(1.0)
        .budget(Budget::default().with_attempts(2))
        .build()
        .expect("a client");

    let _ = spider.scrape(target.clone()).send().await;

    let first = fake.next_request();
    assert!(
        first.contains("\"request\":\"http\""),
        "the explored action was not sent: {first}"
    );

    // The ladder owns every attempt after the first. If exploration ran again
    // it would write its own action over the step and the mode would still be
    // the plain fetch.
    let second = fake.next_request();
    assert!(
        second.contains("\"request\":\"browser\""),
        "exploration fired on a retry: {second}"
    );
}

#[tokio::test]
async fn nothing_explores_at_the_default_rate() {
    let fake = Fake::always(200, SERVED);
    let routed = RouteDecision::new(Action::new(RequestMode::Smart), RouteSource::Heuristic, 0.5);
    let target = explores_into_http(&Explorer::new(1.0), &routed);

    let spider = client(&fake)
        .router(Fixed::new(routed))
        .build()
        .expect("a client");
    let page = spider.scrape(target).send().await.expect("a page");

    let sent = fake.next_request();
    assert!(sent.contains("\"request\":\"smart\""), "{sent}");
    assert_eq!(
        page.route.as_ref().map(|decision| decision.source),
        Some(RouteSource::Heuristic)
    );
}

#[tokio::test]
async fn exploring_never_reaches_past_the_budget() {
    let fake = Fake::always(200, SERVED);
    let routed = RouteDecision::new(Action::new(RequestMode::Smart), RouteSource::Heuristic, 0.5);

    // One assumed minimum, which only the plain fetch fits inside.
    let spider = client(&fake)
        .router(Fixed::new(routed))
        .explore(1.0)
        .budget(Budget::default().with_credits(ASSUMED_MINIMUM_COST))
        .build()
        .expect("a client");

    spider
        .scrape("https://example.com/a")
        .send()
        .await
        .expect("a page");

    let sent = fake.next_request();
    assert!(
        sent.contains("\"request\":\"http\""),
        "an arm dearer than the cap was explored: {sent}"
    );
    assert!(fake.quiet(), "one call was expected");
}

#[tokio::test]
async fn exploring_never_contradicts_a_setting_the_caller_made() {
    let path = std::env::temp_dir().join(format!(
        "spider-routing-pinned-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    // A rate of one explores every page, and an explored arm is always a
    // different action from the routed one. The caller pinned the plain
    // fetch, so whatever arm is drawn contradicts it.
    let fake = Fake::always(200, SERVED);
    let spider = client(&fake)
        .explore(1.0)
        .recorder(JsonlRecorder::create(&path).expect("a file"))
        .build()
        .expect("a client");

    let page = spider
        .scrape("https://example.com/a")
        .mode(RequestMode::Http)
        .send()
        .await
        .expect("a page");

    let sent = fake.next_request();
    assert!(sent.contains("\"request\":\"http\""), "{sent}");

    // What the outcome reports is the action that was sent, not one that was
    // drawn and then overruled.
    let decision = page.route.as_ref().expect("the decision on the outcome");
    assert_eq!(decision.mode(), RequestMode::Http, "{decision:?}");
    assert_eq!(decision.source, RouteSource::Caller, "{decision:?}");

    let rows = std::fs::read_to_string(&path).expect("the rows back");
    assert!(
        rows.contains("\"mode\":\"http\""),
        "the row claims an action that was never sent: {rows}"
    );
    let _ = std::fs::remove_file(&path);
}

/// A login wall, served as a page with a status the site chose.
const WALL: &str = r#"[{"url":"https://example.com/account","status":401,"content":null,"costs":{"total_cost":0.5},"error":"sign in first"}]"#;

#[tokio::test]
async fn a_login_wall_turns_on_the_session_when_the_caller_supplied_cookies() {
    let fake = Fake::always(200, WALL);
    let spider = client(&fake)
        .budget(Budget::default().with_attempts(2))
        .build()
        .expect("a client");

    let mut call = spider.scrape("https://example.com/account");
    call.params_mut().cookies = Some("sid=abc".into());
    let _ = call.send().await;

    let first = fake.next_request();
    assert!(!first.contains("\"session\":true"), "{first}");

    // The second call is the one the session step produces. Without it the
    // wall is read as final and nothing else is sent.
    let second = fake.next_request();
    assert!(
        second.contains("\"session\":true"),
        "the session rung was not applied: {second}"
    );
    assert!(second.contains("sid=abc"), "{second}");
}

#[tokio::test]
async fn the_site_store_stays_bounded_across_many_sites() {
    let fake = Fake::always(200, SERVED);
    let spider = client(&fake)
        .site_memory_capacity(32)
        .build()
        .expect("a client");

    for n in 0..200 {
        let _ = spider
            .scrape(format!("https://site{n}.example{n}.com/a"))
            .send()
            .await;
        let _ = fake.next_request();
    }

    assert_eq!(spider.site_memory().capacity(), 32);
    assert!(spider.site_memory().len() <= 32);
    assert!(
        spider.site_memory().len() > 4,
        "nothing was remembered, so the bound proves nothing"
    );
}
