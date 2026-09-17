//! Every request parameter, what it holds, and which ones may be learned.
//!
//! [`Key`] names each field of the client's `RequestParams` in the order they
//! are declared there, and [`Schema::v1`] says for each one what kind of value
//! it takes, what it needs to take effect, whether an edit may touch it, and
//! whether changing it can change the content that comes back. A test
//! serializes a fully populated request and fails when a field exists on one
//! side and not the other.
//!
//! The learnable set in version one is small on purpose: the fetch mode, the
//! proxy pool, an idle network wait, five resource switches and the network
//! blacklist. Each is a label a model has to learn from rows that do not exist
//! yet, so the set grows when the rows justify it.

use spider_route::{ProxyPool, RequestMode, PROXY_POOLS};

/// The version of the key table and its specs. Recorded on every row.
pub const SCHEMA_VERSION: u16 = 1;

/// How many keys there are.
pub const KEY_COUNT: usize = 80;

/// One field of the request body.
///
/// Declared in the order `RequestParams` declares its fields, so
/// [`Key::index`] is stable for a given schema version. `WaitIdleMillis`
/// stands for `wait_for`, because the only wait an edit writes is an idle
/// network wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
#[repr(u8)]
pub enum Key {
    /// `url`
    Url,
    /// `request`
    Request,
    /// `proxy`
    Proxy,
    /// `country_code`
    CountryCode,
    /// `remote_proxy`
    RemoteProxy,
    /// `stealth`
    Stealth,
    /// `fingerprint`
    Fingerprint,
    /// `user_agent`
    UserAgent,
    /// `viewport`
    Viewport,
    /// `locale`
    Locale,
    /// `cookies`
    Cookies,
    /// `headers`
    Headers,
    /// `encoding`
    Encoding,
    /// `storageless`
    Storageless,
    /// `session`
    Session,
    /// `redirect_policy`
    RedirectPolicy,
    /// `request_timeout`
    RequestTimeout,
    /// `service_worker_enabled`
    ServiceWorkerEnabled,
    /// `preserve_host`
    PreserveHost,
    /// `delay`
    Delay,
    /// `concurrency_limit`
    ConcurrencyLimit,
    /// `wayback`
    Wayback,
    /// `limit`
    Limit,
    /// `depth`
    Depth,
    /// `budget`
    Budget,
    /// `blacklist`
    Blacklist,
    /// `whitelist`
    Whitelist,
    /// `subdomains`
    Subdomains,
    /// `tld`
    Tld,
    /// `external_domains`
    ExternalDomains,
    /// `sitemap`
    Sitemap,
    /// `sitemap_only`
    SitemapOnly,
    /// `sitemap_path`
    SitemapPath,
    /// `respect_robots`
    RespectRobots,
    /// `link_rewrite`
    LinkRewrite,
    /// `crawl_timeout`
    CrawlTimeout,
    /// `run_in_background`
    RunInBackground,
    /// `wait_for`, as an idle network wait in milliseconds.
    WaitIdleMillis,
    /// `scroll`
    Scroll,
    /// `execution_scripts`
    ExecutionScripts,
    /// `automation_scripts`
    AutomationScripts,
    /// `evaluate_on_new_document`
    EvaluateOnNewDocument,
    /// `disable_intercept`
    DisableIntercept,
    /// `full_resources`
    FullResources,
    /// `block_ads`
    BlockAds,
    /// `block_analytics`
    BlockAnalytics,
    /// `block_stylesheets`
    BlockStylesheets,
    /// `disable_first_party_stylesheets`
    DisableFirstPartyStylesheets,
    /// `disable_first_party_javascript`
    DisableFirstPartyJavascript,
    /// `disable_first_party_visuals`
    DisableFirstPartyVisuals,
    /// `network_whitelist`
    NetworkWhitelist,
    /// `network_blacklist`
    NetworkBlacklist,
    /// `event_tracker`
    EventTracker,
    /// `return_format`
    ReturnFormat,
    /// `root_selector`
    RootSelector,
    /// `exclude_selector`
    ExcludeSelector,
    /// `filter_output_images`
    FilterOutputImages,
    /// `filter_output_svg`
    FilterOutputSvg,
    /// `filter_output_main_only`
    FilterOutputMainOnly,
    /// `max_size`
    MaxSize,
    /// `readability`
    Readability,
    /// `clean_html`
    CleanHtml,
    /// `css_extraction_map`
    CssExtractionMap,
    /// `chunking_alg`
    ChunkingAlg,
    /// `metadata`
    Metadata,
    /// `return_embeddings`
    ReturnEmbeddings,
    /// `return_headers`
    ReturnHeaders,
    /// `return_cookies`
    ReturnCookies,
    /// `return_page_links`
    ReturnPageLinks,
    /// `return_json_data`
    ReturnJsonData,
    /// `text`
    Text,
    /// `cache`
    Cache,
    /// `webhooks`
    Webhooks,
    /// `data_connectors`
    DataConnectors,
    /// `router`
    Router,
    /// `provider_options`
    ProviderOptions,
    /// `max_credits_allowed`
    MaxCreditsAllowed,
    /// `max_credits_per_page`
    MaxCreditsPerPage,
    /// `disable_hints`
    DisableHints,
    /// `skip_config_checks`
    SkipConfigChecks,
}

