//! The request body sent to the API.
//!
//! [`RequestParams`] is the whole documented parameter set, and every field on
//! it is optional. A default one serializes to `{}`, so the service applies its
//! own defaults and you pay for nothing you did not ask for.
//!
//! Most callers never touch this module. The client's own methods cover the
//! handful of settings worth choosing by hand, and this is what they hand back
//! when you want the rest.
//!
//! ```
//! use spider_cloud_agent::params::{Country, ProxyPool, RequestMode, RequestParams};
//!
//! let mut params = RequestParams::default();
//! params.url = Some("https://example.com".into());
//! params.request = Some(RequestMode::Browser);
//! params.proxy = Some(ProxyPool::Residential);
//! params.country_code = Country::new("de");
//! ```

pub mod extraction;
pub mod js;
pub mod scope;
pub mod screenshot;
pub mod search;
pub mod transport;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use crate::credits::{Credits, WholeCredits, CREDITS_PER_USD};
pub use extraction::{
    ChunkingAlg, ChunkingKind, CssExtractionMap, ExtractSelector, ReturnFormat,
    ReturnFormatHandling, SelectorGroup,
};
pub use js::{
    Delay, EventTracker, ExecutionScriptsMap, IdleNetwork, SelectorWait, Timeout, WaitFor,
    WebAutomation, WebAutomationMap,
};
pub use scope::{CrawlBudget, LinkRewriteRule};
pub use screenshot::ScreenshotParams;
pub use search::{SearchEngine, SearchParams, TimeWindow};
pub use transport::{
    Country, InvalidCountry, Profile, ProxyPool, RedirectPolicy, RequestMode, Viewport,
    WebhookSettings, BOT_USER_AGENT, PROXY_POOLS,
};

/// Headers to send with the request.
pub type HeaderMap = BTreeMap<String, String>;

