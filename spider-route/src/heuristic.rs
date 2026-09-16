//! The rules, which are what ships until weights exist.
//!
//! This is not a placeholder. It is the baseline any model has to beat before
//! it is worth publishing, and it is the whole router for every build with no
//! weights compiled in. So each rule below is written down with the reason it
//! fires, and each one is tested on its own.
//!
//! Two things the rules deliberately do not do. They hold no per site
//! knowledge: there is no list of sites that need anything, because such a
//! list is exactly what this crate promises never to ship. And they never
//! raise the settings under a rate limit, because heavier settings under a
//! lockout make the lockout worse. Both are constraints a model does not get
//! to learn its way around.
//!
//! What the rules read is the shape of the request and what the caller
//! remembers. The host contributes only what [`crate::domain`] hands out,
//! which is a label group and a few flags.

use crate::action::{ProxyPool, RequestMode};
use crate::decision::{Action, RouteDecision, RouteSource, Wait};
use crate::domain::host_shape;
use crate::features::{extension_of, DeclaredNeed, RouteInput, SiteMemory, StatusClass};
use crate::router::{Router, RouterVersion};

/// How long a settling wait allows before the page is read anyway.
///
/// Ten seconds is the span the client's own escalation uses, and the two
/// should not disagree about what waiting means.
const SETTLE_MILLIS: u32 = 10_000;

/// Which rule decided.
///
/// Returned by [`HeuristicRouter::explain`] so a decision can be traced
/// without reading this file, and so the tests can name the rule they are
/// checking rather than inferring it from the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rule {
    /// The caller fixed the settings, so there was nothing to decide.
    CallerPinned,
    /// The site asked for a slower pace last time, so nothing is raised.
    RateLimitRestraint,
    /// The path names a file whose bytes are fixed before any script could
    /// run.
    FixedBytes,
    /// The caller wants a picture, which only a rendered fetch can produce.
    PictureNeeded,
    /// The site has turned this caller away more than once.
    RepeatedRefusal,
    /// The last attempt came back with nothing in it.
    CameBackEmpty,
    /// The site has failed this caller repeatedly, without refusing outright.
    RepeatedFailure,
    /// The caller wants links or metadata, neither of which needs a render.
    ShallowNeed,
    /// The host is a literal address, which is an interface far more often
    /// than it is a page.
    LiteralAddress,
    /// The site has been answering, so the cheap default stands and the router
    /// is surer of it.
    SteadySite,
    /// Nothing is known, which is the normal case.
    ColdStart,
}

/// The rules router.
///
/// Holds no state, so one can be shared anywhere. It is also why
/// [`Router::observe`] is left as the default: state about a site lives with
/// the caller, which is the only place it can live without a site name
/// reaching this crate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct HeuristicRouter;

impl HeuristicRouter {
    /// The rules as shipped.
    pub const fn new() -> HeuristicRouter {
        HeuristicRouter
    }