/// Every key, in index order.
const ALL_KEYS: [Key; KEY_COUNT] = [
    Key::Url,
    Key::Request,
    Key::Proxy,
    Key::CountryCode,
    Key::RemoteProxy,
    Key::Stealth,
    Key::Fingerprint,
    Key::UserAgent,
    Key::Viewport,
    Key::Locale,
    Key::Cookies,
    Key::Headers,
    Key::Encoding,
    Key::Storageless,
    Key::Session,
    Key::RedirectPolicy,
    Key::RequestTimeout,
    Key::ServiceWorkerEnabled,
    Key::PreserveHost,
    Key::Delay,
    Key::ConcurrencyLimit,
    Key::Wayback,
    Key::Limit,
    Key::Depth,
    Key::Budget,
    Key::Blacklist,
    Key::Whitelist,
    Key::Subdomains,
    Key::Tld,
    Key::ExternalDomains,
    Key::Sitemap,
    Key::SitemapOnly,
    Key::SitemapPath,
    Key::RespectRobots,
    Key::LinkRewrite,
    Key::CrawlTimeout,
    Key::RunInBackground,
    Key::WaitIdleMillis,
    Key::Scroll,
    Key::ExecutionScripts,
    Key::AutomationScripts,
    Key::EvaluateOnNewDocument,
    Key::DisableIntercept,
    Key::FullResources,
    Key::BlockAds,
    Key::BlockAnalytics,
    Key::BlockStylesheets,
    Key::DisableFirstPartyStylesheets,
    Key::DisableFirstPartyJavascript,
    Key::DisableFirstPartyVisuals,
    Key::NetworkWhitelist,
    Key::NetworkBlacklist,
    Key::EventTracker,
    Key::ReturnFormat,
    Key::RootSelector,
    Key::ExcludeSelector,
    Key::FilterOutputImages,
    Key::FilterOutputSvg,
    Key::FilterOutputMainOnly,
    Key::MaxSize,
    Key::Readability,
    Key::CleanHtml,
    Key::CssExtractionMap,
    Key::ChunkingAlg,
    Key::Metadata,
    Key::ReturnEmbeddings,
    Key::ReturnHeaders,
    Key::ReturnCookies,
    Key::ReturnPageLinks,
    Key::ReturnJsonData,
    Key::Text,
    Key::Cache,
    Key::Webhooks,
    Key::DataConnectors,
    Key::Router,
    Key::ProviderOptions,
    Key::MaxCreditsAllowed,
    Key::MaxCreditsPerPage,
    Key::DisableHints,
    Key::SkipConfigChecks,
];

impl Key {
    /// Every key, in index order.
    pub const ALL: &'static [Key] = &ALL_KEYS;