/// Everything the API accepts on a request body.
///
/// Fields are grouped below roughly the way you would reach for them: what to
/// fetch, how to fetch it, how far to go, what to run on the page, what to send
/// back, and what you are willing to spend.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RequestParams {
    /// The address to fetch. Required by every endpoint that reads a page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Whether the page is rendered before it is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestMode>,
    /// Which pool the request leaves from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyPool>,
    /// The country to appear to be in, which changes prices, stock and
    /// language on plenty of sites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<Country>,
    /// Your own static address to send file downloads through, at roughly half
    /// the usual cost. You supply the endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_proxy: Option<String>,
    /// Apply the measures that get past bot checks. What they are is decided
    /// by the service and changes as sites change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stealth: Option<bool>,
    /// Vary the traits a site reads to recognise a returning visitor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<bool>,
    /// The user agent to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// The window the page is laid out in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    /// The locale to present, such as `en-GB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Cookies to send, in one header-style string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    /// Extra headers to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderMap>,
    /// The character encoding to read the response as, when the site declares
    /// it wrongly or not at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// Keep cookies and headers across the requests you make to one site, the
    /// way a signed-in visitor would. Off by default, because a shared session
    /// makes otherwise independent requests affect each other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<bool>,
    /// Whether redirects may leave the host that was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_policy: Option<RedirectPolicy>,
    /// How long one page has to come back, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout: Option<u8>,
    /// Leave service workers running, for sites that load their content through
    /// one. On unless you turn it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_worker_enabled: Option<bool>,
    /// Wait this many milliseconds between pages, up to a minute. Setting it
    /// turns concurrency off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<u64>,
    /// The most pages fetched at once, for a site that slows down under load.
    /// Unlimited when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency_limit: Option<u32>,
    /// Fall back to the latest archived copy of a page when every other attempt
    /// has failed. Archive markup is stripped from what comes back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wayback: Option<bool>,

    /// How many pages a crawl may visit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// How many links deep from the starting page a crawl may go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Per-path caps on top of [`RequestParams::limit`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<CrawlBudget>,
    /// Paths to skip, as plain strings or patterns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blacklist: Option<Vec<String>>,
    /// Paths to visit and nothing else. Combines with the blacklist, which
    /// wins where they overlap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whitelist: Option<Vec<String>>,
    /// Follow links into subdomains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomains: Option<bool>,
    /// Treat every domain ending as the same site, so a `.co.uk` twin of the
    /// starting host is crawled too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tld: Option<bool>,
    /// Other domains a crawl may follow links into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_domains: Option<Vec<String>>,
    /// Read the site's sitemap for links as well as following them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sitemap: Option<bool>,
    /// Crawl the sitemap's links and nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sitemap_only: Option<bool>,
    /// Where the sitemap lives, when it is not `sitemap.xml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sitemap_path: Option<String>,
    /// Obey the site's robots file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respect_robots: Option<bool>,
    /// Change links as they are found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_rewrite: Option<LinkRewriteRule>,
    /// How long the whole crawl may run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crawl_timeout: Option<Timeout>,
    /// Start the crawl and answer straight away, rather than holding the
    /// connection open until it finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_in_background: Option<bool>,

    /// What has to happen on the page before it is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for: Option<WaitFor>,
    /// Keep scrolling this many times to pull in content that loads as you go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll: Option<u32>,
    /// Scripts to run once the page has loaded, keyed by path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_scripts: Option<ExecutionScriptsMap>,
    /// Steps to carry out on the page, keyed by path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_scripts: Option<WebAutomationMap>,
    /// A script to install in every frame before any of the page's own scripts
    /// run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate_on_new_document: Option<String>,
    /// Let the page fetch everything it asks for. Slower and dearer, and what
    /// fixes a page whose content arrives through a third party script that
    /// would otherwise be held back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_intercept: Option<bool>,
    /// Load images, fonts and the rest of the page's assets rather than only
    /// what the content needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_resources: Option<bool>,
    /// Block advertising requests in a rendered page. On unless you turn it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_ads: Option<bool>,
    /// Block analytics requests in a rendered page. On unless you turn it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_analytics: Option<bool>,
    /// Block stylesheets in a rendered page. On unless you turn it off.
    /// First-party CSS still loads unless
    /// [`RequestParams::disable_first_party_stylesheets`] is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_stylesheets: Option<bool>,
    /// Block first-party stylesheets too when stylesheets are blocked. Can stop a
    /// single page app from rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_first_party_stylesheets: Option<bool>,
    /// Apply blocklists to first-party scripts as well, which are let through by
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_first_party_javascript: Option<bool>,
    /// Block first-party images, media and fonts too when visuals are blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_first_party_visuals: Option<bool>,
    /// Only let requests matching one of these hosts or substrings load. Wins
    /// over [`RequestParams::network_blacklist`] where both match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_whitelist: Option<Vec<String>>,
    /// Never load requests matching one of these hosts or substrings, such as a
    /// tag manager the content does not need.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_blacklist: Option<Vec<String>>,
    /// Report the page's own requests and responses alongside the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_tracker: Option<EventTracker>,

    /// The shape the content comes back in, one format or several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_format: Option<ReturnFormatHandling>,
    /// Keep only what is inside this selector, and drop the rest of the page.
    /// The cheapest trim there is, because the bytes never leave the service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_selector: Option<String>,
    /// Drop whatever matches this selector from the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_selector: Option<String>,
    /// Remove images from the returned content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_output_images: Option<bool>,
    /// Remove `svg` elements from the returned content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_output_svg: Option<bool>,
    /// Remove navigation, asides and footers from the returned content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_output_main_only: Option<bool>,
    /// The most bytes of content one page may return. Anything longer keeps its
    /// start and its end and loses the middle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<u64>,
    /// Strip navigation, adverts and the rest of the furniture, leaving the
    /// article.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readability: Option<bool>,
    /// Drop scripts, styles, comments and the attributes nothing reads, so
    /// what comes back is the markup that carries content. Cheaper than
    /// trimming the same bytes after they arrive, because they never leave the
    /// service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_html: Option<bool>,
    /// Named fields to pull out of the page, keyed by path. Set the return
    /// format to [`ReturnFormat::Empty`] alongside it and the response carries
    /// the fields and none of the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css_extraction_map: Option<CssExtractionMap>,
    /// Split the content before returning it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunking_alg: Option<ChunkingAlg>,
    /// Include the title, description and the rest of the page's declared
    /// metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<bool>,
    /// Include vectors for the title and description. Needs
    /// [`RequestParams::metadata`] as well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_embeddings: Option<bool>,
    /// Include the response headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_headers: Option<bool>,
    /// Include the response cookies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_cookies: Option<bool>,
    /// Include the links found on each page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_page_links: Option<bool>,
    /// Include the structured data the page declares, such as its JSON-LD
    /// blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_json_data: Option<bool>,
    /// Serve a stored copy when there is one, rather than fetching again.
    /// A bool switches it, and a [`CacheControl`] says how fresh the copy
    /// has to be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<Cache>,
    /// Where to send progress, and which events to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhooks: Option<WebhookSettings>,
    /// Cloud storage to write results into as they arrive, keyed by connector
    /// name, such as `s3` or `gcs`, with the service's own field names inside.
    /// It carries your storage credentials, so `Debug` never prints it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_connectors: Option<serde_json::Map<String, serde_json::Value>>,
    /// Ask for an outside scraping provider to take part in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<Router>,
    /// Settings passed through to an outside provider, keyed by provider name.
    /// The service applies them only on a route paid for with your own key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<BTreeMap<String, serde_json::Value>>,

    /// The most this request may spend, in whole credits.
    ///
    /// The service deserializes this one as a `u64` and answers a decimal with
    /// a 400, which is why it is [`WholeCredits`] and not [`Credits`]. Build it
    /// with [`WholeCredits::floor`] so the rounding goes down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_credits_allowed: Option<WholeCredits>,
    /// The most any one page may spend, in credits. Only rendered fetches can
    /// run up a bill worth capping this way.
    ///
    /// This one the service does take as a float, so fractions survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_credits_per_page: Option<Credits>,

    /// Turn off the adjustments the service makes on your behalf, such as its
    /// own blocklists, mode choice and country handling. Set this when you are
    /// measuring what your own settings do and want nothing else moving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_hints: Option<bool>,
}

