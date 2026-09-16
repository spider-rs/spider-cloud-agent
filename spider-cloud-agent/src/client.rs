//! The way in.
//!
//! [`Spider`] holds the key, the address, the budget and the policy, and hands
//! out one builder per endpoint. Every builder ends in `send`, and most also
//! offer `send_all`, which keeps the failures instead of dropping them.
//!
//! ```no_run
//! # async fn run() -> spider_cloud_agent::Result<()> {
//! use spider_cloud_agent::{ProxyPool, RequestMode, Spider};
//!
//! let spider = Spider::new()?;
//! let page = spider
//!     .scrape("https://example.com")
//!     .mode(RequestMode::Browser)
//!     .proxy(ProxyPool::Residential)
//!     .send()
//!     .await?;
//! println!("{} bytes", page.body.len());
//! # Ok(())
//! # }
//! ```
//!
//! The key is never printed. `Debug` on [`Spider`], [`SpiderBuilder`] and
//! [`Transport`] is written by hand and puts a placeholder where the key would
//! be. The operation builders derive `Debug` and reach the key only through
//! those impls, and no error the crate returns copies anything from the request
//! into its message.

use std::fmt;
use std::sync::Arc;

use url::Url;

use spider_route::{HeuristicRouter, Router};

use crate::auth::Credentials;
use crate::credits::Credits;
use crate::error::Error;
use crate::memory::SiteMemoryStore;
use crate::ops::crawl::Crawl;
use crate::ops::data::{CrawlLogs, Table};
use crate::ops::fetch::Fetch;
use crate::ops::links::Links;
use crate::ops::scrape::Scrape;
use crate::ops::screenshot::Screenshot;
use crate::ops::search::Search;
use crate::ops::transform::{Document, Transform};
use crate::policy::{Budget, Policy};
use crate::record::Recorder;
use crate::routing::Explorer;
use crate::Result;

pub use crate::transport::{
    route, Method, RateLimit, Reply, Route, Transport, BASE_URL_ENV, DEFAULT_BASE_URL, ROUTES,
    USER_AGENT,
};

pub use crate::auth::{API_KEY_ENV, API_KEY_ENV_ALT, CREDENTIALS_PATH};

/// Shown in place of the key wherever a value is printed.
const REDACTED: &str = "<redacted>";

/// Anything that can name a page.
///
/// A parse failure is held until `send`, so a builder chain reads the same
/// whether the address came from a literal or from user input.
pub trait IntoUrl {
    /// The address, or why it is not one.
    fn into_url(self) -> std::result::Result<Url, url::ParseError>;
}

impl IntoUrl for Url {
    fn into_url(self) -> std::result::Result<Url, url::ParseError> {
        Ok(self)
    }
}

impl IntoUrl for &Url {
    fn into_url(self) -> std::result::Result<Url, url::ParseError> {
        Ok(self.clone())
    }
}

impl IntoUrl for &str {
    fn into_url(self) -> std::result::Result<Url, url::ParseError> {
        Url::parse(self)
    }
}

impl IntoUrl for String {
    fn into_url(self) -> std::result::Result<Url, url::ParseError> {
        Url::parse(&self)
    }
}

impl IntoUrl for &String {
    fn into_url(self) -> std::result::Result<Url, url::ParseError> {
        Url::parse(self)
    }
}

/// A client for the Spider Cloud API.
///
/// Cheap to clone: the HTTP client, the key and the rate limit snapshot are
/// shared, so one `Spider` per process is the normal shape and a clone per task
/// costs an atomic bump.
#[derive(Clone)]
pub struct Spider {
    transport: Arc<Transport>,
    budget: Budget,
    policy: Option<Policy>,
    router: Arc<dyn Router>,
    recorder: Option<Arc<dyn Recorder>>,
    explorer: Explorer,
    memory: Arc<SiteMemoryStore>,
}

