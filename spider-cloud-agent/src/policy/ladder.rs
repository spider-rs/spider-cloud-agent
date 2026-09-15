//! The steps an escalation walks, cheapest first.
//!
//! A [`Rung`] is one public request parameter with a value. A [`Step`] is the set of
//! rungs that define one position on the ladder, and a [`Ladder`] is those positions
//! in the order they are tried.
//!
//! Every step is a superset of the one before it, so escalating never gives up
//! something that was already helping. Each step also costs more than the one before,
//! which is what [`Step::est_multiplier`] records and what lets a budget refuse a step
//! before paying for it.

use crate::credits::Credits;
use crate::params::js::Timeout;
use crate::params::{Country, Profile, ProxyPool, RequestMode, RequestParams, WaitFor};
use crate::policy::budget::Budget;
use std::time::Duration;
use url::Url;

/// One request parameter, with the value an escalation sets it to.
///
/// The list is short on purpose. Each entry is a documented parameter a caller could
/// set by hand and read back off the request, so an escalation is always explainable.
/// Anything finer belongs behind the full parameter struct, not here.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Rung {
    /// Set `request`, which decides whether the page is rendered before it is read.
    Mode(RequestMode),
    /// Set `proxy`, which decides the pool the request leaves from.
    Proxy(ProxyPool),
    /// Set `country_code`, which decides where the request appears to come from.
    Country(Country),
    /// Set `wait_for`, which decides what has to happen before the page is read.
    Wait(WaitFor),
    /// Set the user agent and viewport that go with a coarse identity.
    Profile(Profile),
    /// Set `request_timeout`, the span one page has to come back in.
    Timeout(Duration),
    /// Set `session`, which decides whether state carries from one request to the next.
    Session(bool),
}

impl Rung {
    /// Write this rung onto a request, replacing whatever was there.
    ///
    /// Replacing is the point: an escalation is a deliberate override of what the
    /// previous attempt sent.
    pub fn apply(&self, params: &mut RequestParams) {
        match self {
            Rung::Mode(mode) => params.request = Some(*mode),
            Rung::Proxy(pool) => params.proxy = Some(*pool),
            Rung::Country(country) => params.country_code = Some(country.clone()),
            Rung::Wait(wait) => params.wait_for = Some(wait.clone()),
            Rung::Profile(profile) => {
                if let Some(agent) = profile.user_agent() {
                    params.user_agent = Some(agent.to_string());
                }
                params.viewport = Some(profile.viewport());
            }
            Rung::Timeout(span) => {
                let secs = span.as_secs().clamp(1, u64::from(u8::MAX)) as u8;
                params.request_timeout = Some(secs);
            }
            Rung::Session(on) => params.session = Some(*on),
        }
    }
}

/// One position on the ladder.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// The parameters this step sets.
    pub rungs: Vec<Rung>,
    /// A short name for the step, safe to log and to show a caller.
    pub label: &'static str,
    /// What this step is expected to cost, as a multiple of a plain fetch of the same
    /// page. A budget multiplies it by the last observed cost to decide whether the
    /// step is affordable before sending it.
    pub est_multiplier: f32,
}

impl Step {
    /// Build a step.
    pub fn new(label: &'static str, est_multiplier: f32, rungs: Vec<Rung>) -> Step {
        Step {
            rungs,
            label,
            est_multiplier,
        }
    }

    /// Write every rung of this step onto a request.
    pub fn apply(&self, params: &mut RequestParams) {
        for rung in &self.rungs {
            rung.apply(params);
        }
    }

    /// The country this step pins, when it pins one.
    pub fn country(&self) -> Option<&Country> {
        self.rungs.iter().find_map(|rung| match rung {
            Rung::Country(country) => Some(country),
            _ => None,
        })
    }

    /// Whether this step changes where the request comes from.
    pub fn moves_country(&self) -> bool {
        self.country().is_some()
    }

    /// What this step is expected to cost, given what the last attempt cost.
    ///
    /// A basis of nothing is raised to [`Budget::floor`] before it is scaled, so an
    /// escalation that follows an unbilled attempt still produces a figure a credit cap
    /// can refuse. Scaling happens after the floor, so the dearest step still estimates
    /// dearest.
    pub fn estimate(&self, last_cost: Credits) -> Credits {
        Credits(Budget::floor(last_cost).get() * f64::from(self.est_multiplier))
    }
}

/// The countries a geographic escalation may rotate through, in order.
///
/// There is no built-in list of far away places. A request that fails from one address
/// is not helped by an address chosen for being unusual, and a caller who knows the
/// site knows which countries it serves.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CountryPool(Vec<Country>);

impl CountryPool {
    /// A pool from an explicit list, in the order they will be tried. Repeats are
    /// dropped.
    pub fn new(countries: impl IntoIterator<Item = Country>) -> CountryPool {
        let mut out: Vec<Country> = Vec::new();
        for country in countries {
            if !out.contains(&country) {
                out.push(country);
            }
        }
        CountryPool(out)
    }

