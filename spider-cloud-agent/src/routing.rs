//! Where the router meets the request.
//!
//! The router answers in its own vocabulary, which is deliberately narrower
//! than the request parameters. This module is the translation both ways: what
//! the caller asked for becomes a [`RouteInput`], and the answer becomes
//! settings on the request.
//!
//! One rule governs the whole file. The caller beats the router, and the router
//! beats the default. A setting the caller made is never argued with, which is
//! the same rule the thrift plan already follows and is implemented the same
//! way, by comparing against a snapshot of the request as the caller left it.

use spider_route::{ProxyPool, RequestMode};

// The router's vocabulary is defined once, in spider-route, because the router
// cannot depend on this crate. Re-exported here so writing a [`Router`] needs
// nothing in the caller's manifest but this crate.
pub use spider_route::{
    featurize, Action, AttemptOutcome, CallerPins, DeclaredNeed, ExtClass, FeatureVector,
    HeuristicRouter, KeyClass, RouteDecision, RouteInput, RouteSource, Router, RouterVersion, Rule,
    SiteMemory, StatusClass, Wait, FEATURES_USED, FEATURE_DIM,
};
use url::Url;

use crate::credits::Credits;
use crate::params::js::Timeout;
use crate::params::{RequestParams, WaitFor};
use crate::policy::budget::Budget;
use crate::policy::engine::{Observed, Reached};
use crate::status::{ApiClass, PageClass};
use crate::thrift::Need;

/// What the caller wants back, in the terms the router reads.
pub(crate) fn declared_need(need: Option<&Need>) -> DeclaredNeed {
    match need {
        Some(Need::Text) => DeclaredNeed::Text,
        Some(Need::Markdown) => DeclaredNeed::Markdown,
        Some(Need::Html) => DeclaredNeed::Html,
        Some(Need::Links) => DeclaredNeed::Links,
        Some(Need::Metadata) => DeclaredNeed::Metadata,
        Some(Need::Fields(_)) => DeclaredNeed::Fields,
        Some(Need::Screenshot(_)) => DeclaredNeed::Screenshot,
        // A caller who stated no need has opted out of this layer as surely as
        // one who asked for the service's own answer.
        Some(Need::Raw) | None => DeclaredNeed::Raw,
    }
}

/// The settings the caller fixed on the request itself.
///
/// Read off the caller's own snapshot, so a setting the escalation ladder or
/// the plan wrote later cannot be mistaken for one the caller chose.
pub(crate) fn pins_from(caller: &RequestParams) -> CallerPins<'_> {
    CallerPins {
        mode: caller.request,
        proxy: caller.proxy,
        country: caller.country_code.as_ref(),
    }
}

/// Write a decision onto a request, leaving every setting the caller made.
///
/// A decision that names the mode the caller already named changes nothing,
/// which is what makes the precedence rule readable here rather than a matter
/// of what runs first.
pub(crate) fn apply_decision(
    decision: &RouteDecision,
    params: &mut RequestParams,
    caller: &RequestParams,
) {
    if caller.request.is_none() {
        params.request = Some(decision.mode());
    }
    if caller.proxy.is_none() {
        params.proxy = Some(decision.proxy());
    }
    if caller.country_code.is_none() {
        if let Some(country) = &decision.country {
            params.country_code = Some(country.clone());
        }
    }
    if caller.wait_for.is_none() {
        if let Wait::Settled { millis } = decision.wait() {
            params.wait_for = Some(WaitFor::idle_network(Timeout::from_millis(u64::from(
                millis,
            ))));
        }
    }
}

/// How one attempt ended, in the terms the router and the site memory read.
///
/// `accepted` is the policy's own verdict rather than a second reading of the
/// status codes. The policy already decided whether this attempt got what the
/// caller asked for, and two answers to that question in one crate would drift
/// apart.
pub(crate) fn outcome_of(observed: &Observed, accepted: bool, bytes: u32) -> AttemptOutcome {
    let millis = observed.elapsed.as_millis().min(u128::from(u32::MAX)) as u32;
    let mut out = AttemptOutcome::ok(bytes, millis);
    out.success = accepted;
    out.status = status_class(observed);
    out
}