impl fmt::Debug for Spider {
    /// Prints the address and the budget. The key is replaced with a
    /// placeholder, which is the whole reason this is written out rather than
    /// derived.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spider")
            .field("base", &self.transport.base_url().as_str())
            .field("key", &REDACTED)
            .field("budget", &self.budget)
            .field("policy", &PolicyPresence(self.policy.is_some()))
            .field("router", &self.router.version())
            .field("recorder", &PolicyPresence(self.recorder.is_some()))
            .field("explore", &self.explorer.rate)
            .finish()
    }
}

/// Says whether a policy was set without printing it, so `Debug` on a client
/// stays one line.
struct PolicyPresence(bool);

impl fmt::Debug for PolicyPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0 { "set" } else { "default" })
    }
}

impl Spider {
    /// A client using the first key it can find.
    ///
    /// In order: `SPIDER_API_KEY`, then `SPIDER_CLOUD_API_KEY`, then the file
    /// the command line tool writes at `~/.spider/credentials`, which holds the
    /// key on a single line and nothing else.
    ///
    /// Returns [`Error::Auth`] when none of them has one. The message names the
    /// places that were tried and never the values that were found.
    pub fn new() -> Result<Spider> {
        let key = resolve_key().ok_or_else(no_key)?;
        SpiderBuilder::new().key(key).build()
    }

    /// A client for a key you already hold.
    ///
    /// A key that turns out to be wrong reports itself on the first call, as
    /// [`Error::Auth`]. This fails now only if `SPIDER_API_URL` is set to
    /// something that is not an address, which is worth saying out loud rather
    /// than quietly sending the traffic somewhere else.
    pub fn with_key(key: impl Into<String>) -> Result<Spider> {
        let base = Transport::base_url_from_env()?;
        // Building an http client fails only when the TLS stack will not start.
        // That failure is reported rather than papered over: the fallback this
        // used to take was `reqwest::Client::default()`, which builds the same
        // client and panics when it cannot, so the one case it was written for
        // was the one case it took the caller's process down in.
        let transport = Transport::with_base(key, base)?;
        Ok(Spider {
            transport: Arc::new(transport),
            budget: Budget::default(),
            policy: None,
            router: Arc::new(HeuristicRouter::new()),
            recorder: None,
            explorer: Explorer::default(),
            memory: Arc::new(SiteMemoryStore::default()),
        })
    }

    /// Set the address, the budget, the policy or the HTTP client before
    /// building.
    pub fn builder() -> SpiderBuilder {
        SpiderBuilder::new()
    }