    /// Where this key sits in [`Key::ALL`] and in the spec table.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The field name on the wire.
    pub const fn wire(self) -> &'static str {
        match self {
            Key::Url => "url",
            Key::Request => "request",
            Key::Proxy => "proxy",
            Key::CountryCode => "country_code",
            Key::RemoteProxy => "remote_proxy",
            Key::Stealth => "stealth",
            Key::Fingerprint => "fingerprint",
            Key::UserAgent => "user_agent",
            Key::Viewport => "viewport",
            Key::Locale => "locale",
            Key::Cookies => "cookies",
            Key::Headers => "headers",
            Key::Encoding => "encoding",
            Key::Storageless => "storageless",
            Key::Session => "session",
            Key::RedirectPolicy => "redirect_policy",
            Key::RequestTimeout => "request_timeout",
            Key::ServiceWorkerEnabled => "service_worker_enabled",
            Key::PreserveHost => "preserve_host",
            Key::Delay => "delay",
            Key::ConcurrencyLimit => "concurrency_limit",
            Key::Wayback => "wayback",
            Key::Limit => "limit",
            Key::Depth => "depth",
            Key::Budget => "budget",
            Key::Blacklist => "blacklist",
            Key::Whitelist => "whitelist",
            Key::Subdomains => "subdomains",
            Key::Tld => "tld",
            Key::ExternalDomains => "external_domains",
            Key::Sitemap => "sitemap",
            Key::SitemapOnly => "sitemap_only",
            Key::SitemapPath => "sitemap_path",
            Key::RespectRobots => "respect_robots",
            Key::LinkRewrite => "link_rewrite",
            Key::CrawlTimeout => "crawl_timeout",
            Key::RunInBackground => "run_in_background",
            Key::WaitIdleMillis => "wait_for",
            Key::Scroll => "scroll",
            Key::ExecutionScripts => "execution_scripts",
            Key::AutomationScripts => "automation_scripts",
            Key::EvaluateOnNewDocument => "evaluate_on_new_document",
            Key::DisableIntercept => "disable_intercept",
            Key::FullResources => "full_resources",
            Key::BlockAds => "block_ads",
            Key::BlockAnalytics => "block_analytics",
            Key::BlockStylesheets => "block_stylesheets",
            Key::DisableFirstPartyStylesheets => "disable_first_party_stylesheets",
            Key::DisableFirstPartyJavascript => "disable_first_party_javascript",
            Key::DisableFirstPartyVisuals => "disable_first_party_visuals",
            Key::NetworkWhitelist => "network_whitelist",
            Key::NetworkBlacklist => "network_blacklist",
            Key::EventTracker => "event_tracker",
            Key::ReturnFormat => "return_format",
            Key::RootSelector => "root_selector",
            Key::ExcludeSelector => "exclude_selector",
            Key::FilterOutputImages => "filter_output_images",
            Key::FilterOutputSvg => "filter_output_svg",
            Key::FilterOutputMainOnly => "filter_output_main_only",
            Key::MaxSize => "max_size",
            Key::Readability => "readability",
            Key::CleanHtml => "clean_html",
            Key::CssExtractionMap => "css_extraction_map",
            Key::ChunkingAlg => "chunking_alg",
            Key::Metadata => "metadata",
            Key::ReturnEmbeddings => "return_embeddings",
            Key::ReturnHeaders => "return_headers",
            Key::ReturnCookies => "return_cookies",
            Key::ReturnPageLinks => "return_page_links",
            Key::ReturnJsonData => "return_json_data",
            Key::Text => "text",
            Key::Cache => "cache",
            Key::Webhooks => "webhooks",
            Key::DataConnectors => "data_connectors",
            Key::Router => "router",
            Key::ProviderOptions => "provider_options",
            Key::MaxCreditsAllowed => "max_credits_allowed",
            Key::MaxCreditsPerPage => "max_credits_per_page",
            Key::DisableHints => "disable_hints",
            Key::SkipConfigChecks => "skip_config_checks",
        }
    }
}

/// What a field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// True or false.
    Bool,
    /// One of these wire values, in index order.
    Enum(&'static [&'static str]),
    /// A whole number in this range, inclusive.
    IntRange {
        /// The smallest value the service accepts.
        min: i64,
        /// The largest value the service accepts.
        max: i64,
    },
    /// A span in milliseconds, limited to these buckets.
    MillisBucket(&'static [u32]),
    /// A list of strings.
    StringList,
    /// Keyed values.
    Map,
    /// Anything this schema does not model: free text, nested settings,
    /// floats. Never learnable.
    Opaque,
}

/// What a field needs before setting it does anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dependency {
    /// The request is `browser` or `smart`. Everything that acts on a page
    /// the service renders is ignored by a plain HTTP fetch, which still costs
    /// what it costs.
    Rendered,
    /// The other field is not active on the same request.
    NotWith(Key),
}

