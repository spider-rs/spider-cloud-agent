//! The HTTP layer, and nothing else.
//!
//! This module sends one request and reads one response. It does not retry, it
//! does not back off, and it does not decide what to try next. Those belong to
//! [`crate::policy`] and to the send loop in [`crate::client`], because a retry
//! here would sit inside the retry there and neither layer would know the real
//! attempt count.
//!
//! What it does own:
//!
//! - the address, which is `https://api.spider.cloud` unless `SPIDER_API_URL` says otherwise
//! - the bearer header, which is the only place the key is written
//! - the rate limit headers, kept in an atomic snapshot the policy engine reads
//! - the split between the two status planes, which is the part worth reading twice
//!
//! The two planes come apart here. The HTTP status of the call becomes an
//! [`ApiStatus`]. The `status` field inside the body, which is what the target
//! site answered, becomes a [`PageStatus`]. One server behaviour makes that
//! harder than it sounds: a top level body status of 400 or above is copied onto
//! the HTTP status, so a body saying the site returned 503 arrives as an HTTP
//! 503. [`Transport::read`] recognises that case by the shape of the body and
//! keeps it on the target plane, where a retry is worth making, rather than
//! reporting the service as down.

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::error::Error;
use crate::params::ReturnFormat;
use crate::response::content::MultiBody;
use crate::response::costs::Costs;
use crate::response::metadata::Metadata;
use crate::response::page::{PageParts, PageResult, Pages};
use crate::response::Body;
use crate::status::{ApiStatus, PageStatus};
use crate::Result;

/// Where the API lives when nothing says otherwise.
pub const DEFAULT_BASE_URL: &str = "https://api.spider.cloud";

/// The environment variable that moves the client to another address.
pub const BASE_URL_ENV: &str = "SPIDER_API_URL";

/// What this crate calls itself on the wire.
pub const USER_AGENT: &str = concat!(
    "spider-cloud-agent/",
    env!("CARGO_PKG_VERSION"),
    " (+https://spider.cloud)"
);

/// Shown in place of the key wherever a value is printed.
const REDACTED: &str = "<redacted>";

/// How long the default client waits for a connection to open.
///
/// A connection that has not opened in this long is not going to. Without a
/// ceiling, a dropped packet on the way in left the caller waiting on the
/// operating system's own retries, which run to minutes.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// routes
// ---------------------------------------------------------------------------

/// Whether a route is read or write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Method {
    /// A read. Carries no body.
    Get,
    /// A write. Carries a JSON body.
    Post,
}

impl Method {
    /// The verb as it goes on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One address this crate is allowed to call.
///
/// Routes are constants rather than strings built at the call site, so the set
/// of paths the crate can reach is a list that can be read and tested.
/// `tests/endpoints.rs` asserts that [`ROUTES`] is exactly that list.
///
/// A path may carry `{name}` placeholders, which [`Route::render`] fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Route {
    /// The verb.
    pub method: Method,
    /// The path, starting with a slash, with `{name}` for the parts supplied at
    /// the call site.
    pub path: &'static str,
}

impl Route {
    /// A read route.
    pub const fn get(path: &'static str) -> Route {
        Route {
            method: Method::Get,
            path,
        }
    }

    /// A write route.
    pub const fn post(path: &'static str) -> Route {
        Route {
            method: Method::Post,
            path,
        }
    }

    /// How many `{name}` placeholders the path carries.
    pub fn slots(&self) -> usize {
        self.path.matches('{').count()
    }

    /// The path with its placeholders filled in, left to right.
    ///
    /// Each argument is percent encoded, except that a slash is kept, so a path
    /// argument such as `blog/2026/a-post` stays one path.
    ///
    /// Returns [`Error::Config`] when the number of arguments does not match the
    /// number of placeholders, because a half filled path would be a call to
    /// somewhere nobody meant.
    ///
    /// Returns [`Error::Config`] as well for an argument carrying a `.` or `..`
    /// segment. Keeping the slash is what lets one argument be a whole path, and
    /// the cost of that is a dot segment: [`Url::join`] resolves it before the
    /// request goes out, so `render` on `/data/{table}` with `../v1/scrape`
    /// would leave `/data` and call something else. A route decides which part
    /// of the API an argument can reach, and an argument that walks out of it is
    /// a mistake in the caller rather than a path to send.
    pub fn render(&self, args: &[&str]) -> Result<String> {
        if args.len() != self.slots() {
            return Err(Error::Config(format!(
                "{} takes {} path arguments, got {}",
                self.path,
                self.slots(),
                args.len()
            )));
        }
        for arg in args {
            if arg
                .split('/')
                .any(|segment| segment == "." || segment == "..")
            {
                return Err(Error::Config(format!(
                    "{} was given the path argument {arg:?}, which carries a dot \
                     segment and would resolve to somewhere outside the route",
                    self.path
                )));
            }
        }
        let mut out = String::with_capacity(self.path.len() + 32);
        let mut rest = self.path;
        let mut args = args.iter();
        // Every index here comes from `find` on an ASCII delimiter, so it is
        // already a character boundary. The fallible accessors say that in the
        // type rather than in a comment a later reader has to trust.
        while let Some(open) = rest.find('{') {
            let Some(tail) = rest.get(open..) else { break };
            let close = match tail.find('}') {
                Some(offset) => open + offset,
                None => {
                    return Err(Error::Config(format!(
                        "route path {} has an unclosed placeholder",
                        self.path
                    )))
                }
            };
            let (Some(head), Some(next)) = (rest.get(..open), rest.get(close + 1..)) else {
                break;
            };
            out.push_str(head);
            let arg = args.next().unwrap_or(&"");
            out.push_str(&encode_path(arg));
            rest = next;
        }
        out.push_str(rest);
        Ok(out)
    }
}