    /// Read one page.
    ///
    /// The operation nearly every run is made of. Not to be confused with
    /// [`Spider::fetch`], which is a different endpoint.
    pub fn scrape(&self, url: impl IntoUrl) -> Scrape<'_> {
        Scrape::new(self, url)
    }

    /// Read one path on one host, under the config the service holds for it.
    ///
    /// Takes the host and the path rather than an address, because that is
    /// what the endpoint takes, and it drops the query string. Use
    /// [`Spider::scrape`] unless you want the stored configuration: this one
    /// works the extraction out for a path it has not seen before, which is
    /// slow the first time and can fail while it tries.
    pub fn fetch(&self, domain: impl Into<String>, path: impl Into<String>) -> Fetch<'_> {
        Fetch::new(self, domain, path)
    }

    /// Fetch a site, following its links.
    pub fn crawl(&self, url: impl IntoUrl) -> Crawl<'_> {
        Crawl::new(self, url)
    }

    /// Collect the links on a page without paying for its content.
    pub fn links(&self, url: impl IntoUrl) -> Links<'_> {
        Links::new(self, url)
    }

    /// Run a query.
    pub fn search(&self, query: impl Into<String>) -> Search<'_> {
        Search::new(self, query)
    }

    /// Take a picture of a page.
    pub fn screenshot(&self, url: impl IntoUrl) -> Screenshot<'_> {
        Screenshot::new(self, url)
    }

    /// Convert markup you already hold, without fetching anything.
    pub fn transform(&self, docs: Vec<Document>) -> Transform<'_> {
        Transform::new(self, docs)
    }

    /// What is left on the account.
    pub async fn credits(&self) -> Result<Credits> {
        let reply = crate::ops::data::read_under_wall(
            self,
            self.transport.get(route::DATA_CREDITS, &[], &[]),
        )
        .await?;
        crate::ops::data::credits_from(&reply)
    }

    /// The record of past crawls.
    pub fn crawl_logs(&self) -> CrawlLogs<'_> {
        CrawlLogs::new(self)
    }

    /// One stored table by name, as the escape hatch for `/data`.
    ///
    /// `credits` and `crawl_logs` are the two reads this crate names. This is
    /// the rest of `/data`, for a table a later version of the service adds and
    /// this one has no builder for. It handles paging and unwraps the reply into
    /// rows, which is the only part worth not writing again.
    ///
    /// What the caller owns: the name, which the service either knows or
    /// refuses; the columns, which belong to the service and change without a
    /// release of this crate, so rows arrive as [`serde_json::Value`]; and
    /// whether reading that table is a thing the program should be doing at all.
    /// A name is a constant at a call site, not a string that arrived from
    /// outside the program. `spider-agent` had a `table` subcommand that took
    /// one from the command line, and 0.3.0 removed it for that reason.
    ///
    /// A name carrying a `.` or `..` segment is refused by [`Route::render`]
    /// before a request is built, because [`Url::join`] would resolve it and the
    /// call would leave `/data` altogether.
    ///
    /// [`Spider::raw`] is the wider escape hatch, for anything that is not a
    /// `/data` read.
    ///
    /// [`Route::render`]: crate::client::Route::render
    /// [`Url::join`]: url::Url::join
    pub fn table(&self, name: impl Into<String>) -> Table<'_> {
        Table::new(self, name)
    }

    /// The HTTP layer, for anything the curated surface does not cover.
    ///
    /// Typed parameters, typed replies, and nothing between you and the service:
    /// no escalation, no budget, no trimming. Use it for an endpoint this
    /// version has no builder for, and accept that the outcome is then yours.
    pub fn raw(&self) -> &Transport {
        &self.transport
    }

    /// What the service last said about the rate limit.
    pub fn rate_limit(&self) -> RateLimit {
        self.transport.rate_limit()
    }

    /// The caps every operation starts with.
    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// The policy operations escalate under, when one was set.
    pub fn policy(&self) -> Option<&Policy> {
        self.policy.as_ref()
    }

    /// What picks the settings for a first attempt.
    ///
    /// The shipped rules unless a caller named their own. Every operation
    /// consults this before it sends anything.
    pub fn router(&self) -> &dyn Router {
        self.router.as_ref()
    }

    /// Where routing rows are written, when anywhere.
    pub fn recorder(&self) -> Option<&dyn Recorder> {
        self.recorder.as_deref()
    }

    /// How often a call takes a different action than the router picked.
    pub fn explorer(&self) -> Explorer {
        self.explorer
    }

    /// What this client remembers about the sites it has fetched.
    ///
    /// Shared by every clone of the client and by every operation it runs, and
    /// bounded: see [`crate::memory`].
    pub fn site_memory(&self) -> &SiteMemoryStore {
        &self.memory
    }
}

/// Settings for a client, applied at [`SpiderBuilder::build`].
///
/// `Debug` redacts the key.
#[derive(Default)]
pub struct SpiderBuilder {
    key: Option<String>,
    base_url: Option<Url>,
    budget: Option<Budget>,
    policy: Option<Policy>,
    http_client: Option<reqwest::Client>,
    router: Option<Arc<dyn Router>>,
    recorder: Option<Arc<dyn Recorder>>,
    explorer: Explorer,
    memory_capacity: Option<usize>,
}