/// A coarse grouping of the fields, for reading the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// How the request is sent and presented.
    Transport,
    /// Where it leaves from.
    Proxy,
    /// How far a crawl goes.
    Scope,
    /// What has to happen before the page is read.
    Wait,
    /// What runs on the page.
    Scripting,
    /// What the page is allowed to load.
    Resources,
    /// What comes back and in what shape.
    Extraction,
    /// Stored copies.
    Cache,
    /// Where results go besides the response.
    Delivery,
    /// What the request may spend.
    Budget,
    /// Everything else.
    Other,
}

/// One field's entry in the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamSpec {
    /// The field.
    pub key: Key,
    /// What it holds.
    pub kind: Kind,
    /// What it needs before it does anything.
    pub requires: &'static [Dependency],
    /// Whether an edit may write it.
    pub learnable: bool,
    /// Whether changing it can change the content that comes back, rather
    /// than only how the fetch is carried out.
    pub content_changing: bool,
    /// Where it sits in the table.
    pub group: Group,
}

impl ParamSpec {
    /// A field no edit touches.
    const fn fixed(key: Key, kind: Kind, group: Group) -> ParamSpec {
        ParamSpec {
            key,
            kind,
            requires: &[],
            learnable: false,
            content_changing: false,
            group,
        }
    }

    /// The same spec, open to edits.
    const fn learnable(mut self) -> ParamSpec {
        self.learnable = true;
        self
    }

    /// The same spec, marked as able to change the content.
    const fn content(mut self) -> ParamSpec {
        self.content_changing = true;
        self
    }

    /// The same spec, with these dependencies.
    const fn needs(mut self, requires: &'static [Dependency]) -> ParamSpec {
        self.requires = requires;
        self
    }
}

/// The fetch modes, in the order the `Request` kind lists them.
pub const MODES: [RequestMode; 3] = [RequestMode::Http, RequestMode::Smart, RequestMode::Browser];

/// The proxy pools, in the order the `Proxy` kind lists them.
pub const PROXIES: [ProxyPool; 2] = PROXY_POOLS;

/// The idle network waits an edit may set, in milliseconds.
pub const WAIT_BUCKETS: [u32; 4] = [0, 2_000, 5_000, 10_000];

const RENDERED: &[Dependency] = &[Dependency::Rendered];

// `disable_intercept` lets first party scripts through; the service keeps
// request interception on and still applies the network lists, so the two
// keys do not exclude each other. Checked against the service on 2026-09-16.
const BLACKLIST_NEEDS: &[Dependency] = RENDERED;
const INTERCEPT_NEEDS: &[Dependency] = RENDERED;

const U32_MAX: i64 = 4_294_967_295;
const I64_MAX: i64 = 9_223_372_036_854_775_807;

/// Never returned for a real key. See [`Schema::spec`].
const UNKNOWN: ParamSpec = ParamSpec::fixed(Key::Url, Kind::Opaque, Group::Other);

use self::{Group as G, Kind as K, ParamSpec as S};