impl fmt::Display for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.method, self.path)
    }
}

/// Percent encode one path argument, keeping slashes so a multi segment path
/// stays a path.
fn encode_path(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Every address the crate can call.
///
/// This module is the whole of it. Nothing builds a path from a string at the
/// call site, so adding an endpoint means adding a constant here and to the
/// table in [`ROUTES`], which `tests/endpoints.rs` checks against a literal
/// list.
pub mod route {
    use super::Route;

    /// Fetch one page.
    pub const SCRAPE: Route = Route::post("/scrape");
    /// Fetch a site, following links.
    pub const CRAWL: Route = Route::post("/crawl");
    /// Collect links without returning page content.
    pub const LINKS: Route = Route::post("/links");
    /// Run a query and, if asked, fetch what it finds.
    pub const SEARCH: Route = Route::post("/search");
    /// Take a picture of a page.
    pub const SCREENSHOT: Route = Route::post("/screenshot");
    /// Convert markup you already hold into another format.
    pub const TRANSFORM: Route = Route::post("/transform");

    /// The versioned twin of [`SCRAPE`].
    pub const V1_SCRAPE: Route = Route::post("/v1/scrape");
    /// The versioned twin of [`CRAWL`].
    pub const V1_CRAWL: Route = Route::post("/v1/crawl");
    /// The versioned twin of [`LINKS`].
    pub const V1_LINKS: Route = Route::post("/v1/links");
    /// The versioned twin of [`SEARCH`].
    pub const V1_SEARCH: Route = Route::post("/v1/search");
    /// The versioned twin of [`SCREENSHOT`].
    pub const V1_SCREENSHOT: Route = Route::post("/v1/screenshot");
    /// The versioned twin of [`TRANSFORM`].
    pub const V1_TRANSFORM: Route = Route::post("/v1/transform");

    /// [`SCRAPE`] on a plan that charges for seats rather than per page.
    pub const UNLIMITED_SCRAPE: Route = Route::post("/unlimited/scrape");
    /// [`CRAWL`] on a plan that charges for seats rather than per page.
    pub const UNLIMITED_CRAWL: Route = Route::post("/unlimited/crawl");
    /// [`LINKS`] on a plan that charges for seats rather than per page.
    pub const UNLIMITED_LINKS: Route = Route::post("/unlimited/links");

    /// [`SCRAPE`] with a prompt applied to the page.
    pub const AI_SCRAPE: Route = Route::post("/ai/scrape");
    /// [`CRAWL`] with a prompt applied to each page.
    pub const AI_CRAWL: Route = Route::post("/ai/crawl");
    /// [`SEARCH`] with a prompt applied to the results.
    pub const AI_SEARCH: Route = Route::post("/ai/search");
    /// A prompt that drives a page rather than only reading it.
    pub const AI_BROWSER: Route = Route::post("/ai/browser");
    /// [`LINKS`] with a prompt deciding which links matter.
    pub const AI_LINKS: Route = Route::post("/ai/links");

    /// Read one path on one host under the stored config for that path.
    ///
    /// The target is the path rather than a `url` field, and the body is
    /// overrides rather than the whole request. Post only: a get here is a 400.
    pub const FETCH: Route = Route::post("/fetch/{domain}/{path}");

    /// What is left on the account.
    pub const DATA_CREDITS: Route = Route::get("/data/credits");
    /// The record of past crawls.
    pub const DATA_CRAWL_LOGS: Route = Route::get("/data/crawl_logs");
    /// One stored table by name.
    pub const DATA_TABLE: Route = Route::get("/data/{table}");
}

/// Every route, in one table.
///
/// `tests/endpoints.rs` compares this against a literal list and fails on any
/// difference in either direction, so a path cannot be added quietly.
pub const ROUTES: &[Route] = &[
    route::SCRAPE,
    route::CRAWL,
    route::LINKS,
    route::SEARCH,
    route::SCREENSHOT,
    route::TRANSFORM,
    route::V1_SCRAPE,
    route::V1_CRAWL,
    route::V1_LINKS,
    route::V1_SEARCH,
    route::V1_SCREENSHOT,
    route::V1_TRANSFORM,
    route::UNLIMITED_SCRAPE,
    route::UNLIMITED_CRAWL,
    route::UNLIMITED_LINKS,
    route::AI_SCRAPE,
    route::AI_CRAWL,
    route::AI_SEARCH,
    route::AI_BROWSER,
    route::AI_LINKS,
    route::FETCH,
    route::DATA_CREDITS,
    route::DATA_CRAWL_LOGS,
    route::DATA_TABLE,
];

// ---------------------------------------------------------------------------
// rate limits
// ---------------------------------------------------------------------------

/// What the service last said about how much room is left.
///
/// The policy engine reads this before deciding to send again, which is the
/// only reason the transport parses these headers at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimit {
    /// Calls allowed in the current window.
    pub limit: Option<u32>,
    /// Calls left in the current window.
    pub remaining: Option<u32>,
    /// How long until the window starts again.
    pub reset: Option<Duration>,
    /// How long the service asked you to wait, from `Retry-After`.
    pub retry_after: Option<Duration>,
}