impl fmt::Debug for SpiderBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpiderBuilder")
            .field("key", &self.key.as_ref().map(|_| REDACTED))
            .field("base_url", &self.base_url.as_ref().map(Url::as_str))
            .field("budget", &self.budget)
            .field("policy", &PolicyPresence(self.policy.is_some()))
            .field("http_client", &self.http_client.as_ref().map(|_| "set"))
            .field("router", &PolicyPresence(self.router.is_some()))
            .field("recorder", &PolicyPresence(self.recorder.is_some()))
            .field("explore", &self.explorer.rate)
            .finish()
    }
}

impl SpiderBuilder {
    /// A builder with nothing set.
    pub fn new() -> SpiderBuilder {
        SpiderBuilder::default()
    }

    /// The key to send. Without this, [`SpiderBuilder::build`] looks in the
    /// environment and then in `~/.spider/credentials`.
    pub fn key(mut self, key: impl Into<String>) -> SpiderBuilder {
        self.key = Some(key.into());
        self
    }

    /// The address to call. Overrides `SPIDER_API_URL`.
    pub fn base_url(mut self, base_url: Url) -> SpiderBuilder {
        self.base_url = Some(base_url);
        self
    }

    /// The caps every operation starts with.
    pub fn budget(mut self, budget: Budget) -> SpiderBuilder {
        self.budget = Some(budget);
        self
    }

    /// The rules operations escalate under.
    pub fn policy(mut self, policy: Policy) -> SpiderBuilder {
        self.policy = Some(policy);
        self
    }

    /// An HTTP client of your own, so connection pools and timeouts stay yours.
    pub fn http_client(mut self, client: reqwest::Client) -> SpiderBuilder {
        self.http_client = Some(client);
        self
    }

    /// What picks the settings for a first attempt.
    ///
    /// The shipped rules unless you say otherwise. A router of your own is one
    /// implementation of the same trait, so nothing else changes.
    ///
    /// Whatever it answers, a setting you made yourself wins. The router fills
    /// in what you left alone and nothing else.
    pub fn router(mut self, router: impl Router + 'static) -> SpiderBuilder {
        self.router = Some(Arc::new(router));
        self
    }

    /// Where to write a row for every routed attempt.
    ///
    /// Nothing is written unless you ask for it. A row holds the feature
    /// vector, the settings chosen and how the attempt ended, and it cannot
    /// hold an address: see [`crate::record`].
    ///
    /// ```no_run
    /// # fn run() -> std::io::Result<()> {
    /// use spider_cloud_agent::record::JsonlRecorder;
    /// use spider_cloud_agent::Spider;
    ///
    /// let spider = Spider::builder()
    ///     .recorder(JsonlRecorder::create("routes.jsonl")?)
    ///     .explore(0.05)
    ///     .build();
    /// # let _ = spider;
    /// # Ok(())
    /// # }
    /// ```
    pub fn recorder(mut self, recorder: impl Recorder + 'static) -> SpiderBuilder {
        self.recorder = Some(Arc::new(recorder));
        self
    }

    /// The fraction of pages that take a different action than the router
    /// picked, from zero to one. Zero by default, which explores nothing.
    ///
    /// Logged history only ever shows the action that was taken, so it can
    /// never say whether another one would have worked. A small fraction of
    /// deliberately different calls is the only way to find out, and the rows
    /// a recorder writes for them are the only unbiased ones.
    ///
    /// Two things it will not do. It never fires on a retry, only on the first
    /// attempt of an operation, so an escalation is never interfered with. And
    /// it never reaches for an action that costs more than the budget allows.
    ///
    /// The choice depends on the address, so the same page explores the same
    /// way on every call rather than flapping between settings.
    pub fn explore(mut self, rate: f32) -> SpiderBuilder {
        self.explorer.rate = rate.clamp(0.0, 1.0);
        self
    }

    /// The seed exploration draws from.
    ///
    /// Fixed by default, so two processes with the same settings explore the
    /// same pages. Change it to explore a different slice of the same traffic.
    pub fn explore_seed(mut self, seed: u64) -> SpiderBuilder {
        self.explorer.seed = seed;
        self
    }