/// The version one table, in key order.
const SPECS: [ParamSpec; KEY_COUNT] = [
    S::fixed(Key::Url, K::Opaque, G::Transport),
    S::fixed(
        Key::Request,
        K::Enum(&["http", "smart", "browser"]),
        G::Transport,
    )
    .learnable(),
    S::fixed(Key::Proxy, K::Enum(&["isp", "residential"]), G::Proxy).learnable(),
    S::fixed(Key::CountryCode, K::Opaque, G::Proxy),
    S::fixed(Key::RemoteProxy, K::Opaque, G::Proxy),
    S::fixed(Key::Stealth, K::Bool, G::Transport),
    S::fixed(Key::Fingerprint, K::Bool, G::Transport),
    S::fixed(Key::UserAgent, K::Opaque, G::Transport),
    S::fixed(Key::Viewport, K::Opaque, G::Transport),
    S::fixed(Key::Locale, K::Opaque, G::Transport),
    S::fixed(Key::Cookies, K::Opaque, G::Transport),
    S::fixed(Key::Headers, K::Map, G::Transport),
    S::fixed(Key::Encoding, K::Opaque, G::Transport),
    S::fixed(Key::Storageless, K::Bool, G::Transport),
    S::fixed(Key::Session, K::Bool, G::Transport),
    S::fixed(Key::RedirectPolicy, K::Opaque, G::Transport),
    S::fixed(
        Key::RequestTimeout,
        K::IntRange { min: 0, max: 255 },
        G::Transport,
    ),
    S::fixed(Key::ServiceWorkerEnabled, K::Bool, G::Transport),
    S::fixed(Key::PreserveHost, K::Bool, G::Transport),
    S::fixed(
        Key::Delay,
        K::IntRange {
            min: 0,
            max: 60_000,
        },
        G::Transport,
    ),
    S::fixed(
        Key::ConcurrencyLimit,
        K::IntRange {
            min: 0,
            max: U32_MAX,
        },
        G::Transport,
    ),
    S::fixed(Key::Wayback, K::Bool, G::Transport),
    S::fixed(
        Key::Limit,
        K::IntRange {
            min: 0,
            max: U32_MAX,
        },
        G::Scope,
    ),
    S::fixed(
        Key::Depth,
        K::IntRange {
            min: 0,
            max: U32_MAX,
        },
        G::Scope,
    ),
    S::fixed(Key::Budget, K::Map, G::Scope),
    S::fixed(Key::Blacklist, K::StringList, G::Scope),
    S::fixed(Key::Whitelist, K::StringList, G::Scope),
    S::fixed(Key::Subdomains, K::Bool, G::Scope),
    S::fixed(Key::Tld, K::Bool, G::Scope),
    S::fixed(Key::ExternalDomains, K::StringList, G::Scope),
    S::fixed(Key::Sitemap, K::Bool, G::Scope),
    S::fixed(Key::SitemapOnly, K::Bool, G::Scope),
    S::fixed(Key::SitemapPath, K::Opaque, G::Scope),
    S::fixed(Key::RespectRobots, K::Bool, G::Scope),
    S::fixed(Key::LinkRewrite, K::Opaque, G::Scope),
    S::fixed(Key::CrawlTimeout, K::Opaque, G::Scope),
    S::fixed(Key::RunInBackground, K::Bool, G::Delivery),
    S::fixed(Key::WaitIdleMillis, K::MillisBucket(&WAIT_BUCKETS), G::Wait)
        .learnable()
        .needs(RENDERED),
    S::fixed(
        Key::Scroll,
        K::IntRange {
            min: 0,
            max: U32_MAX,
        },
        G::Wait,
    ),
    S::fixed(Key::ExecutionScripts, K::Map, G::Scripting),
    S::fixed(Key::AutomationScripts, K::Map, G::Scripting),
    S::fixed(Key::EvaluateOnNewDocument, K::Opaque, G::Scripting),
    S::fixed(Key::DisableIntercept, K::Bool, G::Resources)
        .learnable()
        .needs(INTERCEPT_NEEDS),
    S::fixed(Key::FullResources, K::Bool, G::Resources)
        .learnable()
        .needs(RENDERED),
    S::fixed(Key::BlockAds, K::Bool, G::Resources)
        .learnable()
        .needs(RENDERED),
    S::fixed(Key::BlockAnalytics, K::Bool, G::Resources)
        .learnable()
        .needs(RENDERED),
    S::fixed(Key::BlockStylesheets, K::Bool, G::Resources)
        .learnable()
        .content()
        .needs(RENDERED),
    S::fixed(Key::DisableFirstPartyStylesheets, K::Bool, G::Resources).content(),
    S::fixed(Key::DisableFirstPartyJavascript, K::Bool, G::Resources).content(),
    S::fixed(Key::DisableFirstPartyVisuals, K::Bool, G::Resources).content(),
    S::fixed(Key::NetworkWhitelist, K::StringList, G::Resources).content(),
    S::fixed(Key::NetworkBlacklist, K::StringList, G::Resources)
        .learnable()
        .content()
        .needs(BLACKLIST_NEEDS),
    S::fixed(Key::EventTracker, K::Opaque, G::Other),
    S::fixed(Key::ReturnFormat, K::Opaque, G::Extraction).content(),
    S::fixed(Key::RootSelector, K::Opaque, G::Extraction).content(),
    S::fixed(Key::ExcludeSelector, K::Opaque, G::Extraction).content(),
    S::fixed(Key::FilterOutputImages, K::Bool, G::Extraction).content(),
    S::fixed(Key::FilterOutputSvg, K::Bool, G::Extraction).content(),
    S::fixed(Key::FilterOutputMainOnly, K::Bool, G::Extraction).content(),
    S::fixed(
        Key::MaxSize,
        K::IntRange {
            min: 0,
            max: I64_MAX,
        },
        G::Extraction,
    )
    .content(),
    S::fixed(Key::Readability, K::Bool, G::Extraction).content(),
    S::fixed(Key::CleanHtml, K::Bool, G::Extraction).content(),
    S::fixed(Key::CssExtractionMap, K::Map, G::Extraction).content(),
    S::fixed(Key::ChunkingAlg, K::Opaque, G::Extraction).content(),
    S::fixed(Key::Metadata, K::Bool, G::Extraction),
    S::fixed(Key::ReturnEmbeddings, K::Bool, G::Extraction),
    S::fixed(Key::ReturnHeaders, K::Bool, G::Extraction),
    S::fixed(Key::ReturnCookies, K::Bool, G::Extraction),
    S::fixed(Key::ReturnPageLinks, K::Bool, G::Extraction),
    S::fixed(Key::ReturnJsonData, K::Bool, G::Extraction),
    S::fixed(Key::Text, K::Opaque, G::Extraction).content(),
    S::fixed(Key::Cache, K::Opaque, G::Cache),
    S::fixed(Key::Webhooks, K::Opaque, G::Delivery),
    S::fixed(Key::DataConnectors, K::Map, G::Delivery),
    S::fixed(Key::Router, K::Opaque, G::Other),
    S::fixed(Key::ProviderOptions, K::Map, G::Other),
    S::fixed(
        Key::MaxCreditsAllowed,
        K::IntRange {
            min: 0,
            max: I64_MAX,
        },
        G::Budget,
    ),
    S::fixed(Key::MaxCreditsPerPage, K::Opaque, G::Budget),
    S::fixed(Key::DisableHints, K::Bool, G::Other),
    S::fixed(Key::SkipConfigChecks, K::Bool, G::Other),
];