    /// Decide, and say which rule decided.
    ///
    /// The rules are tried in the order below and the first one that fires
    /// wins, so the order is part of the behaviour. Safety constraints come
    /// first, then facts that settle the question outright, then what the
    /// caller remembers, then what the caller asked for.
    pub fn explain(&self, input: &RouteInput<'_>) -> (RouteDecision, Rule) {
        let memory = input.memory.copied().unwrap_or_else(SiteMemory::cold);

        // The caller's own settings are not a suggestion. Nothing below runs.
        if input.pins.any() {
            let action = Action {
                mode: input.pins.mode.unwrap_or_default(),
                proxy: input.pins.proxy.unwrap_or_default(),
                wait: Wait::Now,
            };

            return (
                RouteDecision::new(action, RouteSource::Caller, 1.0)
                    .with_country(input.pins.country.cloned()),
                Rule::CallerPinned,
            );
        }

        // A site that asked for a slower pace gets one. Rendering more of it,
        // or arriving from a dearer pool, spends more of a quota that is
        // already the problem and lengthens the lockout. This outranks every
        // rule below, including the memory rules that would otherwise raise
        // the settings on a run of failures.
        if memory.last_status == StatusClass::RateLimited {
            return (
                RouteDecision::new(Action::new(RequestMode::Http), RouteSource::Memory, 0.6),
                Rule::RateLimitRestraint,
            );
        }

        // A feed, a data file, an image or an archive is the same bytes
        // whichever way it is fetched, because there was never any script to
        // run. Paying for a render here buys nothing, and this is the single
        // most reliable rule in the file.
        if !extension_of(input.url).can_need_rendering() {
            return (
                RouteDecision::new(Action::new(RequestMode::Http), RouteSource::Heuristic, 0.95),
                Rule::FixedBytes,
            );
        }

        // A picture of a page can only come from drawing the page.
        if input.need == DeclaredNeed::Screenshot {
            return (
                RouteDecision::new(
                    Action::new(RequestMode::Browser),
                    RouteSource::Heuristic,
                    1.0,
                ),
                Rule::PictureNeeded,
            );
        }

        // Turned away more than once. A refusal is about who is asking rather
        // than about how the page is built, so the pool is what changes, and
        // the escalation restarts past the steps that only change the fetch.
        if memory.is_informative()
            && memory.failure_streak() >= 2
            && memory.last_status.is_refusal()
        {
            let action = Action::new(RequestMode::Browser)
                .with_proxy(ProxyPool::Residential)
                .with_wait(Wait::Settled {
                    millis: SETTLE_MILLIS,
                });

            return (
                RouteDecision::new(action, RouteSource::Memory, 0.8).starting_at(3),
                Rule::RepeatedRefusal,
            );
        }

        // Nothing in the body last time. That is what a page whose content
        // arrives after load looks like from outside, so render it and give it
        // time to settle rather than fetching the same shell again.
        if memory.observations >= 1 && memory.last_status == StatusClass::Empty {
            let action = Action::new(RequestMode::Browser).with_wait(Wait::Settled {
                millis: SETTLE_MILLIS,
            });

            return (
                RouteDecision::new(action, RouteSource::Memory, 0.75).starting_at(2),
                Rule::CameBackEmpty,
            );
        }

        // Failing repeatedly without an outright refusal. Start rendered, and
        // let the escalation pick up past the steps that have already been
        // paid for and did not work.
        if memory.is_informative()
            && (memory.failure_streak() >= 2
                || (memory.observations >= 3 && memory.success_rate < 0.34))
        {
            return (
                RouteDecision::new(Action::new(RequestMode::Browser), RouteSource::Memory, 0.7)
                    .starting_at(1),
                Rule::RepeatedFailure,
            );
        }

        // Links and metadata are markup, and markup arrives with the first
        // response. A caller that asked for either is not asking for anything
        // a render adds, so do not price one in.
        if matches!(input.need, DeclaredNeed::Links | DeclaredNeed::Metadata) {
            return (
                RouteDecision::new(Action::new(RequestMode::Http), RouteSource::Heuristic, 0.7),
                Rule::ShallowNeed,
            );
        }

        // A literal address is an api, an appliance or something on a desk far
        // more often than it is a page built in the browser, and it is never a
        // site with an anti bot vendor in front of it.
        if host_shape(input.url).ip.is_some() {
            return (
                RouteDecision::new(Action::new(RequestMode::Http), RouteSource::Heuristic, 0.8),
                Rule::LiteralAddress,
            );
        }

        // Answering steadily. The default already costs the least of the
        // modes that can recover, so the rule changes nothing except how sure
        // the router says it is, which is what a confidence floor reads.
        if memory.observations >= 3 && memory.success_rate >= 0.8 {
            return (
                RouteDecision::new(Action::new(RequestMode::Smart), RouteSource::Memory, 0.8),
                Rule::SteadySite,
            );
        }

        // Nothing known. The default mode starts cheap and changes course on
        // its own when the first response says it has to, which is the right
        // bet when the router has nothing to go on. By construction this is
        // the common case: no feature depends on which site it is, so a site
        // never seen before is handled exactly like one seen a thousand times.
        (
            RouteDecision::new(Action::new(RequestMode::Smart), RouteSource::Heuristic, 0.5),
            Rule::ColdStart,
        )
    }
}

impl Router for HeuristicRouter {
    fn route(&self, input: &RouteInput<'_>) -> RouteDecision {
        self.explain(input).0
    }