/// Whether an attempt says anything about the site it was aimed at.
///
/// A rejected key, an empty balance or a malformed request are facts about this
/// account and this call. Folding them into a site's record would teach the
/// router that a site is hard when the only hard thing was the key.
pub(crate) fn describes_the_site(observed: &Observed) -> bool {
    observed.page.is_some() || observed.is_blank_success()
}

/// The class an attempt falls into.
fn status_class(observed: &Observed) -> StatusClass {
    if observed.is_blank_success() {
        return StatusClass::Empty;
    }

    match observed.page_class() {
        Some(PageClass::Ok) => StatusClass::Ok,
        Some(PageClass::BadRequest) => StatusClass::BadRequest,
        Some(PageClass::NeedsLogin) => StatusClass::NeedsLogin,
        Some(PageClass::Blocked) => StatusClass::Blocked,
        Some(PageClass::NotFound) => StatusClass::NotFound,
        Some(PageClass::TargetRateLimited) => StatusClass::RateLimited,
        Some(PageClass::ServerError) => StatusClass::ServerError,
        Some(_) => StatusClass::Unknown,
        None => api_class(observed),
    }
}

/// The class a call that never reached a site falls into.
fn api_class(observed: &Observed) -> StatusClass {
    match observed.api {
        Reached::TimedOut | Reached::ConnectFailed => StatusClass::ServerError,
        Reached::Api(_) => match observed.api_class() {
            Some(ApiClass::Ok) | Some(ApiClass::NoContent) => StatusClass::Ok,
            Some(ApiClass::RateLimited { .. }) => StatusClass::RateLimited,
            Some(ApiClass::Unauthorized) => StatusClass::NeedsLogin,
            Some(ApiClass::ServerError) | Some(ApiClass::Draining { .. }) => {
                StatusClass::ServerError
            }
            Some(_) => StatusClass::BadRequest,
            None => StatusClass::Unknown,
        },
    }
}

/// One request, as the router sees it.
pub(crate) fn input<'a>(
    url: &'a Url,
    need: DeclaredNeed,
    pins: CallerPins<'a>,
    memory: Option<&'a SiteMemory>,
) -> RouteInput<'a> {
    let base = RouteInput::new(url, need).with_pins(pins);
    match memory {
        Some(memory) => base.with_memory(memory),
        None => base,
    }
}

/// The actions exploration may reach for, and what each is expected to cost as
/// a multiple of a plain fetch of the same page.
///
/// The same reduced lattice the router chooses from, and the multipliers are
/// the ladder's own, so an arm that would cost more than the caller allows is
/// refused on the same arithmetic an escalation is refused on.
const ARMS: [(RequestMode, ProxyPool, u32, f32); 6] = [
    (RequestMode::Http, ProxyPool::Isp, 0, 1.0),
    (RequestMode::Smart, ProxyPool::Isp, 0, 1.5),
    (RequestMode::Browser, ProxyPool::Isp, 0, 4.0),
    (RequestMode::Browser, ProxyPool::Isp, SETTLE_MILLIS, 5.0),
    (RequestMode::Smart, ProxyPool::Residential, 0, 3.0),
    (
        RequestMode::Browser,
        ProxyPool::Residential,
        SETTLE_MILLIS,
        8.0,
    ),
];

/// How long an exploring arm gives a page to settle, matching the ladder.
const SETTLE_MILLIS: u32 = 10_000;

/// How finely the exploration rate is read, which is one call in a million.
const RESOLUTION: u64 = 1_000_000;