// Keep free-form content out of diagnostics: URLs, scripts, selectors and text
// can contain credentials. Serialization remains the original request body.
impl std::fmt::Debug for RequestParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestParams")
            .field("url", &self.url.as_ref().map(|_| "<redacted>"))
            .field("request", &self.request)
            .field("proxy", &self.proxy)
            .field("country_code", &self.country_code)
            .field(
                "remote_proxy",
                &self.remote_proxy.as_deref().map(RedactedProxy),
            )
            .field("stealth", &self.stealth)
            .field("fingerprint", &self.fingerprint)
            .field(
                "user_agent",
                &self.user_agent.as_ref().map(|_| "<redacted>"),
            )
            .field("viewport", &self.viewport)
            .field("locale", &self.locale.as_ref().map(|_| "<redacted>"))
            .field("cookies", &self.cookies.as_ref().map(|_| "<redacted>"))
            .field("headers", &self.headers.as_ref().map(RedactedHeaders))
            .field("encoding", &self.encoding.as_ref().map(|_| "<redacted>"))
            .field("session", &self.session)
            .field("redirect_policy", &self.redirect_policy)
            .field("request_timeout", &self.request_timeout)
            .field("service_worker_enabled", &self.service_worker_enabled)
            .field("delay", &self.delay)
            .field("concurrency_limit", &self.concurrency_limit)
            .field("wayback", &self.wayback)
            .field("limit", &self.limit)
            .field("depth", &self.depth)
            .field("budget", &self.budget.as_ref().map(|_| "<redacted>"))
            .field("blacklist", &self.blacklist.as_ref().map(|_| "<redacted>"))
            .field("whitelist", &self.whitelist.as_ref().map(|_| "<redacted>"))
            .field("subdomains", &self.subdomains)
            .field("tld", &self.tld)
            .field(
                "external_domains",
                &self.external_domains.as_ref().map(|_| "<redacted>"),
            )
            .field("sitemap", &self.sitemap)
            .field("sitemap_only", &self.sitemap_only)
            .field(
                "sitemap_path",
                &self.sitemap_path.as_ref().map(|_| "<redacted>"),
            )
            .field("respect_robots", &self.respect_robots)
            .field(
                "link_rewrite",
                &self.link_rewrite.as_ref().map(|_| "<redacted>"),
            )
            .field("crawl_timeout", &self.crawl_timeout)
            .field("run_in_background", &self.run_in_background)
            .field("wait_for", &self.wait_for.as_ref().map(|_| "<redacted>"))
            .field("scroll", &self.scroll)
            .field(
                "execution_scripts",
                &self.execution_scripts.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "automation_scripts",
                &self.automation_scripts.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "evaluate_on_new_document",
                &self.evaluate_on_new_document.as_ref().map(|_| "<redacted>"),
            )
            .field("disable_intercept", &self.disable_intercept)
            .field("full_resources", &self.full_resources)
            .field("block_ads", &self.block_ads)
            .field("block_analytics", &self.block_analytics)
            .field("block_stylesheets", &self.block_stylesheets)
            .field(
                "disable_first_party_stylesheets",
                &self.disable_first_party_stylesheets,
            )
            .field(
                "disable_first_party_javascript",
                &self.disable_first_party_javascript,
            )
            .field(
                "disable_first_party_visuals",
                &self.disable_first_party_visuals,
            )
            .field(
                "network_whitelist",
                &self.network_whitelist.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "network_blacklist",
                &self.network_blacklist.as_ref().map(|_| "<redacted>"),
            )
            .field("event_tracker", &self.event_tracker)
            .field("return_format", &self.return_format)
            .field(
                "root_selector",
                &self.root_selector.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "exclude_selector",
                &self.exclude_selector.as_ref().map(|_| "<redacted>"),
            )
            .field("filter_output_images", &self.filter_output_images)
            .field("filter_output_svg", &self.filter_output_svg)
            .field("filter_output_main_only", &self.filter_output_main_only)
            .field("max_size", &self.max_size)
            .field("readability", &self.readability)
            .field("clean_html", &self.clean_html)
            .field(
                "css_extraction_map",
                &self.css_extraction_map.as_ref().map(|_| "<redacted>"),
            )
            .field("chunking_alg", &self.chunking_alg)
            .field("metadata", &self.metadata)
            .field("return_embeddings", &self.return_embeddings)
            .field("return_headers", &self.return_headers)
            .field("return_cookies", &self.return_cookies)
            .field("return_page_links", &self.return_page_links)
            .field("return_json_data", &self.return_json_data)
            .field("cache", &self.cache)
            .field("webhooks", &self.webhooks)
            .field(
                "data_connectors",
                &self.data_connectors.as_ref().map(|_| "<redacted>"),
            )
            .field("router", &self.router)
            .field(
                "provider_options",
                &self.provider_options.as_ref().map(|_| "<redacted>"),
            )
            .field("max_credits_allowed", &self.max_credits_allowed)
            .field("max_credits_per_page", &self.max_credits_per_page)
            .field("disable_hints", &self.disable_hints)
            .finish_non_exhaustive()
    }
}