impl RateLimit {
    /// Whether the service has reported anything yet.
    pub fn is_known(&self) -> bool {
        self != &RateLimit::default()
    }

    /// What one response's headers say, and nothing older.
    fn from_headers(headers: &reqwest::header::HeaderMap) -> RateLimit {
        let seconds = |name: &str| header_number(headers, name).map(Duration::from_secs);
        RateLimit {
            limit: header_number(headers, "ratelimit-limit").map(count),
            remaining: header_number(headers, "ratelimit-remaining").map(count),
            reset: seconds("ratelimit-reset"),
            retry_after: seconds("retry-after"),
        }
    }

    /// The longest wait either header asked for.
    pub fn wait(&self) -> Option<Duration> {
        match (self.retry_after, self.reset) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

/// The snapshot, stored so any thread holding the client can read the latest
/// values without waiting on a lock.
#[derive(Debug, Default)]
struct RateLimitCell {
    limit: AtomicI64,
    remaining: AtomicI64,
    reset: AtomicI64,
    retry_after: AtomicI64,
}

/// Stored for a header that has not arrived.
const UNSET: i64 = -1;

impl RateLimitCell {
    fn new() -> RateLimitCell {
        RateLimitCell {
            limit: AtomicI64::new(UNSET),
            remaining: AtomicI64::new(UNSET),
            reset: AtomicI64::new(UNSET),
            retry_after: AtomicI64::new(UNSET),
        }
    }

    fn store(&self, slot: &AtomicI64, value: Option<u64>) {
        if let Some(value) = value {
            // Already held under the ceiling by `header_number`, so the fallback
            // is never taken. It is written out rather than cast so the type
            // says so.
            slot.store(i64::try_from(value).unwrap_or(i64::MAX), Ordering::Relaxed);
        }
    }

    fn read(slot: &AtomicI64) -> Option<u64> {
        match slot.load(Ordering::Relaxed) {
            UNSET => None,
            value => u64::try_from(value).ok(),
        }
    }

    fn snapshot(&self) -> RateLimit {
        let seconds = |slot: &AtomicI64| RateLimitCell::read(slot).map(Duration::from_secs);
        RateLimit {
            limit: RateLimitCell::read(&self.limit).map(count),
            remaining: RateLimitCell::read(&self.remaining).map(count),
            reset: seconds(&self.reset),
            retry_after: seconds(&self.retry_after),
        }
    }
}

/// A count as the snapshot holds it.
///
/// A value past what the field holds saturates rather than wraps. Cast, a limit
/// of 2^32 read as zero, which says the opposite of what the service said.
fn count(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Read one header as a whole number of seconds or calls.
///
/// A negative or unreadable value is treated as absent rather than as zero,
/// because zero means "none left" and would make a healthy client stop. A
/// value past what the cell holds is kept at the cell's ceiling, which is
/// still an age.
fn header_number(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    let raw = headers.get(name)?.to_str().ok()?.trim();
    let value = raw.parse::<u64>().ok()?;
    Some(value.min(i64::MAX as u64))
}

// ---------------------------------------------------------------------------
// the transport
// ---------------------------------------------------------------------------

/// One HTTP client pointed at the API.
///
/// Reachable from [`crate::Spider::raw`], where it is the escape hatch: typed
/// parameters, typed responses, and no policy between you and the service.
///
/// `Debug` prints the address but never the key.
pub struct Transport {
    client: reqwest::Client,
    base: Url,
    key: String,
    limits: Arc<RateLimitCell>,
}

impl fmt::Debug for Transport {
    /// Prints the address and says the key is held, never what it is.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("base", &self.base.as_str())
            .field("key", &REDACTED)
            .field("rate_limit", &self.rate_limit())
            .finish()
    }
}

impl Transport {
    /// A transport for one key, against the default address.
    pub fn new(key: impl Into<String>) -> Result<Transport> {
        Transport::with_base(key, Transport::base_url_from_env()?)
    }

    /// A transport for one key, against an address you name.
    ///
    /// The client it builds gives up on a connection that has not opened after
    /// thirty seconds. It sets no ceiling on the answer itself, because a
    /// crawl can legitimately take minutes: the wall on a [`crate::Budget`] is
    /// what bounds that, per operation, and [`Transport::with_client`] takes a
    /// client with whatever timeouts you want.
    pub fn with_base(key: impl Into<String>, base: Url) -> Result<Transport> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(Error::Transport)?;
        Ok(Transport::with_client(key, base, client))
    }

    /// A transport built on a client you already have, so connection pools and
    /// timeouts stay yours.
    ///
    /// A base whose path does not end in a slash is given one. [`Url::join`]
    /// reads the last segment of a base as a file and replaces it, so without
    /// this `https://proxy.example/spider` plus `scrape` called
    /// `https://proxy.example/scrape`, an address nobody named.
    pub fn with_client(key: impl Into<String>, base: Url, client: reqwest::Client) -> Transport {
        let mut base = base;
        if !base.cannot_be_a_base() && !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Transport {
            client,
            base,
            key: key.into(),
            limits: Arc::new(RateLimitCell::new()),
        }
    }

    /// The address from `SPIDER_API_URL`, or [`DEFAULT_BASE_URL`].
    pub fn base_url_from_env() -> Result<Url> {
        match std::env::var("SPIDER_API_URL") {
            Ok(value) if !value.trim().is_empty() => Url::parse(value.trim())
                .map_err(|e| Error::Config(format!("{BASE_URL_ENV} is not a url: {e}"))),
            _ => Url::parse(DEFAULT_BASE_URL)
                .map_err(|e| Error::Config(format!("the default address is not a url: {e}"))),
        }
    }

    /// The address this transport calls.
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// What the service last said about the rate limit.
    pub fn rate_limit(&self) -> RateLimit {
        self.limits.snapshot()
    }

    /// Send a body to a route and read the reply.
    ///
    /// One attempt. A failure here is the caller's to act on.
    pub async fn post<B: Serialize>(&self, route: Route, args: &[&str], body: &B) -> Result<Reply> {
        if route.method != Method::Post {
            return Err(Error::Config(format!("{route} is not a post route")));
        }
        let url = self.url_for(route, args, &[])?;
        let request = self.client.post(url).json(body);
        self.execute(request).await
    }

    /// Read a route, with an optional query string.
    ///
    /// One attempt, same as [`Transport::post`].
    pub async fn get(&self, route: Route, args: &[&str], query: &[(&str, &str)]) -> Result<Reply> {
        if route.method != Method::Get {
            return Err(Error::Config(format!("{route} is not a get route")));
        }
        let url = self.url_for(route, args, query)?;
        let request = self.client.get(url);
        self.execute(request).await
    }

    /// The full address for a route, placeholders filled and query attached.
    pub fn url_for(&self, route: Route, args: &[&str], query: &[(&str, &str)]) -> Result<Url> {
        let path = route.render(args)?;
        let mut url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|e| Error::Config(format!("cannot build an address for {route}: {e}")))?;
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    async fn execute(&self, request: reqwest::RequestBuilder) -> Result<Reply> {
        let started = Instant::now();
        let response = request
            .header(reqwest::header::AUTHORIZATION, self.bearer())
            .send()
            .await
            .map_err(Error::Transport)?;

        let status = ApiStatus::new(response.status().as_u16());
        let headers = response.headers().clone();
        self.record_limits(&headers);
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_ascii_lowercase());
        let body = response.bytes().await.map_err(Error::Transport)?;

        // The reply carries this response's headers and not the snapshot. The
        // snapshot outlives the response, and a reset one answer named was
        // being read off every later reply as a wait the service had asked
        // for, so one header set the wait for every retry after it.
        let rate_limit = RateLimit::from_headers(&headers);
        Ok(Reply {
            status,
            retry_after: rate_limit.retry_after,
            rate_limit,
            elapsed: started.elapsed(),
            content_type,
            body,
        })
    }