/// Picks a different action on a fraction of calls, so that something is
/// learned about the arms nobody pulls.
///
/// # The two limits, and why they are limits
///
/// Exploration never fires on a retry. An escalation is already a considered
/// move away from what did not work, and taking a random action in the middle
/// of one would make the attempt trail unreadable and the row it produced
/// unattributable. Only the first attempt of an operation is ever explored,
/// and that is enforced by where this is called from rather than by a flag.
///
/// Exploration never costs more than the budget allows. An arm is eligible only
/// when its estimated spend fits inside the caller's credit caps, on the same
/// arithmetic the escalation ladder is held to. A caller who capped an
/// operation at a cheap fetch gets a cheap fetch, explored or not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Explorer {
    /// The fraction of calls that take a different action, from zero to one.
    pub rate: f32,
    /// The seed the choice is drawn from.
    pub seed: u64,
}

/// The seed used when nobody names one.
///
/// Fixed rather than drawn from the clock, so two processes with the same
/// settings explore the same pages and a test can assert what happens.
pub const DEFAULT_SEED: u64 = 0x5370_6964_6572_3031;

impl Default for Explorer {
    fn default() -> Explorer {
        Explorer {
            rate: 0.0,
            seed: DEFAULT_SEED,
        }
    }
}

impl Explorer {
    /// An explorer that fires on this fraction of calls.
    pub fn new(rate: f32) -> Explorer {
        Explorer {
            rate: rate.clamp(0.0, 1.0),
            ..Explorer::default()
        }
    }

    /// The same explorer drawing from another seed.
    pub fn with_seed(mut self, seed: u64) -> Explorer {
        self.seed = seed;
        self
    }

    /// Whether this address is one of the ones explored.
    ///
    /// A function of the address and the seed only, so the same page is
    /// explored on every call rather than a different one each time. Which
    /// pages get explored is what a rate of a tenth means: a tenth of the
    /// pages, every time, not a tenth of the calls to each page.
    pub fn fires(&self, url: &Url) -> bool {
        if self.rate <= 0.0 {
            return false;
        }
        if self.rate >= 1.0 {
            return true;
        }

        let draw = (self.hash(url) >> 32) % RESOLUTION;
        draw < (f64::from(self.rate) * RESOLUTION as f64) as u64
    }

    /// A different action to try, or `None` when this call is not explored and
    /// when nothing else is both different and affordable.
    pub fn choose(
        &self,
        url: &Url,
        routed: &RouteDecision,
        budget: &Budget,
    ) -> Option<RouteDecision> {
        if !self.fires(url) {
            return None;
        }

        let eligible: Vec<Action> = ARMS
            .iter()
            .filter(|(_, _, _, multiplier)| affordable(budget, *multiplier))
            .map(|(mode, proxy, millis, _)| arm(*mode, *proxy, *millis))
            .filter(|action| *action != routed.action)
            .collect();

        let pick = eligible.get((self.hash(url) % eligible.len().max(1) as u64) as usize)?;

        // The country the router pinned is carried over: exploring the settings
        // is not a reason to change where the request appears to come from.
        //
        // The escalation starts from the top rather than from where the router
        // would have started it, because the router's starting point was
        // reasoning about the action it chose and this is not that action.
        Some(
            RouteDecision::new(*pick, RouteSource::Explore, routed.confidence)
                .with_country(routed.country.clone()),
        )
    }

    /// A number drawn from the seed and the address.
    fn hash(&self, url: &Url) -> u64 {
        let mut hash = self.seed ^ 0xcbf2_9ce4_8422_2325;
        for byte in url.as_str().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // One more round, so two addresses differing in the last byte alone do
        // not land next to each other in the draw.
        hash ^= hash >> 33;
        hash.wrapping_mul(0xff51_afd7_ed55_8ccd)
    }
}

/// One arm as an action.
fn arm(mode: RequestMode, proxy: ProxyPool, millis: u32) -> Action {
    let wait = if millis == 0 {
        Wait::Now
    } else {
        Wait::Settled { millis }
    };

    Action::new(mode).with_proxy(proxy).with_wait(wait)
}