// The table and the key list line up slot for slot, and the last key is the
// last slot, so `Key::index` is always inside the table. Checked when the
// crate compiles rather than when a request is edited.
const _: () = {
    let mut at = 0;
    let mut keys: &[Key] = &ALL_KEYS;
    let mut specs: &[ParamSpec] = &SPECS;
    while let ([key, keys_rest @ ..], [spec, specs_rest @ ..]) = (keys, specs) {
        assert!(*key as usize == at);
        assert!(spec.key as usize == at);
        keys = keys_rest;
        specs = specs_rest;
        at += 1;
    }
    assert!(at == KEY_COUNT);
    assert!(Key::SkipConfigChecks as usize + 1 == KEY_COUNT);
};

/// The parameter table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schema {
    specs: &'static [ParamSpec; KEY_COUNT],
}

impl Schema {
    /// The version one table.
    pub const fn v1() -> Schema {
        Schema { specs: &SPECS }
    }

    /// One field's spec.
    pub fn spec(&self, key: Key) -> &ParamSpec {
        // The compile time block above proves every key indexes inside the
        // table. Were that ever wrong, an opaque spec refuses every edit
        // rather than panicking.
        match self.specs.get(key.index()) {
            Some(spec) => spec,
            None => &UNKNOWN,
        }
    }

    /// The fields an edit may write, in key order.
    pub fn learnable(&self) -> impl Iterator<Item = &ParamSpec> {
        self.specs.iter().filter(|spec| spec.learnable)
    }

    /// Every field, in key order.
    pub fn specs(&self) -> &[ParamSpec] {
        self.specs
    }
}

impl Default for Schema {
    fn default() -> Schema {
        Schema::v1()
    }
}

/// The position of a learnable key among the learnable keys, in key order.
pub const fn learnable_slot(key: Key) -> Option<usize> {
    match key {
        Key::Request => Some(0),
        Key::Proxy => Some(1),
        Key::WaitIdleMillis => Some(2),
        Key::DisableIntercept => Some(3),
        Key::FullResources => Some(4),
        Key::BlockAds => Some(5),
        Key::BlockAnalytics => Some(6),
        Key::BlockStylesheets => Some(7),
        Key::NetworkBlacklist => Some(8),
        _ => None,
    }
}

