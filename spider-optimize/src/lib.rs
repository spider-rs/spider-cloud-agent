//! A second decision layer for Spider Cloud requests.
//!
//! `spider-route` picks the settings of a first attempt from rules. This
//! crate runs after the router and the request plan, when the request is
//! about to go out. It generates valid candidate edits to the request
//! parameters ([`generate`]), scores each for success, latency and credits
//! through a [`Scorer`], and applies at most one edit set, only when it
//! passes [`validate()`] and the evidence [`Gate`]. Otherwise the request goes
//! out exactly as it was. Leaving it unchanged is always candidate zero and
//! the baseline every other candidate is measured against.
//!
//! Like the router, it is sync, does no input or output, holds no clock,
//! reads no environment, and allocates nothing while it scores. With no model
//! compiled in, [`NoModel`] abstains and every request is kept.
//!
//! # What never reaches a number
//!
//! No host, domain, address or page body reaches a feature slot, a recorded
//! row or a public numeric type. The one place a resource identifier lives is
//! [`Identifier::pattern`], which exists to be written into
//! `network_blacklist` and nowhere else.
//!
//! # The request type
//!
//! The request body is defined by the client crate, which depends on this one,
//! so this crate reads and writes it through the [`Params`] trait. The client
//! implements it for its own request type.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod candidates;
pub mod edit;
pub mod features;
pub mod gate;
pub mod labels;
pub mod model;
pub mod observe;
pub mod params;
pub mod row;
pub mod schema;
pub mod validate;

pub use candidates::{generate, Candidate, Context, MAX_CANDIDATES, MULTIPLIERS};
pub use edit::{Applied, Edit, EditError, EditSet, Op, Value, MAX_EDITS};
pub use features::{featurize_edit, EditFeatures, Input, EDIT_DIM, EDIT_FEATURE_VERSION};
pub use gate::{choose, Choice, Gate, Reason};
pub use labels::{byte_ratio, fields_ok, shingle_jaccard};
pub use model::{ModelVersion, NoModel, Score, Scorer};
pub use observe::{summarize, Identifier, Observation, ResourceSummary, MAX_IDENTIFIERS};
pub use params::Params;
pub use row::{
    comparison_row, Arm, ComparisonRow, EditDescriptor, IdentDescriptor, MemoryState,
    CMP_ROW_VERSION,
};
pub use schema::{Dependency, Group, Key, Kind, ParamSpec, Schema, SCHEMA_VERSION};
pub use validate::{validate, Rejection};

#[cfg(test)]
mod testing {
    //! The client's request type, read and written for the tests.
    //!
    //! The client crate implements [`Params`] for its own type when it takes
    //! this crate on. Until then, and because a test build of this crate is a
    //! different crate from the one the client links, the tests carry their
    //! own implementation. Presence and flags are read off the serialized
    //! body, so this cannot drift from the struct.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use crate::candidates::Context;
    use crate::observe::{summarize, Observation};
    use crate::params::Params;
    use crate::schema::Key;
    use spider_cloud_agent::params::{RequestParams, Timeout, WaitFor};
    use spider_route::{DeclaredNeed, ProxyPool, RequestMode, RouteDecision};
    use url::Url;

    /// Everything a [`Context`] borrows, owned, so a test can change one
    /// piece and borrow the rest.
    pub(crate) struct Fixture {
        pub url: Url,
        pub need: DeclaredNeed,
        pub current: RequestParams,
        pub caller: RequestParams,
        pub routed: RouteDecision,
        pub observation: Observation,
        pub cap: f32,
    }

    impl Fixture {
        /// A smart fetch, nothing pinned, nothing known.
        pub fn cold() -> Fixture {
            Fixture {
                url: Url::parse("https://example.com/articles/one").unwrap(),
                need: DeclaredNeed::Markdown,
                current: RequestParams {
                    request: Some(RequestMode::Smart),
                    ..RequestParams::default()
                },
                caller: RequestParams::default(),
                routed: RouteDecision::default(),
                observation: Observation::cold(),
                cap: 8.0,
            }
        }