    fn version(&self) -> RouterVersion {
        RouterVersion::Heuristic
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
    use crate::action::Country;
    use crate::features::CallerPins;
    use url::Url;

    fn decide(url: &str) -> (RouteDecision, Rule) {
        let parsed = Url::parse(url).unwrap();
        HeuristicRouter::new().explain(&RouteInput::new(&parsed, DeclaredNeed::Markdown))
    }

    fn decide_with(url: &str, need: DeclaredNeed, memory: SiteMemory) -> (RouteDecision, Rule) {
        let parsed = Url::parse(url).unwrap();
        HeuristicRouter::new().explain(&RouteInput::new(&parsed, need).with_memory(&memory))
    }

    fn failing(count: u32, streak: i16, status: StatusClass) -> SiteMemory {
        SiteMemory {
            observations: count,
            success_rate: 0.0,
            streak,
            last_status: status,
        }
    }

    #[test]
    fn nothing_known_starts_cheap_and_says_so() {
        let (decision, rule) = decide("https://www.example.com/pricing");

        assert_eq!(rule, Rule::ColdStart);
        assert_eq!(decision.mode(), RequestMode::Smart);
        assert_eq!(decision.proxy(), ProxyPool::Isp);
        assert_eq!(decision.wait(), Wait::Now);
        assert_eq!(decision.start_rung, 0);
        assert_eq!(decision.source, RouteSource::Heuristic);
        assert!(decision.confidence < 0.6);
    }

    #[test]
    fn a_file_that_was_never_markup_takes_the_cheap_path() {
        for url in [
            "https://example.com/feed.xml",
            "https://example.com/api/items.json",
            "https://example.com/report.pdf",
            "https://example.com/hero.png",
            "https://example.com/data/rows.csv",
            "https://example.com/dump.tar",
        ] {
            let (decision, rule) = decide(url);
            assert_eq!(rule, Rule::FixedBytes, "{url}");
            assert_eq!(decision.mode(), RequestMode::Http, "{url}");
            assert!(decision.confidence > 0.9, "{url}");
        }

        // The counterpart: markup and unknown paths are not settled by the
        // extension, or the rule would swallow every page on the web.
        for url in [
            "https://example.com/index.html",
            "https://example.com/pricing",
            "https://example.com/thing.wobble",
        ] {
            assert_ne!(decide(url).1, Rule::FixedBytes, "{url}");
        }
    }

    #[test]
    fn a_picture_has_to_be_drawn() {
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Screenshot,
            SiteMemory::cold(),
        );

        assert_eq!(rule, Rule::PictureNeeded);
        assert_eq!(decision.mode(), RequestMode::Browser);
        assert_eq!(decision.confidence, 1.0);
    }

    #[test]
    fn links_and_metadata_do_not_pay_for_a_render() {
        for need in [DeclaredNeed::Links, DeclaredNeed::Metadata] {
            let (decision, rule) =
                decide_with("https://example.com/pricing", need, SiteMemory::cold());

            assert_eq!(rule, Rule::ShallowNeed, "{need:?}");
            assert_eq!(decision.mode(), RequestMode::Http, "{need:?}");
        }

        // And the needs that do want content are not swept up with them.
        for need in [
            DeclaredNeed::Markdown,
            DeclaredNeed::Text,
            DeclaredNeed::Html,
        ] {
            assert_ne!(
                decide_with("https://example.com/pricing", need, SiteMemory::cold()).1,
                Rule::ShallowNeed,
                "{need:?}"
            );
        }
    }

    #[test]
    fn being_turned_away_twice_changes_the_pool_and_skips_ahead() {
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            failing(6, -2, StatusClass::Blocked),
        );