struct RedactedHeaders<'a>(&'a HeaderMap);

impl std::fmt::Debug for RedactedHeaders<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|name| (name, "<redacted>")))
            .finish()
    }
}

// Only the proxy endpoint is useful in diagnostics. Paths, queries and
// fragments can carry credentials too. Invalid input is never echoed.
struct RedactedProxy<'a>(&'a str);

impl std::fmt::Debug for RedactedProxy<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match url::Url::parse(self.0) {
            Ok(url) if url.has_host() => f
                .debug_struct("Proxy")
                .field("scheme", &url.scheme())
                .field("userinfo", &"<redacted>")
                .field("host", &url.host_str())
                .field("port", &url.port())
                .finish_non_exhaustive(),
            _ => f.write_str("<redacted>"),
        }
    }
}

/// The `router` parameter: whether and how an outside scraping provider takes
/// part in a request.
///
/// Every field is optional and sent as text, because the service ignores a
/// value it does not know rather than refusing the request.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Router {
    /// `fallback` puts a provider behind the service's own fetch, `first`
    /// puts one in front of it, and `off` keeps the request away from
    /// outside providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The provider to prefer, by name. A preference, not a pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Your key for [`Router::provider`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Your provider credentials keyed by credential name, for providers that
    /// need more than one value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<BTreeMap<String, String>>,
    /// `any`, the default, or `own` to allow only routes your own keys pay
    /// for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding: Option<String>,
}

// The token and the credentials are keys. Only whether they are there, and
// how many credentials, is printed.
impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("mode", &self.mode)
            .field("provider", &self.provider)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("credentials", &self.credentials.as_ref().map(|c| c.len()))
            .field("funding", &self.funding)
            .finish()
    }
}

/// The `cache` parameter, in either of the two shapes the service accepts.
///
/// The service reads a bool or a control object from the same key. This is
/// not a curated knob: it is reached through `params_mut()` on an operation
/// like any other raw parameter, and a bool still goes out as a bool, so
/// `Some(true.into())` sends exactly what `Some(true)` used to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Cache {
    /// Use a stored copy when there is one, or never.
    Enabled(bool),
    /// Use a stored copy on these terms.
    Control(CacheControl),
    /// A shape the service accepts that this crate has no name for yet. Sent
    /// as given.
    Other(serde_json::Value),
}

impl From<bool> for Cache {
    fn from(enabled: bool) -> Self {
        Self::Enabled(enabled)
    }
}

impl From<CacheControl> for Cache {
    fn from(control: CacheControl) -> Self {
        Self::Control(control)
    }
}

/// How fresh a stored copy has to be to be served.
///
/// The field names and units are the service's. Every field is optional and
/// an unset one leaves the service's default in place, which for the age is
/// two days.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheControl {
    /// The most a stored copy may be, in milliseconds. Zero always fetches
    /// fresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<u64>,
    /// Serve a stored copy however old it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_stale: Option<bool>,
    /// Answer from stored markup without starting a browser at all, when
    /// there is any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_browser: Option<bool>,
    /// A cutoff instant as an RFC 3339 timestamp. A copy stored before it is
    /// not served. Set, it overrides `max_age`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// Any control the service adds later, sent and read back as given.
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl RequestParams {
    /// Parameters for one address, with everything else left to the service.
    pub fn url(url: impl Into<String>) -> RequestParams {
        RequestParams {
            url: Some(url.into()),
            ..RequestParams::default()
        }
    }

    /// Fill in the user agent and viewport that go with a profile.
    ///
    /// Anything already set is left alone, so this can be called after your own
    /// settings without undoing them.
    pub fn apply_profile(&mut self, profile: Profile) {
        if self.user_agent.is_none() {
            if let Some(user_agent) = profile.user_agent() {
                self.user_agent = Some(user_agent.to_string());
            }
        }

        if self.viewport.is_none() {
            self.viewport = Some(profile.viewport());
        }
    }

    /// True when nothing has been set, so the request carries `{}`.
    pub fn is_empty(&self) -> bool {
        self == &RequestParams::default()
    }
}