    /// How many sites this client remembers at once.
    ///
    /// Zero disables site memory without allocating a table.
    /// Rounded up to a power of two and held inside the limits in
    /// [`crate::memory`]. The store is bounded whatever you pass.
    pub fn site_memory_capacity(mut self, sites: usize) -> SpiderBuilder {
        self.memory_capacity = Some(sites);
        self
    }

    /// Build the client.
    ///
    /// Fails when no key can be found, and when the address is not a URL.
    pub fn build(self) -> Result<Spider> {
        let key = match self.key {
            Some(key) => key,
            None => resolve_key().ok_or_else(no_key)?,
        };
        if key.trim().is_empty() {
            return Err(Error::Auth("the api key is empty".to_string()));
        }
        let base = match self.base_url {
            Some(base) => base,
            None => Transport::base_url_from_env()?,
        };
        let transport = match self.http_client {
            Some(client) => Transport::with_client(key, base, client),
            None => Transport::with_base(key, base)?,
        };
        let memory = match self.memory_capacity {
            Some(capacity) => SiteMemoryStore::new(capacity),
            None => SiteMemoryStore::default(),
        };

        Ok(Spider {
            transport: Arc::new(transport),
            budget: self.budget.unwrap_or_default(),
            policy: self.policy,
            router: self
                .router
                .unwrap_or_else(|| Arc::new(HeuristicRouter::new())),
            recorder: self.recorder,
            explorer: self.explorer,
            memory: Arc::new(memory),
        })
    }
}

/// The key, from wherever this machine keeps one. See [`crate::auth::store`]
/// for the order.
///
/// Nothing here logs what it found, and the caller only learns whether there was
/// one.
fn resolve_key() -> Option<String> {
    Credentials::resolve().map(Credentials::into_key)
}