    /// The one place the key is written.
    fn bearer(&self) -> String {
        format!("Bearer {}", self.key)
    }

    fn record_limits(&self, headers: &reqwest::header::HeaderMap) {
        let cell = &self.limits;
        cell.store(&cell.limit, header_number(headers, "ratelimit-limit"));
        cell.store(
            &cell.remaining,
            header_number(headers, "ratelimit-remaining"),
        );
        cell.store(&cell.reset, header_number(headers, "ratelimit-reset"));
        cell.store(&cell.retry_after, header_number(headers, "retry-after"));
    }
}

// ---------------------------------------------------------------------------
// the reply
// ---------------------------------------------------------------------------

/// One response, read but not yet judged.
///
/// Holding the bytes rather than a parsed value is deliberate: which type the
/// body reads into depends on the route, and the send loop needs the status and
/// the headers before it decides whether the body is worth parsing at all.
#[derive(Debug, Clone)]
pub struct Reply {
    /// The status of the call to the service. Not the target site's status.
    pub status: ApiStatus,
    /// What the service said about the rate limit on this response.
    pub rate_limit: RateLimit,
    /// The `Retry-After` header, when it was sent.
    pub retry_after: Option<Duration>,
    /// How long the call took, end to end.
    pub elapsed: Duration,
    /// The declared content type, lowercased.
    pub content_type: Option<String>,
    /// The body, exactly as it arrived.
    pub body: Bytes,
}

impl Reply {
    /// Whether the call itself succeeded. Says nothing about the target site.
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    /// The body as text, when it is valid UTF-8.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }

