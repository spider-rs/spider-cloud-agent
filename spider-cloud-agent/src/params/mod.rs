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
pub use search::{SearchParams, TimeWindow};
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// Keep nothing between requests, so each one starts with no cookies or
    /// stored state. Slower on a site that expects a session, and the right
    /// choice when one page's state would spoil the next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storageless: Option<bool>,

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
    /// The text to work from, instead of fetching anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Serve a stored copy when there is one, rather than fetching again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<bool>,
    /// Where to send progress, and which events to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhooks: Option<WebhookSettings>,

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
    /// Skip the checks that catch a combination of settings that cannot work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_config_checks: Option<bool>,
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
}