/// Why a client could not be built, naming the places that were tried and none
/// of the values that were found.
fn no_key() -> Error {
    Error::Auth(format!(
        "no api key. Set {API_KEY_ENV}, set {API_KEY_ENV_ALT}, or sign in so the key is written to ~/{CREDENTIALS_PATH}"
    ))
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

    const SECRET: &str = "sk-live-not-a-real-key-0123456789";

    #[test]
    fn a_client_never_prints_its_key() {
        let spider = Spider::with_key(SECRET).expect("a client");
        let printed = format!("{spider:?}");
        assert!(!printed.contains(SECRET), "{printed}");
        assert!(printed.contains(REDACTED));

        let raw = format!("{:?}", spider.raw());
        assert!(!raw.contains(SECRET), "{raw}");
    }

    #[test]
    fn a_builder_never_prints_its_key() {
        let builder = SpiderBuilder::new().key(SECRET);
        let printed = format!("{builder:?}");
        assert!(!printed.contains(SECRET), "{printed}");
        assert!(printed.contains(REDACTED));
    }

    #[test]
    fn an_empty_key_is_refused_before_a_call_is_made() {
        let built = SpiderBuilder::new().key("   ").build();
        assert!(matches!(built, Err(Error::Auth(_))));
    }

    #[test]
    fn an_address_that_is_not_a_url_fails_the_build() {
        let built = SpiderBuilder::new()
            .key(SECRET)
            .base_url(Url::parse("https://api.spider.cloud").expect("a url"));
        assert!(built.build().is_ok());
    }

    #[test]
    fn a_missing_key_names_the_places_it_looked_and_no_values() {
        let message = no_key().to_string();
        assert!(message.contains("SPIDER_API_KEY"));
        assert!(message.contains(".spider/credentials"));
    }

    #[test]
    fn an_address_can_be_named_on_the_builder() {
        let spider = SpiderBuilder::new()
            .key(SECRET)
            .base_url(Url::parse("https://spider.cloud/api/").expect("a url"))
            .build()
            .expect("a client");
        assert_eq!(
            spider.raw().base_url().as_str(),
            "https://spider.cloud/api/"
        );
    }

    #[test]
    fn a_budget_set_on_the_builder_reaches_the_client() {
        let spider = SpiderBuilder::new()
            .key(SECRET)
            .budget(Budget::default().with_attempts(2))
            .build()
            .expect("a client");
        assert_eq!(spider.budget().attempts, 2);
    }

    /// Every error the crate can hand back, built the way the crate builds them.
    ///
    /// The point is not that these strings happen to be clean. It is that the
    /// only way the key could reach one is if some code path copied it in, and
    /// this walks every path that makes an error.
    fn every_error(spider: &Spider) -> Vec<Error> {
        use crate::error::BudgetKind;
        use crate::status::ApiStatus;
        use crate::transport::{RateLimit, Reply};
        use bytes::Bytes;
        use std::time::Duration;

        let reply = |code: u16, body: &str| Reply {
            status: ApiStatus::new(code),
            rate_limit: RateLimit::default(),
            retry_after: Some(Duration::from_secs(3)),
            elapsed: Duration::from_millis(12),
            content_type: Some("application/json".into()),
            body: Bytes::from(body.to_string()),
        };

        let mut out = Vec::new();
        // The call plane, in every shape the transport sorts it into.
        out.extend(reply(500, r#"{"error":"something gave way"}"#).as_error());
        out.extend(reply(429, r#"{"error":"slow down"}"#).as_error());
        out.extend(reply(401, r#"{"error":"invalid api key"}"#).as_error());
        out.extend(reply(402, r#"{"error":"balance is zero"}"#).as_error());
        // A body that is not what this version expects.
        out.push(
            reply(200, "not json")
                .json::<serde_json::Value>()
                .expect_err("a decode error"),
        );
        // An address that is not one, which is caught before a call is made.
        out.push(
            futures_now(spider.scrape("not a url").send_all()).expect_err("a configuration error"),
        );
        // The budget and the attempt trail.
        out.push(Error::BudgetExceeded {
            kind: BudgetKind::Credits,
            attempts: Vec::new(),
        });
        out.push(Error::Exhausted {
            attempts: Vec::new(),
            last: None,
        });
        out.push(Error::InsufficientCredits);
        out
    }

    /// Run a future that is known to finish without waiting on anything.
    ///
    /// The error paths above are all reached before a request leaves, so none of
    /// them needs a runtime.
    fn futures_now<T>(future: impl std::future::Future<Output = T>) -> T {
        use std::task::{Context, Poll, Waker};

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = Box::pin(future);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("this path was expected to finish before a call was made"),
        }
    }

    #[test]
    fn no_error_the_crate_makes_carries_the_key() {
        let spider = Spider::with_key(SECRET).expect("a client");
        let errors = every_error(&spider);
        // A loop over an empty list passes and proves nothing.
        assert_eq!(errors.len(), 9, "an error path stopped being covered");
        for error in errors {
            let debug = format!("{error:?}");
            let display = format!("{error}");
            assert!(!debug.contains(SECRET), "key in Debug: {debug}");
            assert!(!display.contains(SECRET), "key in Display: {display}");
            assert!(!debug.contains("Bearer"), "authorization header in {debug}");
        }
    }

    /// A builder prints the client it borrows, so it is another way the key
    /// could reach a log. Same rule as the errors above, asserted the same way.
    #[test]
    fn no_builder_prints_the_key() {
        let spider = Spider::with_key(SECRET).expect("a client");
        let printed = [
            format!("{:?}", spider.scrape("https://example.com")),
            format!("{:?}", spider.fetch("example.com", "/")),
            format!("{:?}", spider.crawl("https://example.com")),
            format!("{:?}", spider.links("https://example.com")),
            format!("{:?}", spider.search("a query")),
            format!("{:?}", spider.screenshot("https://example.com")),
            format!("{:?}", spider.transform(Vec::new())),
            format!("{:?}", spider.crawl_logs()),
            format!("{:?}", spider.table("pages")),
        ];
        for one in printed {
            assert!(!one.contains(SECRET), "key in Debug: {one}");
            assert!(!one.contains("Bearer"), "authorization header in {one}");
        }
    }

    #[test]
    fn no_builder_prints_caller_credentials() {
        let spider = Spider::with_key(SECRET).expect("a client");
        let cookie = "session=synthetic-cookie-value";
        let authorization = "Bearer synthetic-header-value";
        let password = "synthetic-proxy-password";
        let proxy = format!("http://user:{password}@example.com:8080");
        macro_rules! check {
            ($builder:expr) => {{
                let mut builder = $builder;
                let params = builder.params_mut();
                params.cookies = Some(cookie.into());
                params.headers = Some(
                    [
                        ("Authorization".into(), authorization.into()),
                        ("X-Custom".into(), password.into()),
                    ]
                    .into(),
                );
                params.remote_proxy = Some(proxy.clone());
                let wire = serde_json::to_value(&*params).expect("serialize");
                assert!(wire["cookies"] == cookie);
                assert!(wire["headers"]["Authorization"] == authorization);
                assert!(wire["remote_proxy"] == proxy);
                let direct = format!("{params:?}");
                for printed in [direct, format!("{builder:?}"), format!("{builder:#?}")] {
                    for secret in [SECRET, cookie, authorization, password] {
                        assert!(!printed.contains(secret), "caller credential in Debug");
                    }
                    assert!(printed.contains("Authorization"));
                    assert!(printed.contains("X-Custom"));
                    assert!(printed.contains("<redacted>"));
                }
            }};
        }
        check!(spider.scrape("https://example.com"));
        check!(spider.crawl("https://example.com"));
        check!(spider.fetch("example.com", "/"));
        check!(spider.links("https://example.com"));
        check!(spider.search("a query"));
        check!(spider.screenshot("https://example.com"));
        check!(spider.transform(Vec::new()));
    }

    #[test]
    fn builder_debug_redacts_copies_of_url_and_document_credentials() {
        let spider = Spider::with_key(SECRET).expect("a client");
        let secret = "synthetic-url-password";
        let url = format!("https://user:{secret}@example.com/?token={secret}");
        let document = crate::ops::transform::Document::html(secret).from_url(&url);
        for printed in [
            format!("{:?}", spider.scrape(&url)),
            format!(
                "{:?}",
                spider.fetch(format!("user:{secret}@example.com"), secret)
            ),
            format!("{:?}", spider.search(&url)),
            format!("{document:?}"),
            format!("{:?}", spider.transform(vec![document])),
        ] {
            assert!(!printed.contains(secret), "credential in builder Debug");
        }
    }

    #[tokio::test]
    async fn a_failed_call_reports_nothing_about_the_key() {
        // A documentation address, which routes nowhere, with a timeout short
        // enough that the attempt trail finishes quickly.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(40))
            .timeout(std::time::Duration::from_millis(80))
            .build()
            .expect("a client");
        let spider = SpiderBuilder::new()
            .key(SECRET)
            .base_url(Url::parse("http://192.0.2.1:9/").expect("a url"))
            .budget(Budget::default().with_attempts(1))
            .http_client(client)
            .build()
            .expect("a client");

        let error = spider
            .scrape("https://example.com")
            .send()
            .await
            .expect_err("nothing answers at that address");
        let printed = format!("{error:?} {error}");
        assert!(!printed.contains(SECRET), "{printed}");
    }

    #[test]
    fn addresses_come_from_several_shapes() {
        let url = Url::parse("https://example.com/a").expect("a url");
        assert!("https://example.com/a".into_url().is_ok());
        assert!(String::from("https://example.com/a").into_url().is_ok());
        assert!((&url).into_url().is_ok());
        assert!(url.into_url().is_ok());
        assert!("not a url".into_url().is_err());
    }
}