    /// The pool to use when the caller named none: the address's own country code when
    /// it has one, then `us`.
    ///
    /// A site on a two letter national domain usually serves that country best, and a
    /// site on anything else is most often served from the United States.
    pub fn for_url(url: &str) -> CountryPool {
        let mut countries = Vec::new();

        if let Some(country) = Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .and_then(|host| host.rsplit('.').next().and_then(Country::new))
        {
            countries.push(country);
        }

        if let Some(fallback) = Country::new("us") {
            countries.push(fallback);
        }

        CountryPool::new(countries)
    }

    /// The countries, in order.
    pub fn countries(&self) -> &[Country] {
        &self.0
    }

    /// Whether the pool holds nothing, in which case a ladder built from it has no
    /// geographic step.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for Ladder {
    /// The standard ladder with no address to read a country from, so the geographic
    /// step rotates to `us`.
    fn default() -> Ladder {
        Ladder::standard()
    }
}

/// The steps in the order they are tried.
///
/// Index zero is the first escalation, not the first attempt. The first attempt uses
/// whatever the caller or the router already chose, and the ladder is where a failure
/// goes next.
#[derive(Debug, Clone, PartialEq)]
pub struct Ladder(pub Vec<Step>);

/// How long a wait step gives the page to settle before reading it anyway.
const SETTLE: Timeout = Timeout::from_secs(10);

impl Ladder {
    /// The standard ladder, rotating to `us` at the geographic step.
    pub fn standard() -> Ladder {
        Ladder::with_countries(&CountryPool::new(Country::new("us")))
    }

    /// The standard ladder with the geographic step reading its countries from the
    /// address being fetched.
    pub fn for_target(url: &str) -> Ladder {
        Ladder::with_countries(&CountryPool::for_url(url))
    }

    /// The standard ladder with a caller's own country rotation.
    ///
    /// The first three steps never change. The pool adds one step per country after
    /// them, each costing slightly more than the last so the order stays readable off
    /// the multipliers alone.
    pub fn with_countries(pool: &CountryPool) -> Ladder {
        let rendered = vec![Rung::Mode(RequestMode::Browser)];

        let settled = vec![
            Rung::Mode(RequestMode::Browser),
            Rung::Wait(WaitFor::idle_network(SETTLE)),
        ];

        let mut residential = settled.clone();
        residential.push(Rung::Proxy(ProxyPool::Residential));

        let mut steps = vec![
            Step::new("browser", 4.0, rendered),
            Step::new("browser+wait", 5.0, settled),
            Step::new("residential", 8.0, residential.clone()),
        ];

        for (offset, country) in pool.countries().iter().enumerate() {
            let mut rungs = residential.clone();
            rungs.push(Rung::Country(country.clone()));
            steps.push(Step::new("residential+geo", 9.0 + offset as f32, rungs));
        }

        Ladder(steps)
    }

    /// The step at an index, when the ladder goes that far.
    pub fn get(&self, index: usize) -> Option<&Step> {
        self.0.get(index)
    }