        assert_eq!(rule, Rule::RepeatedRefusal);
        assert_eq!(decision.proxy(), ProxyPool::Residential);
        assert_eq!(decision.mode(), RequestMode::Browser);
        assert_eq!(
            decision.wait(),
            Wait::Settled {
                millis: SETTLE_MILLIS
            }
        );
        assert_eq!(decision.start_rung, 3);
        assert_eq!(decision.source, RouteSource::Memory);
    }

    #[test]
    fn one_refusal_is_an_anecdote() {
        // The boundary, from both sides. A caller with two attempts recorded
        // and one refusal has told us enough to be worth reading and not
        // enough to spend the dearer pool on, so the count is what decides.
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            failing(2, -1, StatusClass::Blocked),
        );

        assert_eq!(rule, Rule::ColdStart);
        assert_eq!(decision.proxy(), ProxyPool::Isp);

        // One more failure and the same memory does spend it.
        assert_eq!(
            decide_with(
                "https://example.com/pricing",
                DeclaredNeed::Markdown,
                failing(2, -2, StatusClass::Blocked),
            )
            .1,
            Rule::RepeatedRefusal
        );

        // A single recorded attempt is not read at all, whatever it says.
        assert_eq!(
            decide_with(
                "https://example.com/pricing",
                DeclaredNeed::Markdown,
                failing(1, -1, StatusClass::Blocked),
            )
            .1,
            Rule::ColdStart
        );
    }

    #[test]
    fn records_that_contradict_themselves_are_not_acted_on() {
        // One recorded attempt and a run of three failures cannot both be
        // true. The count is the thing that says how much history there is,
        // so it is the count that decides, and a caller with a bug in its own
        // bookkeeping does not get charged for the dearer pool.
        let contradictory = SiteMemory {
            observations: 1,
            success_rate: 0.0,
            streak: -3,
            last_status: StatusClass::Blocked,
        };
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            contradictory,
        );

        assert_eq!(rule, Rule::ColdStart);
        assert_eq!(decision.proxy(), ProxyPool::Isp);

        let consistent = SiteMemory {
            observations: 3,
            ..contradictory
        };
        assert_eq!(
            decide_with(
                "https://example.com/pricing",
                DeclaredNeed::Markdown,
                consistent
            )
            .1,
            Rule::RepeatedRefusal
        );
    }

    #[test]
    fn an_empty_body_is_treated_as_a_page_that_builds_itself() {
        let memory = SiteMemory {
            observations: 1,
            success_rate: 0.0,
            streak: -1,
            last_status: StatusClass::Empty,
        };
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            memory,
        );

        assert_eq!(rule, Rule::CameBackEmpty);
        assert_eq!(decision.mode(), RequestMode::Browser);
        assert_eq!(
            decision.wait(),
            Wait::Settled {
                millis: SETTLE_MILLIS
            }
        );
        assert_eq!(decision.proxy(), ProxyPool::Isp);
        assert_eq!(decision.start_rung, 2);
    }

    #[test]
    fn repeated_failure_without_a_refusal_starts_rendered() {
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            failing(9, -4, StatusClass::ServerError),
        );

        assert_eq!(rule, Rule::RepeatedFailure);
        assert_eq!(decision.mode(), RequestMode::Browser);
        assert_eq!(decision.proxy(), ProxyPool::Isp);
        assert_eq!(decision.start_rung, 1);
    }

    #[test]
    fn a_poor_success_rate_counts_even_without_a_streak() {
        let memory = SiteMemory {
            observations: 20,
            success_rate: 0.2,
            streak: 1,
            last_status: StatusClass::Ok,
        };

        assert_eq!(
            decide_with(
                "https://example.com/pricing",
                DeclaredNeed::Markdown,
                memory
            )
            .1,
            Rule::RepeatedFailure
        );
    }

    #[test]
    fn a_rate_limit_never_raises_the_settings() {
        // The constraint that outranks the memory rules. Everything in this
        // memory says escalate, and the answer is still the cheapest mode.
        let memory = SiteMemory {
            observations: 30,
            success_rate: 0.0,
            streak: -9,
            last_status: StatusClass::RateLimited,
        };
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            memory,
        );

        assert_eq!(rule, Rule::RateLimitRestraint);
        assert_eq!(decision.mode(), RequestMode::Http);
        assert_eq!(decision.proxy(), ProxyPool::Isp);
        assert_eq!(decision.wait(), Wait::Now);
        assert_eq!(decision.start_rung, 0);

        // And the same memory with any other ending does escalate, so the
        // check above is about the rate limit and not about the rest of it.
        let refused = SiteMemory {
            last_status: StatusClass::Blocked,
            ..memory
        };
        assert_eq!(
            decide_with(
                "https://example.com/pricing",
                DeclaredNeed::Markdown,
                refused
            )
            .1,
            Rule::RepeatedRefusal
        );
    }

    #[test]
    fn a_literal_address_is_treated_as_an_interface() {
        let (decision, rule) = decide("http://198.51.100.4:8080/status");

        assert_eq!(rule, Rule::LiteralAddress);
        assert_eq!(decision.mode(), RequestMode::Http);

        assert_eq!(
            decide("http://[2001:db8::1]/status").1,
            Rule::LiteralAddress
        );
        assert_ne!(
            decide("http://example.com:8080/status").1,
            Rule::LiteralAddress
        );
    }

    #[test]
    fn a_site_that_answers_is_left_alone() {
        let memory = SiteMemory {
            observations: 40,
            success_rate: 0.95,
            streak: 12,
            last_status: StatusClass::Ok,
        };
        let (decision, rule) = decide_with(
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            memory,
        );

        assert_eq!(rule, Rule::SteadySite);
        assert_eq!(decision.mode(), RequestMode::Smart);
        assert!(decision.confidence > decide("https://example.com/pricing").0.confidence);
    }

    #[test]
    fn a_pin_is_obeyed_and_nothing_else_runs() {
        let url = Url::parse("https://example.com/feed.xml").unwrap();
        let country = Country::new("de").unwrap();
        let memory = failing(6, -4, StatusClass::Blocked);
        let pins = CallerPins {
            mode: Some(RequestMode::Browser),
            proxy: Some(ProxyPool::Residential),
            country: Some(&country),
        };
        let input = RouteInput::new(&url, DeclaredNeed::Markdown)
            .with_pins(pins)
            .with_memory(&memory);
        let (decision, rule) = HeuristicRouter::new().explain(&input);

        // Two rules below would have answered differently, and neither ran.
        assert_eq!(rule, Rule::CallerPinned);
        assert_eq!(decision.mode(), RequestMode::Browser);
        assert_eq!(decision.proxy(), ProxyPool::Residential);
        assert_eq!(decision.country.as_ref().map(Country::as_str), Some("de"));
        assert_eq!(decision.source, RouteSource::Caller);
        assert_eq!(decision.confidence, 1.0);
    }

    #[test]
    fn a_partial_pin_keeps_the_defaults_for_the_rest() {
        let url = Url::parse("https://example.com/pricing").unwrap();
        let pins = CallerPins {
            proxy: Some(ProxyPool::Residential),
            ..CallerPins::none()
        };
        let input = RouteInput::new(&url, DeclaredNeed::Markdown).with_pins(pins);
        let (decision, rule) = HeuristicRouter::new().explain(&input);

        assert_eq!(rule, Rule::CallerPinned);
        assert_eq!(decision.proxy(), ProxyPool::Residential);
        assert_eq!(decision.mode(), RequestMode::Smart);
        assert_eq!(decision.country, None);
    }

    #[test]
    fn the_rules_answer_with_no_weights_anywhere_in_the_build() {
        // The no model path is the only path this crate has today, and it has
        // to stay a path rather than a fallback. If weights ever become a
        // default feature, this test keeps answering from rules alone.
        let router = HeuristicRouter::new();
        let url = Url::parse("https://www.example.com/a/b/c").unwrap();

        assert_eq!(router.version(), RouterVersion::Heuristic);
        assert_eq!(
            router
                .route(&RouteInput::new(&url, DeclaredNeed::Markdown))
                .source,
            RouteSource::Heuristic
        );
    }

    #[test]
    fn every_rule_is_reachable() {
        // A rule nobody can reach is a rule nobody tests. This lists what the
        // cases above produced, so adding a rule without a case fails here.
        let url = Url::parse("https://example.com/pricing").unwrap();
        let country = Country::new("us").unwrap();
        let pinned = RouteInput::new(&url, DeclaredNeed::Markdown).with_pins(CallerPins {
            country: Some(&country),
            ..CallerPins::none()
        });

        let mut seen = vec![
            HeuristicRouter::new().explain(&pinned).1,
            decide("https://example.com/feed.xml").1,
            decide("http://203.0.113.1/status").1,
            decide("https://example.com/pricing").1,
            decide_with(
                "https://example.com/p",
                DeclaredNeed::Screenshot,
                SiteMemory::cold(),
            )
            .1,
            decide_with(
                "https://example.com/p",
                DeclaredNeed::Links,
                SiteMemory::cold(),
            )
            .1,
            decide_with(
                "https://example.com/p",
                DeclaredNeed::Markdown,
                failing(30, -9, StatusClass::RateLimited),
            )
            .1,
            decide_with(
                "https://example.com/p",
                DeclaredNeed::Markdown,
                failing(6, -2, StatusClass::Blocked),
            )
            .1,
            decide_with(
                "https://example.com/p",
                DeclaredNeed::Markdown,
                failing(1, -1, StatusClass::Empty),
            )
            .1,
            decide_with(
                "https://example.com/p",
                DeclaredNeed::Markdown,
                failing(9, -4, StatusClass::ServerError),
            )
            .1,
            decide_with(
                "https://example.com/p",
                DeclaredNeed::Markdown,
                SiteMemory {
                    observations: 40,
                    success_rate: 0.95,
                    streak: 12,
                    last_status: StatusClass::Ok,
                },
            )
            .1,
        ];

        seen.sort_by_key(|rule| format!("{rule:?}"));
        seen.dedup();

        assert_eq!(seen.len(), 11, "distinct rules reached: {seen:?}");
    }
}