/// Whether an arm at this multiple of a plain fetch fits inside the caps.
///
/// The basis is [`Budget::floor`], the same assumed minimum an escalation is
/// priced from before anything has been billed. Nothing has been spent yet when
/// this is asked, because only a first attempt is explored.
fn affordable(budget: &Budget, multiplier: f32) -> bool {
    let estimate = Credits(Budget::floor(Credits::ZERO).get() * f64::from(multiplier));

    // Both caps are checked here: `allows_spend` refuses an estimate over the
    // per page cap as well as one that would run the whole operation past its
    // total.
    budget.allows_spend(Credits::ZERO, estimate).is_ok()
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
    use crate::params::Country;
    use crate::policy::budget::ASSUMED_MINIMUM_COST;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("a url")
    }

    fn routed() -> RouteDecision {
        RouteDecision::new(Action::new(RequestMode::Smart), RouteSource::Heuristic, 0.5)
    }

    #[test]
    fn a_decision_fills_in_what_the_caller_left_alone() {
        let caller = RequestParams::default();
        let mut params = caller.clone();
        let decision = RouteDecision::new(
            Action::new(RequestMode::Browser)
                .with_proxy(ProxyPool::Residential)
                .with_wait(Wait::Settled { millis: 10_000 }),
            RouteSource::Memory,
            0.8,
        )
        .with_country(Country::new("de"));

        apply_decision(&decision, &mut params, &caller);

        assert_eq!(params.request, Some(RequestMode::Browser));
        assert_eq!(params.proxy, Some(ProxyPool::Residential));
        assert_eq!(
            params.country_code.as_ref().map(Country::as_str),
            Some("de")
        );
        assert!(params.wait_for.is_some());
    }

    #[test]
    fn a_decision_never_argues_with_the_caller() {
        let caller = RequestParams {
            request: Some(RequestMode::Http),
            proxy: Some(ProxyPool::Isp),
            country_code: Country::new("fr"),
            wait_for: Some(WaitFor::idle_network(Timeout::from_millis(1))),
            ..RequestParams::default()
        };

        let mut params = caller.clone();
        let decision = RouteDecision::new(
            Action::new(RequestMode::Browser)
                .with_proxy(ProxyPool::Residential)
                .with_wait(Wait::Settled { millis: 10_000 }),
            RouteSource::Memory,
            0.8,
        )
        .with_country(Country::new("de"));

        apply_decision(&decision, &mut params, &caller);

        assert_eq!(params, caller, "the router overwrote a caller's setting");
    }

    #[test]
    fn pins_are_read_off_the_callers_own_request() {
        let mut caller = RequestParams::default();
        assert!(!pins_from(&caller).any());

        caller.request = Some(RequestMode::Browser);
        let pins = pins_from(&caller);
        assert_eq!(pins.mode, Some(RequestMode::Browser));
        assert!(pins.any());
    }

    #[test]
    fn a_need_becomes_the_one_the_router_reads() {
        assert_eq!(declared_need(Some(&Need::Markdown)), DeclaredNeed::Markdown);
        assert_eq!(declared_need(Some(&Need::Links)), DeclaredNeed::Links);
        assert_eq!(
            declared_need(Some(&Need::screenshot())),
            DeclaredNeed::Screenshot
        );
        assert_eq!(declared_need(None), DeclaredNeed::Raw);
    }

    #[test]
    fn an_attempt_becomes_the_outcome_the_memory_reads() {
        let refused = Observed::seen(200, Some(403));
        assert_eq!(status_class(&refused), StatusClass::Blocked);
        assert!(describes_the_site(&refused));

        let blank = Observed::seen(200, Some(200)).blank();
        assert_eq!(status_class(&blank), StatusClass::Empty);

        let bad_key = Observed::seen(401, None);
        assert_eq!(status_class(&bad_key), StatusClass::NeedsLogin);
        assert!(
            !describes_the_site(&bad_key),
            "a rejected key is not a fact about the site"
        );

        let served = Observed::seen(200, Some(200)).taking(std::time::Duration::from_millis(250));
        let recorded = outcome_of(&served, true, 4_096);
        assert!(recorded.success);
        assert_eq!(recorded.status, StatusClass::Ok);
        assert_eq!(recorded.bytes, 4_096);
        assert_eq!(recorded.millis, 250);
    }

    #[test]
    fn nothing_is_explored_at_the_default_rate() {
        let explorer = Explorer::default();
        assert_eq!(explorer.rate, 0.0);

        for n in 0..500 {
            let page = url(&format!("https://example.com/{n}"));
            assert!(explorer
                .choose(&page, &routed(), &Budget::unlimited())
                .is_none());
        }
    }

    #[test]
    fn a_rate_is_the_fraction_of_pages_that_explore() {
        let explorer = Explorer::new(0.2);
        let fired = (0..2_000)
            .filter(|n| explorer.fires(&url(&format!("https://example.com/page/{n}"))))
            .count();

        assert!(
            (300..500).contains(&fired),
            "{fired} of 2000 pages explored at a rate of a fifth"
        );
    }

    #[test]
    fn one_page_explores_the_same_way_every_time() {
        let explorer = Explorer::new(0.5);
        let page = url("https://example.com/a/b/c");
        let first = explorer.choose(&page, &routed(), &Budget::unlimited());

        for _ in 0..50 {
            assert_eq!(
                explorer.choose(&page, &routed(), &Budget::unlimited()),
                first
            );
        }
    }

    #[test]
    fn another_seed_explores_other_pages() {
        let one = Explorer::new(0.3);
        let other = Explorer::new(0.3).with_seed(99);
        let pages: Vec<Url> = (0..400)
            .map(|n| url(&format!("https://example.com/p/{n}")))
            .collect();

        let differences = pages
            .iter()
            .filter(|page| one.fires(page) != other.fires(page))
            .count();

        assert!(differences > 20, "two seeds drew the same pages");
    }

    #[test]
    fn exploring_never_repeats_the_action_the_router_chose() {
        let explorer = Explorer::new(1.0);

        for n in 0..300 {
            let page = url(&format!("https://example.com/p/{n}"));
            let chosen = explorer
                .choose(&page, &routed(), &Budget::unlimited())
                .expect("a rate of one explores every page");
            assert_ne!(chosen.action, routed().action);
            assert_eq!(chosen.source, RouteSource::Explore);
            assert_eq!(chosen.start_rung, 0);
        }
    }

    #[test]
    fn exploring_stays_inside_the_credit_cap() {
        // Twice the floor, so only the plain fetch and the mixed mode fit.
        let budget = Budget::default().with_credits(Credits(ASSUMED_MINIMUM_COST.get() * 2.0));
        let explorer = Explorer::new(1.0);
        let mut seen = 0;

        for n in 0..300 {
            let page = url(&format!("https://example.com/p/{n}"));
            let Some(chosen) = explorer.choose(&page, &routed(), &budget) else {
                continue;
            };
            seen += 1;
            assert_eq!(
                chosen.mode(),
                RequestMode::Http,
                "an arm dearer than the cap was explored: {chosen:?}"
            );
            assert_eq!(chosen.proxy(), ProxyPool::Isp);
        }

        assert!(seen > 0, "nothing was explored, so the cap proved nothing");
    }

    #[test]
    fn a_per_page_cap_refuses_a_dear_arm_too() {
        let budget = Budget::default().with_per_page_credits(ASSUMED_MINIMUM_COST);
        assert!(affordable(&budget, 1.0));
        assert!(!affordable(&budget, 4.0));
    }

    #[test]
    fn a_rate_cannot_leave_its_range() {
        assert_eq!(Explorer::new(-1.0).rate, 0.0);
        assert_eq!(Explorer::new(9.0).rate, 1.0);
    }
}