        /// A smart fetch with hints off and four third party groups seen.
        pub fn observed() -> Fixture {
            let mut fixture = Fixture::cold();
            fixture.current.disable_hints = Some(true);
            let resources: Vec<(Url, u64)> = [
                ("https://example.com/app.js", 40_000),
                ("https://tracker-alpha.example/t.js", 90_000),
                ("https://cdn-beta.example/big.js", 300_000),
                ("https://ads-gamma.example/a.gif", 20_000),
                ("https://fonts-delta.example/f.woff2", 5_000),
                ("https://widget-epsilon.example/w.js", 1_000),
            ]
            .iter()
            .map(|(url, bytes)| (Url::parse(url).unwrap(), *bytes))
            .collect();
            fixture.observation.resources = Some(summarize(&fixture.url, &resources));
            fixture
        }

        /// The caller fixed the mode, the pool and a selector wait.
        pub fn pinned() -> Fixture {
            let mut fixture = Fixture::observed();
            fixture.caller.request = Some(RequestMode::Browser);
            fixture.caller.proxy = Some(ProxyPool::Isp);
            fixture.caller.wait_for = Some(WaitFor::selector("main", Timeout::from_secs(2)));
            fixture.current.request = fixture.caller.request;
            fixture.current.proxy = fixture.caller.proxy;
            fixture.current.wait_for = fixture.caller.wait_for.clone();
            fixture
        }

        pub fn ctx(&self) -> Context<'_> {
            Context {
                url: &self.url,
                need: self.need,
                current: &self.current,
                caller: &self.caller,
                routed: &self.routed,
                observation: &self.observation,
                multiplier_cap: self.cap,
            }
        }
    }

    fn body(params: &RequestParams) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::to_value(params).expect("a request serializes") {
            serde_json::Value::Object(map) => map,
            other => panic!("a request is an object, got {other}"),
        }
    }

    impl Params for RequestParams {
        fn is_set(&self, key: Key) -> bool {
            body(self).contains_key(key.wire())
        }

        fn request(&self) -> Option<RequestMode> {
            self.request
        }

        fn proxy(&self) -> Option<ProxyPool> {
            self.proxy
        }

        fn idle_wait_millis(&self) -> Option<u32> {
            let idle = self.wait_for.as_ref()?.idle_network?;
            let millis = idle.timeout.secs * 1_000 + u64::from(idle.timeout.nanos / 1_000_000);
            Some(millis.min(u64::from(u32::MAX)) as u32)
        }

        fn flag(&self, key: Key) -> Option<bool> {
            body(self)
                .get(key.wire())
                .and_then(serde_json::Value::as_bool)
        }

        fn list(&self, key: Key) -> Option<&[String]> {
            match key {
                Key::NetworkBlacklist => self.network_blacklist.as_deref(),
                Key::NetworkWhitelist => self.network_whitelist.as_deref(),
                Key::Blacklist => self.blacklist.as_deref(),
                Key::Whitelist => self.whitelist.as_deref(),
                Key::ExternalDomains => self.external_domains.as_deref(),
                _ => None,
            }
        }

        fn set_request(&mut self, mode: RequestMode) {
            self.request = Some(mode);
        }

        fn set_proxy(&mut self, pool: ProxyPool) {
            self.proxy = Some(pool);
        }

        fn set_idle_wait(&mut self, millis: u32) {
            self.wait_for = Some(WaitFor::idle_network(Timeout::from_millis(u64::from(
                millis,
            ))));
        }

        fn set_flag(&mut self, key: Key, value: bool) {
            let slot = match key {
                Key::BlockStylesheets => &mut self.block_stylesheets,
                Key::BlockAds => &mut self.block_ads,
                Key::BlockAnalytics => &mut self.block_analytics,
                Key::FullResources => &mut self.full_resources,
                Key::DisableIntercept => &mut self.disable_intercept,
                Key::DisableHints => &mut self.disable_hints,
                _ => return,
            };
            *slot = Some(value);
        }

        fn set_list(&mut self, key: Key, list: Option<Vec<String>>) {
            if key == Key::NetworkBlacklist {
                self.network_blacklist = list;
            }
        }
    }
}