    /// Read the body into a type.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(Error::Decode)
    }

    /// The error this reply stands for, when the call failed.
    ///
    /// Running out of credits gets its own variant rather than a status, because
    /// it is the one failure where trying again is strictly worse.
    pub fn as_error(&self) -> Option<Error> {
        if self.is_success() {
            return None;
        }
        let message = self.message();
        Some(match self.status.code() {
            402 => Error::InsufficientCredits,
            401 => Error::Auth(
                message.unwrap_or_else(|| "the key was missing or rejected".to_string()),
            ),
            _ => Error::Api {
                status: self.status,
                message,
                retry_after: self.retry_after.or(self.rate_limit.reset),
            },
        })
    }

    /// Fail when the call failed, and hand the reply back when it did not.
    pub fn into_result(self) -> Result<Reply> {
        match self.as_error() {
            Some(error) => Err(error),
            None => Ok(self),
        }
    }

    /// What the service said went wrong, when the body carried a message.
    ///
    /// The key is never part of a response body, and nothing from the request is
    /// copied in here, so an error message cannot carry the key.
    pub fn message(&self) -> Option<String> {
        let value: serde_json::Value = serde_json::from_slice(&self.body).ok()?;
        for key in ["error", "message", "detail"] {
            if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
                if !text.trim().is_empty() {
                    return Some(text.trim().to_string());
                }
            }
        }
        None
    }

    /// Whether this failing reply is really the target site's status wearing the
    /// call's clothes.
    ///
    /// The service copies a top level body status of 400 or above onto the HTTP
    /// status, so a page the site answered 503 for arrives as an HTTP 503. Read
    /// naively that says the service is down, and the policy engine would wait
    /// on the wrong thing.
    ///
    /// The tell is the body: a page response carries a `url` and a `status` that
    /// match the HTTP status, while an account level failure carries only a
    /// message. Two codes never mirror, whatever the body says: 401 and 402 are
    /// about the key and the balance, and treating either as a page would turn a
    /// stop into a retry.
    pub fn is_mirrored_page_status(&self) -> bool {
        if self.status.is_success() || matches!(self.status.code(), 401 | 402) {
            return false;
        }
        let value: serde_json::Value = match serde_json::from_slice(&self.body) {
            Ok(value) => value,
            Err(_) => return false,
        };
        let first = match &value {
            serde_json::Value::Array(items) => match items.first() {
                Some(first) => first,
                None => return false,
            },
            other => other,
        };
        let has_url = first.get("url").and_then(|v| v.as_str()).is_some();
        let same_status = first
            .get("status")
            .and_then(|v| v.as_u64())
            .is_some_and(|code| code == u64::from(self.status.code()));
        has_url && same_status
    }

    /// Sort this reply into pages.
    ///
    /// `requested` is the address that was asked for, used when the body does not
    /// name one. `format` is the format the request asked for, which is the only
    /// way to tell a raw field holding markdown from one holding markup.
    pub fn read(&self, requested: &Url, format: Option<ReturnFormat>) -> Result<Pages> {
        let value: serde_json::Value = self.json()?;
        let items = match value {
            serde_json::Value::Array(items) => items,
            serde_json::Value::Null => Vec::new(),
            other => converted_documents(other),
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(self.read_one(item, requested, format)?);
        }
        Ok(Pages(out))
    }

    fn read_one(
        &self,
        value: serde_json::Value,
        requested: &Url,
        format: Option<ReturnFormat>,
    ) -> Result<PageResult> {
        let wire: WirePage = serde_json::from_value(value).map_err(Error::Decode)?;
        let url = wire
            .url
            .as_deref()
            .and_then(|u| Url::parse(u).ok())
            .unwrap_or_else(|| requested.clone());
        // An absent status means the call reached the site and the service saw no
        // reason to say otherwise, so the call's own status stands in.
        let status = PageStatus::new(wire.status.unwrap_or(self.status.code()));
        let links = wire.links.map(|links| {
            links
                .iter()
                .filter_map(|link| Url::parse(link).ok().or_else(|| url.join(link).ok()))
                .collect()
        });
        Ok(PageResult::from_parts(PageParts {
            url,
            status,
            body: match wire.css_extracted {
                // A request that named selectors asked for those fields, so they
                // are the body. Asking for them alongside a page format is
                // unusual, and the fields are still the more specific answer.
                Some(fields) if !fields.is_empty() => Body::Fields(fields),
                _ => body_from(wire.content, format),
            },
            duration: self.elapsed,
            costs: wire.costs.unwrap_or_default(),
            metadata: wire.metadata,
            links,
            headers: wire.headers.or(wire.response_headers),
            cookies: wire.cookies.or(wire.response_cookies),
            error: wire.error,
        }))
    }
}

/// One item per converted document, for the one endpoint that answers with a
/// list of them.
///
/// The transform endpoint sends a single object whose `content` is an array of
/// strings, one per document in the request. Everywhere else `content` holds one
/// document, and an array there is the page as bytes, so the reader asked for
/// numbers and a conversion failed with `invalid type: string "# Title\nBody
/// text.", expected u8`. Measured against the live service on 2026-09-15, which
/// answered `{"content":["# Title\nBody text."]}` and could not be read at all.
///
/// Anything else is one item, as before. The tell is narrow on purpose: a
/// `content` array of strings is a shape no page response uses.
///
/// Whatever sits beside `content` describes the call rather than any one
/// document, and the wire sends one of each, so it stays with the first
/// document. Copying a cost block onto every document would report a bill of as
/// many times the charge as there were documents.
fn converted_documents(value: serde_json::Value) -> Vec<serde_json::Value> {
    let mut object = match value {
        serde_json::Value::Object(object) => object,
        other => return vec![other],
    };
    let documents = match object.get("content") {
        Some(serde_json::Value::Array(items))
            if !items.is_empty() && items.iter().all(serde_json::Value::is_string) =>
        {
            items.clone()
        }
        _ => return vec![serde_json::Value::Object(object)],
    };
    let mut out = Vec::with_capacity(documents.len());
    for (position, document) in documents.into_iter().enumerate() {
        let mut item = match position {
            0 => std::mem::take(&mut object),
            _ => serde_json::Map::new(),
        };
        item.insert("content".to_string(), document);
        out.push(serde_json::Value::Object(item));
    }
    out
}

/// One page as the API writes it.
///
/// Header and cookie fields are read under two names each. The request
/// parameters that ask for them are `return_headers` and `return_cookies`, and
/// which key the response uses was not something the wire could be made to say
/// without an account, so both spellings are accepted and whichever arrives wins.
#[derive(Debug, Default, serde::Deserialize)]
struct WirePage {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    content: Option<MultiBody>,
    // Extractions come back here, at the top level, not in metadata. The
    // metadata block has an extracted_data field that stays null, which is the
    // obvious place to look and the wrong one. Confirmed against the live API
    // on 2026-09-15.
    #[serde(default)]
    css_extracted: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    links: Option<Vec<String>>,
    #[serde(default)]
    metadata: Option<Metadata>,
    #[serde(default)]
    costs: Option<Costs>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    response_headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    cookies: Option<String>,
    #[serde(default)]
    response_cookies: Option<String>,
}