    /// How many steps the ladder holds.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the ladder holds no steps at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The first step at or after `from` that changes the country.
    ///
    /// A site that is rate limiting the fetch is not helped by a heavier request, and
    /// sending one costs more for the same refusal. Coming from somewhere else is the
    /// move that has a chance, so a rate limit jumps here rather than walking.
    pub fn first_geo_from(&self, from: usize) -> Option<usize> {
        (from..self.0.len()).find(|index| self.0.get(*index).is_some_and(Step::moves_country))
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
    use crate::policy::budget::ASSUMED_MINIMUM_COST;

    #[test]
    fn the_standard_ladder_is_four_steps_ending_in_a_country_move() {
        let ladder = Ladder::standard();
        let labels: Vec<_> = ladder.0.iter().map(|step| step.label).collect();
        assert_eq!(
            labels,
            ["browser", "browser+wait", "residential", "residential+geo"]
        );
        assert_eq!(
            ladder.get(3).and_then(Step::country).map(Country::as_str),
            Some("us")
        );
    }

    #[test]
    fn each_step_is_a_superset_of_the_one_before() {
        let ladder = Ladder::standard();
        for pair in ladder.0.windows(2) {
            for rung in &pair[0].rungs {
                assert!(
                    pair[1].rungs.contains(rung),
                    "{} dropped {rung:?} that {} had",
                    pair[1].label,
                    pair[0].label
                );
            }
        }
    }

    #[test]
    fn the_estimates_only_go_up() {
        for ladder in [
            Ladder::standard(),
            Ladder::for_target("https://example.de/thing"),
        ] {
            for pair in ladder.0.windows(2) {
                assert!(
                    pair[1].est_multiplier > pair[0].est_multiplier,
                    "{} does not cost more than {}",
                    pair[1].label,
                    pair[0].label
                );
            }
        }
    }

    #[test]
    fn a_national_domain_rotates_to_its_own_country_first() {
        let pool = CountryPool::for_url("https://shop.example.de/a");
        let codes: Vec<_> = pool.countries().iter().map(Country::as_str).collect();
        assert_eq!(codes, ["de", "us"]);

        let ladder = Ladder::for_target("https://shop.example.de/a");
        assert_eq!(ladder.len(), 5);
        assert_eq!(
            ladder.get(3).and_then(Step::country).map(Country::as_str),
            Some("de")
        );
        assert_eq!(
            ladder.get(4).and_then(Step::country).map(Country::as_str),
            Some("us")
        );
    }

    #[test]
    fn a_generic_domain_rotates_to_us_only() {
        let pool = CountryPool::for_url("https://example.com/a");
        let codes: Vec<_> = pool.countries().iter().map(Country::as_str).collect();
        assert_eq!(codes, ["us"]);
    }

    #[test]
    fn a_us_domain_is_not_listed_twice() {
        let pool = CountryPool::for_url("https://example.us/a");
        let codes: Vec<_> = pool.countries().iter().map(Country::as_str).collect();
        assert_eq!(codes, ["us"]);
    }

    #[test]
    fn the_geo_jump_finds_the_first_country_step() {
        let ladder = Ladder::standard();
        assert_eq!(ladder.first_geo_from(0), Some(3));
        assert_eq!(ladder.first_geo_from(3), Some(3));
        assert_eq!(ladder.first_geo_from(4), None);
    }

    #[test]
    fn applying_a_step_writes_the_parameters_it_names() {
        let mut params = RequestParams::url("https://example.com");
        Ladder::standard()
            .get(3)
            .expect("the geographic step")
            .apply(&mut params);

        assert_eq!(params.request, Some(RequestMode::Browser));
        assert_eq!(params.proxy, Some(ProxyPool::Residential));
        assert_eq!(
            params.country_code.as_ref().map(Country::as_str),
            Some("us")
        );
        assert!(params
            .wait_for
            .as_ref()
            .is_some_and(|wait| wait.idle_network.is_some()));
        // The escalation never touches what the service decides for itself.
        assert_eq!(params.stealth, None);
        assert_eq!(params.fingerprint, None);
    }

    #[test]
    fn applying_a_step_replaces_what_the_last_attempt_sent() {
        let mut params = RequestParams::url("https://example.com");
        params.proxy = Some(ProxyPool::Isp);
        Ladder::standard()
            .get(2)
            .expect("the residential step")
            .apply(&mut params);
        assert_eq!(params.proxy, Some(ProxyPool::Residential));
    }

    #[test]
    fn a_timeout_rung_is_written_in_whole_seconds() {
        let mut params = RequestParams::default();
        Rung::Timeout(Duration::from_millis(45_500)).apply(&mut params);
        assert_eq!(params.request_timeout, Some(45));

        Rung::Timeout(Duration::from_secs(9_000)).apply(&mut params);
        assert_eq!(params.request_timeout, Some(255));

        Rung::Timeout(Duration::from_millis(10)).apply(&mut params);
        assert_eq!(params.request_timeout, Some(1));
    }

    #[test]
    fn a_profile_rung_sets_the_viewport_and_only_pins_an_agent_when_the_profile_does() {
        let mut params = RequestParams::default();
        Rung::Profile(Profile::Desktop).apply(&mut params);
        assert_eq!(params.user_agent, None);
        assert!(params.viewport.is_some());

        Rung::Profile(Profile::Bot).apply(&mut params);
        assert!(params.user_agent.is_some());
    }

    #[test]
    fn a_session_rung_sets_the_session_flag() {
        let mut params = RequestParams::default();
        Rung::Session(true).apply(&mut params);
        assert_eq!(params.session, Some(true));
    }

    #[test]
    fn the_ladder_ends_on_the_geographic_step() {
        for ladder in [
            Ladder::standard(),
            Ladder::for_target("https://shop.example.de/a"),
        ] {
            let last = ladder.0.last().expect("a last step");
            assert_eq!(last.label, "residential+geo");
            assert!(last.moves_country(), "the last step changes the country");
        }
    }

    #[test]
    fn an_estimate_scales_the_last_observed_cost() {
        let step = Ladder::standard().get(2).expect("residential").clone();
        assert_eq!(step.estimate(Credits(3.0)), Credits(24.0));
    }

    #[test]
    fn an_estimate_after_a_free_attempt_still_names_a_figure() {
        let ladder = Ladder::standard();
        let cheapest = ladder.get(0).expect("browser");
        let dearest = ladder.get(3).expect("the geographic step");

        // Scaling zero would give zero for every step, and a credit cap fed zero can
        // refuse nothing. The figures are multiples of the floor, so they follow it.
        let floor = ASSUMED_MINIMUM_COST.get();
        assert_eq!(cheapest.estimate(Credits::ZERO), Credits(floor * 4.0));
        assert_eq!(dearest.estimate(Credits::ZERO), Credits(floor * 9.0));
        assert!(dearest.estimate(Credits::ZERO) > cheapest.estimate(Credits::ZERO));
    }

    #[test]
    fn an_empty_pool_leaves_the_ladder_without_a_country_step() {
        let ladder = Ladder::with_countries(&CountryPool::default());
        assert_eq!(ladder.len(), 3);
        assert_eq!(ladder.first_geo_from(0), None);
    }
}