#[cfg(test)]
mod tests {
    // Setting one field on a default is how this struct is meant to be used, so
    // the tests do it the same way a caller would. A test may also unwrap and
    // panic: one that cannot set itself up should fail loudly rather than
    // quietly measure nothing.
    #![allow(
        clippy::field_reassign_with_default,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;

    #[test]
    fn credential_payloads_are_redacted_but_still_serialize() {
        let secret = "synthetic-private-value";
        let automation = [
            WebAutomation::Evaluate(secret.into()),
            WebAutomation::Type {
                value: secret.into(),
                modifier: None,
            },
            WebAutomation::Fill {
                selector: "input".into(),
                value: secret.into(),
            },
        ];
        let webhook = WebhookSettings::new(format!("https://example.com/{secret}"));
        let rewrite = LinkRewriteRule::replace("old", secret);
        let mut params = RequestParams::url(format!("https://user:{secret}@example.com"));
        params.execution_scripts = Some([("/".into(), secret.into())].into());
        params.evaluate_on_new_document = Some(secret.into());
        params.automation_scripts = Some([("/".into(), automation.to_vec())].into());
        params.webhooks = Some(webhook.clone());
        params.link_rewrite = Some(rewrite.clone());
        for value in [
            format!("{params:?}"),
            format!("{automation:?}"),
            format!("{webhook:?}"),
            format!("{rewrite:?}"),
        ] {
            assert!(!value.contains(secret), "credential in Debug");
            assert!(value.contains("<redacted>"));
        }
        let wire = serde_json::to_value(&params).expect("serialize");
        assert!(wire["execution_scripts"]["/"] == secret);
        assert!(wire["automation_scripts"]["/"][1]["Type"]["value"] == secret);
        assert!(wire["evaluate_on_new_document"] == secret);
        assert!(wire["webhooks"]["destination"] == webhook.destination);
    }

    #[test]
    fn proxy_debug_redacts_userinfo_and_rejects_malformed_input() {
        for proxy in [
            "http://synthetic-user:synthetic-password@example.com:8080",
            "http://synthetic-user:synthetic-password@example.com:8080/private?token=synthetic-token",
            "not a proxy synthetic-password",
        ] {
            let params = RequestParams { remote_proxy: Some(proxy.into()), ..RequestParams::default() };
            let printed = format!("{params:#?}");
            assert!(!printed.contains("synthetic-"), "credential in Debug");
            assert!(printed.contains("<redacted>"));
            if proxy.starts_with("http://") {
                assert!(printed.contains("example.com"));
                assert!(printed.contains("8080"));
            }
            assert!(serde_json::to_value(params).expect("serialize")["remote_proxy"] == proxy);
        }
    }

    #[test]
    fn default_params_serialize_to_an_empty_object() {
        let json = serde_json::to_string(&RequestParams::default()).expect("serialize");

        assert_eq!(json, "{}");
        assert!(RequestParams::default().is_empty());
    }

    #[test]
    fn set_fields_are_the_only_ones_sent() {
        let mut params = RequestParams::url("https://example.com");
        params.request = Some(RequestMode::Browser);

        let json = serde_json::to_value(&params).expect("serialize");
        let object = json.as_object().expect("object");

        assert_eq!(object.len(), 2);
        assert_eq!(object["url"], "https://example.com");
        assert_eq!(object["request"], "browser");
    }

    #[test]
    fn request_mode_wire_values_and_aliases() {
        assert_eq!(
            serde_json::to_string(&RequestMode::Browser).expect("serialize"),
            "\"browser\""
        );
        assert_eq!(
            serde_json::to_string(&RequestMode::Smart).expect("serialize"),
            "\"smart\""
        );
        assert_eq!(
            serde_json::to_string(&RequestMode::Http).expect("serialize"),
            "\"http\""
        );

        for value in ["\"chrome\"", "\"headless\"", "\"BROWSER\"", "\"Chrome\""] {
            let mode: RequestMode = serde_json::from_str(value).expect("deserialize");

            assert_eq!(
                mode,
                RequestMode::Browser,
                "{value} should be a browser mode"
            );
        }

        for value in ["\"smart\"", "\"smart_mode\"", "\"SmartMode\""] {
            let mode: RequestMode = serde_json::from_str(value).expect("deserialize");

            assert_eq!(mode, RequestMode::Smart, "{value} should be smart");
        }

        assert!(serde_json::from_str::<RequestMode>("\"teapot\"").is_err());
        assert_eq!(RequestMode::default(), RequestMode::Smart);
    }