/// What the service does when a `Bool` field is left unset.
///
/// The three blocking switches default to on at the service, so `None` and
/// `Some(true)` send the same request. Everything else is off until set.
pub const fn service_default(key: Key) -> bool {
    matches!(
        key,
        Key::BlockAds | Key::BlockAnalytics | Key::BlockStylesheets
    )
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;
    use spider_cloud_agent::params::{
        Cache, ChunkingAlg, ChunkingKind, CrawlBudget, CssExtractionMap, EventTracker,
        LinkRewriteRule, RedirectPolicy, RequestParams, ReturnFormat, Router, SelectorGroup,
        Timeout, Viewport, WaitFor, WebAutomation, WebhookSettings,
    };
    use spider_cloud_agent::{Credits, WholeCredits};
    use std::collections::{BTreeMap, BTreeSet};

    /// Every field set by hand. A field added to `RequestParams` and not set
    /// here is caught too, because `RequestParams` has no `..Default` below.
    pub(crate) fn populated() -> RequestParams {
        let mut css = CssExtractionMap::new();
        css.insert("/".into(), vec![SelectorGroup::css("title", ["h1"])]);
        RequestParams {
            url: Some("https://example.com/".into()),
            request: Some(RequestMode::Browser),
            proxy: Some(ProxyPool::Residential),
            country_code: spider_route::Country::new("de"),
            remote_proxy: Some("http://example.com:8080".into()),
            stealth: Some(true),
            fingerprint: Some(true),
            user_agent: Some("agent".into()),
            viewport: Some(Viewport::desktop()),
            locale: Some("en-GB".into()),
            cookies: Some("a=b".into()),
            headers: Some(BTreeMap::from([("x".to_string(), "y".to_string())])),
            encoding: Some("utf-8".into()),
            storageless: Some(true),
            session: Some(true),
            redirect_policy: Some(RedirectPolicy::Loose),
            request_timeout: Some(30),
            service_worker_enabled: Some(false),
            preserve_host: Some(true),
            delay: Some(10),
            concurrency_limit: Some(2),
            wayback: Some(true),
            limit: Some(5),
            depth: Some(2),
            budget: Some(CrawlBudget::from([("*".to_string(), 3)])),
            blacklist: Some(vec!["/a".into()]),
            whitelist: Some(vec!["/b".into()]),
            subdomains: Some(true),
            tld: Some(true),
            external_domains: Some(vec!["example.org".into()]),
            sitemap: Some(true),
            sitemap_only: Some(true),
            sitemap_path: Some("sitemap.xml".into()),
            respect_robots: Some(true),
            link_rewrite: Some(LinkRewriteRule::replace("a", "b")),
            crawl_timeout: Some(Timeout::from_secs(5)),
            run_in_background: Some(false),
            wait_for: Some(WaitFor::idle_network(Timeout::from_millis(2_000))),
            scroll: Some(1),
            execution_scripts: Some(BTreeMap::from([("/".to_string(), "1".to_string())])),
            automation_scripts: Some(BTreeMap::from([(
                "/".to_string(),
                vec![WebAutomation::Evaluate("1".into())],
            )])),
            evaluate_on_new_document: Some("1".into()),
            disable_intercept: Some(false),
            full_resources: Some(false),
            block_ads: Some(true),
            block_analytics: Some(true),
            block_stylesheets: Some(true),
            disable_first_party_stylesheets: Some(false),
            disable_first_party_javascript: Some(false),
            disable_first_party_visuals: Some(false),
            network_whitelist: Some(vec!["example.com".into()]),
            network_blacklist: Some(vec!["example.net".into()]),
            event_tracker: Some(EventTracker::default()),
            return_format: Some(ReturnFormat::Markdown.into()),
            root_selector: Some("main".into()),
            exclude_selector: Some("nav".into()),
            filter_output_images: Some(true),
            filter_output_svg: Some(true),
            filter_output_main_only: Some(true),
            max_size: Some(1_000),
            readability: Some(true),
            clean_html: Some(true),
            css_extraction_map: Some(css),
            chunking_alg: Some(ChunkingAlg::new(ChunkingKind::BySentence, 2)),
            metadata: Some(true),
            return_embeddings: Some(true),
            return_headers: Some(true),
            return_cookies: Some(true),
            return_page_links: Some(true),
            return_json_data: Some(true),
            text: Some("text".into()),
            cache: Some(Cache::Enabled(true)),
            webhooks: Some(WebhookSettings::new("https://example.com/hook")),
            data_connectors: Some(serde_json::Map::from_iter([(
                "s3".to_string(),
                serde_json::Value::Bool(true),
            )])),
            router: Some(Router {
                mode: Some("off".into()),
                ..Router::default()
            }),
            provider_options: Some(BTreeMap::from([(
                "p".to_string(),
                serde_json::Value::Bool(true),
            )])),
            max_credits_allowed: Some(WholeCredits::floor(Credits::new(10.0))),
            max_credits_per_page: Some(Credits::new(1.0)),
            disable_hints: Some(true),
            skip_config_checks: Some(false),
        }
    }

    #[test]
    fn every_request_params_field_has_a_spec() {
        let json = serde_json::to_value(populated()).unwrap();
        let wire: BTreeSet<String> = json.as_object().unwrap().keys().cloned().collect();
        let keys: BTreeSet<String> = Key::ALL.iter().map(|k| k.wire().to_string()).collect();

        let missing_spec: Vec<_> = wire.difference(&keys).collect();
        let missing_field: Vec<_> = keys.difference(&wire).collect();
        assert!(
            missing_spec.is_empty(),
            "fields with no Key: {missing_spec:?}"
        );
        assert!(
            missing_field.is_empty(),
            "Keys with no field: {missing_field:?}"
        );
        assert_eq!(Key::ALL.len(), wire.len());
        assert_eq!(keys.len(), Key::ALL.len(), "two keys share a wire name");
    }

    #[test]
    fn keys_follow_the_declaration_order_of_the_struct() {
        // serde writes fields in declaration order, so the serialized order is
        // the struct's order.
        let text = serde_json::to_string(&populated()).unwrap();
        let mut last = 0;
        for key in Key::ALL {
            let at = text
                .find(&format!("\"{}\":", key.wire()))
                .unwrap_or_else(|| panic!("{} is not on the wire", key.wire()));
            assert!(at >= last, "{:?} is out of order", key);
            last = at;
        }
    }

    #[test]
    fn the_learnable_set_is_exactly_version_one() {
        let schema = Schema::v1();
        let learnable: Vec<Key> = schema.learnable().map(|spec| spec.key).collect();
        assert_eq!(
            learnable,
            [
                Key::Request,
                Key::Proxy,
                Key::WaitIdleMillis,
                Key::DisableIntercept,
                Key::FullResources,
                Key::BlockAds,
                Key::BlockAnalytics,
                Key::BlockStylesheets,
                Key::NetworkBlacklist,
            ]
        );
        for (slot, key) in learnable.iter().enumerate() {
            assert_eq!(learnable_slot(*key), Some(slot));
        }
        for spec in schema.specs() {
            if !spec.learnable {
                assert_eq!(learnable_slot(spec.key), None, "{:?}", spec.key);
            }
        }

        let content: BTreeSet<Key> = schema
            .specs()
            .iter()
            .filter(|spec| spec.content_changing)
            .map(|spec| spec.key)
            .collect();
        let expected = BTreeSet::from([
            Key::Readability,
            Key::CleanHtml,
            Key::RootSelector,
            Key::ExcludeSelector,
            Key::ReturnFormat,
            Key::FilterOutputMainOnly,
            Key::FilterOutputImages,
            Key::FilterOutputSvg,
            Key::MaxSize,
            Key::ChunkingAlg,
            Key::CssExtractionMap,
            Key::Text,
            Key::BlockStylesheets,
            Key::DisableFirstPartyStylesheets,
            Key::DisableFirstPartyJavascript,
            Key::DisableFirstPartyVisuals,
            Key::NetworkBlacklist,
            Key::NetworkWhitelist,
        ]);
        assert_eq!(content, expected);
    }

    #[test]
    fn the_spec_for_a_key_is_that_keys_spec() {
        let schema = Schema::v1();
        for key in Key::ALL {
            assert_eq!(schema.spec(*key).key, *key);
        }
        assert_eq!(
            schema.spec(Key::WaitIdleMillis).kind,
            Kind::MillisBucket(&[0, 2_000, 5_000, 10_000])
        );
    }
}