/// Turn the `content` field into a typed body.
///
/// The wire sends one field called `raw` whatever was asked for, so the format
/// the request named is what tells markdown from markup. With no format named,
/// raw is read as markup, which is the API's own default.
fn body_from(content: Option<MultiBody>, format: Option<ReturnFormat>) -> Body {
    let multi = match content {
        Some(multi) => multi,
        None => return Body::Empty,
    };
    if multi.is_empty() && multi.present().is_empty() {
        return Body::Empty;
    }
    let present = multi.present();
    let [only] = present[..] else {
        return Body::Multi(multi);
    };
    match only {
        "text" => multi.text.map(Body::Text).unwrap_or(Body::Empty),
        "markdown" => multi.markdown.map(Body::Markdown).unwrap_or(Body::Empty),
        "html2text" => multi.html2text.map(Body::Text).unwrap_or(Body::Empty),
        "bytes" => multi.bytes.map(Body::Bytes).unwrap_or(Body::Empty),
        "screenshot" => multi
            .screenshot
            .map(Body::Screenshot)
            .unwrap_or(Body::Empty),
        "raw" => match multi.raw {
            Some(raw) => match format {
                Some(ReturnFormat::Markdown) | Some(ReturnFormat::Commonmark) => {
                    Body::Markdown(raw)
                }
                Some(ReturnFormat::Text) => Body::Text(raw),
                Some(ReturnFormat::Xml) => Body::Xml(raw),
                Some(ReturnFormat::Bytes) => Body::Bytes(Bytes::from(raw.into_bytes())),
                Some(ReturnFormat::Empty) => Body::Empty,
                Some(ReturnFormat::Raw) | None => Body::Html(raw),
            },
            None => Body::Empty,
        },
        _ => Body::Multi(multi),
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
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;

    fn reply(code: u16, body: &str) -> Reply {
        Reply {
            status: ApiStatus::new(code),
            rate_limit: RateLimit::default(),
            retry_after: None,
            elapsed: Duration::from_millis(40),
            content_type: Some("application/json".into()),
            body: Bytes::from(body.to_string()),
        }
    }

    /// One recorded response, read off disk.
    ///
    /// The fixtures are the same files `tests/deser.rs` reads. They are loaded at
    /// run time rather than compiled in, because the packaged crate carries only
    /// `src`, and a build of the package would then fail on a file that is not
    /// there.
    fn fixture(name: &str) -> Reply {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        reply(200, &text)
    }

    #[test]
    fn a_recorded_page_becomes_a_typed_page() {
        let pages = fixture("scrape_markdown.json")
            .read(
                &Url::parse("https://example.com/pricing").unwrap(),
                Some(ReturnFormat::Markdown),
            )
            .expect("pages");
        let page = pages.first_ok().expect("a page");

        assert_eq!(page.url.as_str(), "https://example.com/pricing");
        assert_eq!(page.status.code(), 200);
        assert!(matches!(page.body, Body::Markdown(_)));
        assert!(page.text().expect("text").starts_with("# Pricing"));
        // The fixture sends 0.6 dollars, which is 6,000 credits.
        assert_eq!(page.cost(), crate::credits::Credits::from_usd(0.6));
        assert_eq!(
            page.metadata.as_ref().expect("metadata").title.as_deref(),
            Some("Pricing")
        );
    }

    #[test]
    fn a_recorded_crawl_sorts_itself_into_pages_and_refusals() {
        let pages = fixture("crawl_mixed.json")
            .read(&Url::parse("https://example.org/").unwrap(), None)
            .expect("pages");

        assert_eq!(pages.len(), 3);
        assert_eq!(pages.ok().count(), 2);
        assert_eq!(pages.failed().count(), 1);

        let failed = pages.failed().next().expect("a refusal");
        assert_eq!(failed.status.code(), 403);
        assert!(failed.was_billed());
        assert_eq!(failed.hint, crate::response::Hint::TryResidentialProxy);

        // The 200 with nothing in it is a page, and it says so.
        assert_eq!(pages.ok().filter(|p| p.is_blank()).count(), 1);
        // Three pages at 0.4 dollars each, so 12,000 credits. Compared with a
        // tolerance because adding three tenths back up does not land on
        // exactly 1.2.
        assert!((pages.total_cost().get() - 12_000.0).abs() < 1e-6);
    }

    #[test]
    fn a_recorded_refusal_never_becomes_a_page() {
        let pages = fixture("blocked_page.json")
            .read(&Url::parse("https://example.net/checkout").unwrap(), None)
            .expect("pages");
        assert!(pages.first_ok().is_none());
        let failed = pages.failed().next().expect("a refusal");
        assert_eq!(failed.error.as_deref(), Some("the site refused the fetch"));
    }

    #[test]
    fn a_recorded_picture_arrives_as_bytes() {
        let pages = fixture("screenshot.json")
            .read(&Url::parse("https://example.com/").unwrap(), None)
            .expect("pages");
        let page = pages.first_ok().expect("a page");
        assert!(matches!(page.body, Body::Screenshot(_)));
        assert_eq!(page.body.len(), 8);
        assert_eq!(page.text(), None);
    }

    #[test]
    fn a_recorded_page_keeps_its_headers_and_cookies() {
        let pages = fixture("multi_format.json")
            .read(&Url::parse("https://httpbin.org/html").unwrap(), None)
            .expect("pages");
        let page = pages.first_ok().expect("a page");
        assert!(matches!(page.body, Body::Multi(_)));
        assert_eq!(page.text(), Some("Herman Melville"));
        assert_eq!(
            page.headers.as_ref().expect("headers").get("content-type"),
            Some(&"text/html; charset=utf-8".to_string())
        );
        assert!(page
            .cookies
            .as_deref()
            .expect("cookies")
            .contains("session"));
    }

    #[test]
    fn the_route_table_has_no_duplicates_and_no_pipeline() {
        let mut seen: Vec<String> = ROUTES.iter().map(|r| r.to_string()).collect();
        let before = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), before, "a route is listed twice");
        assert!(
            !ROUTES.iter().any(|r| r.path.contains("pipeline")),
            "that path is not part of the public api"
        );
    }

    #[test]
    fn a_route_fills_its_placeholders() {
        assert_eq!(
            route::FETCH
                .render(&["example.com", "blog/a post"])
                .unwrap(),
            "/fetch/example.com/blog/a%20post"
        );
        assert_eq!(
            route::DATA_TABLE.render(&["crawl_logs"]).unwrap(),
            "/data/crawl_logs"
        );
        assert_eq!(route::SCRAPE.render(&[]).unwrap(), "/scrape");
    }

    #[test]
    fn a_route_refuses_the_wrong_number_of_arguments() {
        assert!(route::FETCH.render(&["example.com"]).is_err());
        assert!(route::SCRAPE.render(&["extra"]).is_err());
    }

    #[test]
    fn a_reply_keeps_the_two_planes_apart() {
        let pages = reply(
            200,
            r#"[{"url":"https://example.com/a","status":403,"content":null}]"#,
        )
        .read(&Url::parse("https://example.com/a").unwrap(), None)
        .expect("pages");
        assert_eq!(pages.len(), 1);
        let failed = pages.failed().next().expect("a failure");
        assert_eq!(failed.status.code(), 403);
    }

    #[test]
    fn a_body_status_mirrored_onto_the_call_stays_on_the_target_plane() {
        let mirrored = reply(503, r#"{"url":"https://example.com/a","status":503}"#);
        assert!(mirrored.is_mirrored_page_status());

        let service_down = reply(503, r#"{"error":"try again shortly"}"#);
        assert!(!service_down.is_mirrored_page_status());
    }

    #[test]
    fn the_balance_and_the_key_never_mirror() {
        for code in [401u16, 402] {
            let r = reply(code, r#"{"url":"https://example.com/a","status":403}"#);
            assert!(
                !r.is_mirrored_page_status(),
                "code {code} must stay on the call plane"
            );
        }
    }

    #[test]
    fn running_out_of_credits_is_its_own_error() {
        let error = reply(402, r#"{"error":"balance is zero"}"#)
            .as_error()
            .expect("an error");
        assert!(matches!(error, Error::InsufficientCredits));
        assert!(!error.is_retryable());
    }

    #[test]
    fn a_rejected_key_reports_what_the_service_said_and_nothing_else() {
        let error = reply(401, r#"{"error":"invalid api key"}"#)
            .as_error()
            .expect("an error");
        match error {
            Error::Auth(message) => assert_eq!(message, "invalid api key"),
            other => panic!("expected an auth error, got {other:?}"),
        }
    }

    #[test]
    fn a_success_is_not_an_error() {
        assert!(reply(200, "[]").as_error().is_none());
        assert!(reply(204, "").as_error().is_none());
    }

    #[test]
    fn the_format_that_was_asked_for_names_the_body() {
        let json = r#"[{"url":"https://example.com/a","status":200,"content":{"raw":"Heading"}}]"#;
        let url = Url::parse("https://example.com/a").unwrap();

        let markdown = reply(200, json)
            .read(&url, Some(ReturnFormat::Markdown))
            .unwrap();
        assert!(matches!(
            markdown.first_ok().unwrap().body,
            Body::Markdown(_)
        ));

        let raw = reply(200, json).read(&url, None).unwrap();
        assert!(matches!(raw.first_ok().unwrap().body, Body::Html(_)));
    }

    #[test]
    fn several_formats_at_once_stay_together() {
        let json = r#"[{"url":"https://example.com/a","status":200,
            "content":{"raw":"<p>a</p>","markdown":"a"}}]"#;
        let pages = reply(200, json)
            .read(&Url::parse("https://example.com/a").unwrap(), None)
            .unwrap();
        assert!(matches!(pages.first_ok().unwrap().body, Body::Multi(_)));
    }

    #[test]
    fn a_page_with_no_content_reads_as_blank_rather_than_failing() {
        let pages = reply(200, r#"[{"url":"https://example.com/a","status":200}]"#)
            .read(&Url::parse("https://example.com/a").unwrap(), None)
            .unwrap();
        assert!(pages.first_ok().expect("a page").is_blank());
    }

    #[test]
    fn a_single_object_and_an_array_read_the_same() {
        let url = Url::parse("https://example.com/a").unwrap();
        let one = reply(200, r#"{"url":"https://example.com/a","status":200}"#)
            .read(&url, None)
            .unwrap();
        let many = reply(200, r#"[{"url":"https://example.com/a","status":200}]"#)
            .read(&url, None)
            .unwrap();
        assert_eq!(one.len(), many.len());
    }

    #[test]
    fn links_are_resolved_against_the_page() {
        let json = r#"[{"url":"https://example.com/a/b","status":200,
            "links":["/c","https://example.org/d","http://"]}]"#;
        let pages = reply(200, json)
            .read(&Url::parse("https://example.com/a/b").unwrap(), None)
            .unwrap();
        let links = pages.first_ok().unwrap().links.clone().expect("links");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].as_str(), "https://example.com/c");
    }

    #[test]
    fn rate_limit_headers_land_in_the_snapshot() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("ratelimit-limit", "100".parse().unwrap());
        headers.insert("ratelimit-remaining", "0".parse().unwrap());
        headers.insert("ratelimit-reset", "30".parse().unwrap());
        headers.insert("retry-after", "12".parse().unwrap());

        let transport = Transport::with_client(
            "not-a-real-key",
            Url::parse(DEFAULT_BASE_URL).unwrap(),
            reqwest::Client::new(),
        );
        transport.record_limits(&headers);

        let snapshot = transport.rate_limit();
        assert_eq!(snapshot.limit, Some(100));
        assert_eq!(snapshot.remaining, Some(0));
        assert_eq!(snapshot.reset, Some(Duration::from_secs(30)));
        assert_eq!(snapshot.retry_after, Some(Duration::from_secs(12)));
        assert_eq!(snapshot.wait(), Some(Duration::from_secs(30)));
        assert!(snapshot.is_known());
    }

    /// A count the field cannot hold saturates. Cast, 2^32 wrapped to zero and
    /// the snapshot said no calls were left when the service had said the
    /// opposite.
    #[test]
    fn a_rate_limit_past_the_field_saturates_rather_than_wrapping_to_zero() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("ratelimit-limit", "4294967296".parse().unwrap());
        headers.insert("ratelimit-remaining", "4294967296".parse().unwrap());

        let transport = Transport::with_client(
            "not-a-real-key",
            Url::parse(DEFAULT_BASE_URL).unwrap(),
            reqwest::Client::new(),
        );
        transport.record_limits(&headers);

        let snapshot = transport.rate_limit();
        assert_eq!(snapshot.limit, Some(u32::MAX));
        assert_eq!(snapshot.remaining, Some(u32::MAX));
    }

    #[test]
    fn a_missing_rate_limit_header_reads_as_unknown_not_as_zero() {
        let transport = Transport::with_client(
            "not-a-real-key",
            Url::parse(DEFAULT_BASE_URL).unwrap(),
            reqwest::Client::new(),
        );
        assert!(!transport.rate_limit().is_known());
        assert_eq!(transport.rate_limit().remaining, None);
        assert_eq!(transport.rate_limit().wait(), None);
    }

    #[test]
    fn the_transport_never_prints_the_key() {
        let transport = Transport::with_client(
            "sk-a-secret-value",
            Url::parse(DEFAULT_BASE_URL).unwrap(),
            reqwest::Client::new(),
        );
        let printed = format!("{transport:?}");
        assert!(!printed.contains("sk-a-secret-value"), "{printed}");
        assert!(printed.contains(REDACTED));
    }

    #[test]
    fn an_address_is_built_from_the_route_and_the_base() {
        let transport = Transport::with_client(
            "not-a-real-key",
            Url::parse("https://api.spider.cloud").unwrap(),
            reqwest::Client::new(),
        );
        assert_eq!(
            transport.url_for(route::SCRAPE, &[], &[]).unwrap().as_str(),
            "https://api.spider.cloud/scrape"
        );
        assert_eq!(
            transport
                .url_for(route::DATA_TABLE, &["crawl_logs"], &[("limit", "5")])
                .unwrap()
                .as_str(),
            "https://api.spider.cloud/data/crawl_logs?limit=5"
        );
    }

    /// A base with a path and no trailing slash kept the host and lost the
    /// path: `Url::join` treats the last segment as a file to replace, so
    /// `https://proxy.example/spider` plus `scrape` called
    /// `https://proxy.example/scrape`, an address nobody named.
    #[test]
    fn a_base_with_a_path_keeps_its_path_whether_or_not_it_ends_in_a_slash() {
        for base in [
            "https://proxy.example/spider",
            "https://proxy.example/spider/",
        ] {
            let transport = Transport::with_client(
                "not-a-real-key",
                Url::parse(base).unwrap(),
                reqwest::Client::new(),
            );
            assert_eq!(
                transport.url_for(route::SCRAPE, &[], &[]).unwrap().as_str(),
                "https://proxy.example/spider/scrape",
                "from base {base}"
            );
            assert_eq!(
                transport
                    .url_for(route::FETCH, &["example.com", "a/b"], &[])
                    .unwrap()
                    .as_str(),
                "https://proxy.example/spider/fetch/example.com/a/b",
                "from base {base}"
            );
        }
    }

    #[tokio::test]
    async fn a_route_refuses_the_wrong_verb() {
        let transport = Transport::with_client(
            "not-a-real-key",
            Url::parse(DEFAULT_BASE_URL).unwrap(),
            reqwest::Client::new(),
        );
        // No request leaves: the verb is checked before an address is built.
        assert!(matches!(
            transport.get(route::SCRAPE, &[], &[]).await,
            Err(Error::Config(_))
        ));
        assert!(matches!(
            transport.post(route::DATA_CREDITS, &[], &()).await,
            Err(Error::Config(_))
        ));
    }
}