    #[test]
    fn proxy_pool_has_two_members_and_an_alias() {
        assert_eq!(PROXY_POOLS.len(), 2);
        assert_eq!(ProxyPool::default(), ProxyPool::Isp);
        assert_eq!(
            serde_json::to_string(&ProxyPool::Residential).expect("serialize"),
            "\"residential\""
        );
        assert_eq!(
            serde_json::from_str::<ProxyPool>("\"datacenter\"").expect("deserialize"),
            ProxyPool::Isp
        );

        // We do not sell a mobile pool, so there is nothing to select.
        assert!(serde_json::from_str::<ProxyPool>("\"mobile\"").is_err());

        for pool in PROXY_POOLS {
            assert!(matches!(pool, ProxyPool::Isp | ProxyPool::Residential));
            assert_ne!(pool.as_str(), "mobile");
        }
    }

    #[test]
    fn return_format_handling_round_trips_both_shapes() {
        let single = ReturnFormatHandling::Single(ReturnFormat::Markdown);
        let json = serde_json::to_string(&single).expect("serialize");

        assert_eq!(json, "\"markdown\"");
        assert_eq!(
            serde_json::from_str::<ReturnFormatHandling>(&json).expect("deserialize"),
            single
        );

        let multi = ReturnFormatHandling::Multi(vec![ReturnFormat::Markdown, ReturnFormat::Empty]);
        let json = serde_json::to_string(&multi).expect("serialize");

        assert_eq!(json, "[\"markdown\",\"empty\"]");
        assert_eq!(
            serde_json::from_str::<ReturnFormatHandling>(&json).expect("deserialize"),
            multi
        );

        assert!(multi.contains(ReturnFormat::Empty));
        assert_eq!(single.formats(), &[ReturnFormat::Markdown]);

        let err = serde_json::from_str::<ReturnFormatHandling>("\"pdf\"")
            .expect_err("pdf is not a return format");

        assert!(err.to_string().contains("pdf"), "{err}");
    }

    #[test]
    fn country_takes_two_letters_and_sends_them_lowercase() {
        let country = Country::new("DE").expect("de is a country");

        assert_eq!(country.as_str(), "de");
        assert_eq!(
            serde_json::to_string(&country).expect("serialize"),
            "\"de\""
        );
        assert_eq!(
            serde_json::from_str::<Country>("\"US\"").expect("deserialize"),
            Country::new("us").expect("us is a country")
        );

        assert!(Country::new("usa").is_none());
        assert!(Country::new("u").is_none());
        assert!(Country::new("u1").is_none());
        assert!(serde_json::from_str::<Country>("\"usa\"").is_err());
    }

    #[test]
    fn wait_for_sends_only_what_was_asked_for() {
        let mut params = RequestParams::default();
        params.wait_for = Some(WaitFor::idle_network(Timeout::from_millis(1_500)));

        let json = serde_json::to_value(&params).expect("serialize");

        assert_eq!(json["wait_for"]["idle_network"]["timeout"]["secs"], 1);
        assert_eq!(
            json["wait_for"]["idle_network"]["timeout"]["nanos"],
            500_000_000
        );
        assert_eq!(
            json["wait_for"].as_object().expect("object").len(),
            1,
            "an unset wait is not sent"
        );
        assert!(WaitFor::default().is_empty());
    }

    #[test]
    fn extraction_map_carries_css_and_xpath() {
        let mut params = RequestParams::default();
        let mut map = CssExtractionMap::new();

        map.insert(
            "/product".into(),
            vec![
                SelectorGroup::css("price", [".price", "[data-price]"]),
                SelectorGroup::xpath("title", ["//h1"]),
            ],
        );
        params.css_extraction_map = Some(map);
        params.return_format = Some(ReturnFormat::Empty.into());

        let json = serde_json::to_value(&params).expect("serialize");

        assert_eq!(json["css_extraction_map"]["/product"][0]["name"], "price");
        assert_eq!(
            json["css_extraction_map"]["/product"][0]["selectors"][0],
            ".price"
        );
        assert_eq!(
            json["css_extraction_map"]["/product"][1]["selectors"][0],
            "//h1"
        );
        assert_eq!(json["return_format"], "empty");

        let back: RequestParams = serde_json::from_value(json).expect("deserialize");
        let groups = &back.css_extraction_map.expect("map")["/product"];

        assert_eq!(groups[0].selectors[0], ExtractSelector::css(".price"));
        assert_eq!(groups[1].selectors[0], ExtractSelector::xpath("//h1"));
    }

    #[test]
    fn profile_fills_presentation_without_overwriting() {
        let mut params = RequestParams::default();
        params.apply_profile(Profile::MobileDevice);

        assert_eq!(params.user_agent, None, "the service picks this one");
        assert_eq!(params.viewport.as_ref().expect("viewport").width, 390);

        let mut bot = RequestParams::default();
        bot.apply_profile(Profile::Bot);

        assert_eq!(bot.user_agent.as_deref(), Some(BOT_USER_AGENT));

        let mut mine = RequestParams::default();
        mine.user_agent = Some("mine".into());
        mine.apply_profile(Profile::Bot);

        assert_eq!(mine.user_agent.as_deref(), Some("mine"));
    }

    /// The service takes the two credit caps as different types. Sending a
    /// float for `max_credits_allowed` comes back as "Deserialization error:
    /// max_credits_allowed: invalid type: floating point `30.0`, expected u64",
    /// while `max_credits_per_page` is an `f64` and keeps its fraction.
    #[test]
    fn the_whole_operation_cap_goes_out_as_an_integer_and_the_page_cap_as_a_float() {
        let mut params = RequestParams::default();
        params.max_credits_per_page = Some(Credits::from_usd(0.01));
        params.max_credits_allowed = Some(WholeCredits::floor(Credits::new(30.9)));

        let json = serde_json::to_string(&params).expect("serialize");

        assert!(
            json.contains(r#""max_credits_allowed":30"#),
            "a decimal point here is a 400: {json}"
        );
        assert!(json.contains(r#""max_credits_per_page":100.0"#), "{json}");

        let value = serde_json::to_value(&params).expect("serialize");
        assert!(value["max_credits_allowed"].is_u64());
        assert_eq!(value["max_credits_per_page"], 100.0);
        assert_eq!(Credits::new(10_000.0).to_usd(), 1.0);
    }

    #[test]
    fn search_params_flatten_the_fetch_settings() {
        let mut params = SearchParams::new("rust web crawler");
        params.base.request = Some(RequestMode::Http);
        params.search_limit = Some(5);
        params.tbs = Some(TimeWindow::PastWeek);

        let json = serde_json::to_value(&params).expect("serialize");
        let object = json.as_object().expect("object");

        assert_eq!(object["search"], "rust web crawler");
        assert_eq!(object["request"], "http");
        assert_eq!(object["search_limit"], 5);
        assert_eq!(object["tbs"], "qdr:w");
        assert_eq!(object.len(), 4, "nothing unset is sent");
    }

    #[test]
    fn link_rewrite_and_automation_keep_their_wire_shapes() {
        let rule =
            LinkRewriteRule::replace("staging.example.com", "example.com").on_host("example.com");
        let json = serde_json::to_value(&rule).expect("serialize");

        assert_eq!(json["type"], "replace");
        assert_eq!(json["host"], "example.com");
        assert_eq!(
            serde_json::from_value::<LinkRewriteRule>(json).expect("deserialize"),
            rule
        );

        let step = WebAutomation::Fill {
            selector: "#q".into(),
            value: "spider".into(),
        };
        let json = serde_json::to_value(&step).expect("serialize");

        assert_eq!(json["Fill"]["selector"], "#q");
        assert_eq!(
            serde_json::from_value::<WebAutomation>(json).expect("deserialize"),
            step
        );
    }

    /// The names here are the service's. A misspelt one reaches the service as
    /// an unknown field and is dropped without an error, so each is pinned.
    #[test]
    fn documented_fields_go_out_under_the_service_names() {
        let mut params = RequestParams::default();
        params.service_worker_enabled = Some(false);
        params.delay = Some(1_000);
        params.concurrency_limit = Some(8);
        params.wayback = Some(true);
        params.sitemap_only = Some(true);
        params.sitemap_path = Some("sitemap-1.xml".into());
        params.block_ads = Some(false);
        params.block_analytics = Some(false);
        params.block_stylesheets = Some(true);
        params.disable_first_party_stylesheets = Some(true);
        params.disable_first_party_javascript = Some(true);
        params.disable_first_party_visuals = Some(true);
        params.network_whitelist = Some(vec!["example.com".into()]);
        params.network_blacklist = Some(vec!["doubleclick.net".into()]);
        params.exclude_selector = Some(".ad".into());
        params.filter_output_images = Some(true);
        params.filter_output_svg = Some(true);
        params.filter_output_main_only = Some(true);
        params.max_size = Some(200_000);
        params.data_connectors = Some(serde_json::Map::from_iter([(
            "on_find".to_string(),
            serde_json::Value::Bool(true),
        )]));
        params.router = Some(Router {
            mode: Some("fallback".into()),
            ..Router::default()
        });
        params.provider_options = Some(BTreeMap::from([(
            "zyte".to_string(),
            serde_json::json!({"geolocation": "US"}),
        )]));

        let json = serde_json::to_value(&params).expect("serialize");
        let object = json.as_object().expect("object");
        for name in [
            "service_worker_enabled",
            "delay",
            "concurrency_limit",
            "wayback",
            "sitemap_only",
            "sitemap_path",
            "block_ads",
            "block_analytics",
            "block_stylesheets",
            "disable_first_party_stylesheets",
            "disable_first_party_javascript",
            "disable_first_party_visuals",
            "network_whitelist",
            "network_blacklist",
            "exclude_selector",
            "filter_output_images",
            "filter_output_svg",
            "filter_output_main_only",
            "max_size",
            "data_connectors",
            "router",
            "provider_options",
        ] {
            assert!(object.contains_key(name), "{name} missing");
        }
        assert_eq!(object.len(), 22);
        assert_eq!(json["router"], serde_json::json!({"mode": "fallback"}));
        let back: RequestParams = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, params);
    }

    #[test]
    fn retired_fields_are_not_part_of_the_body() {
        let back: RequestParams = serde_json::from_str(
            r#"{"gpt_config":{},"proxy_enabled":true,"smart_mode":true,"custom_prompt":"x","vendor_credentials":{"A":"b"}}"#,
        )
        .expect("unknown fields are ignored");

        assert!(back.is_empty());
    }

    #[test]
    fn router_and_connector_secrets_stay_out_of_debug() {
        let secret = "synthetic-router-token";
        let mut params = RequestParams::default();
        params.router = Some(Router {
            provider: Some("zyte".into()),
            token: Some(secret.into()),
            credentials: Some(BTreeMap::from([("NAME".to_string(), secret.to_string())])),
            ..Router::default()
        });
        params.data_connectors = Some(serde_json::Map::from_iter([(
            "s3".to_string(),
            serde_json::json!({"secret_access_key": secret}),
        )]));

        let printed = format!("{params:?}");
        assert!(!printed.contains(secret), "credential in Debug");
        assert!(printed.contains("zyte"));
        assert_eq!(
            serde_json::to_value(&params).expect("serialize")["router"]["token"],
            secret
        );
    }

    #[test]
    fn search_engine_uses_the_service_names() {
        for (engine, wire) in [
            (SearchEngine::Google, r#""google""#),
            (SearchEngine::Brave, r#""brave""#),
            (SearchEngine::All, r#""generic""#),
        ] {
            assert_eq!(serde_json::to_string(&engine).expect("serialize"), wire);
        }
        assert_eq!(
            serde_json::from_str::<SearchEngine>(r#""all""#).expect("alias"),
            SearchEngine::All
        );
    }

    #[test]
    fn screenshot_params_flatten_the_fetch_settings() {
        let mut shot = ScreenshotParams::default();
        shot.base = RequestParams::url("https://example.com");
        shot.full_page = Some(true);
        shot.fast = Some(false);
        shot.cdp_params = Some(serde_json::Map::from_iter([(
            "format".to_string(),
            serde_json::Value::from("jpeg"),
        )]));

        let json = serde_json::to_value(&shot).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "url": "https://example.com",
                "full_page": true,
                "fast": false,
                "cdp_params": {"format": "jpeg"}
            })
        );
    }

    #[test]
    fn a_full_body_round_trips() {
        let mut params = RequestParams::url("https://example.com");
        params.request = Some(RequestMode::Browser);
        params.proxy = Some(ProxyPool::Residential);
        params.country_code = Country::new("gb");
        params.return_format = Some(ReturnFormatHandling::Multi(vec![
            ReturnFormat::Markdown,
            ReturnFormat::Text,
        ]));
        params.wait_for = Some(WaitFor::selector("main", Timeout::from_secs(5)));
        params.redirect_policy = Some(RedirectPolicy::Loose);
        params.chunking_alg = Some(ChunkingAlg::new(ChunkingKind::BySentence, 20));
        params.budget = Some(CrawlBudget::from([("*".to_string(), 25)]));
        params.webhooks = Some(WebhookSettings::new("https://example.com/hook"));
        params.max_credits_allowed = Some(WholeCredits::floor(Credits::new(5_000.0)));

        let json = serde_json::to_string(&params).expect("serialize");
        let back: RequestParams = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back, params);
        assert!(!json.contains("proxy_enabled"), "dropped from the surface");
        assert!(!json.contains("pipeline"));
    }

    /// Every event tracker switch goes out, an unset one as false.
    #[test]
    fn an_event_tracker_always_sends_all_three_switches() {
        let mut params = RequestParams::default();
        params.event_tracker = Some(EventTracker {
            responses: Some(true),
            ..EventTracker::default()
        });
        let json = serde_json::to_value(&params).expect("serialize");
        assert_eq!(
            json["event_tracker"],
            serde_json::json!({"responses": true, "requests": false, "automation": false})
        );

        params.event_tracker = Some(EventTracker::default());
        let json = serde_json::to_value(&params).expect("serialize");
        assert_eq!(
            json["event_tracker"],
            serde_json::json!({"responses": false, "requests": false, "automation": false})
        );

        // Reading back still takes a partial object.
        let partial: EventTracker = serde_json::from_str(r#"{"automation": true}"#).expect("parse");
        assert_eq!(partial.automation, Some(true));
        assert_eq!(partial.responses, None);
    }
}
