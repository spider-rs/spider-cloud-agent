//! What a router answers with.
//!
//! An [`Action`] is the settings one attempt uses. A [`RouteDecision`] is that
//! action plus where it came from, how sure the router is, and where the
//! escalation should pick up if the attempt fails anyway.
//!
//! Everything here is public request vocabulary. A decision can be printed in
//! a log, handed to a support case, or replayed by hand against the API, and
//! none of it says anything about how a fetch is carried out.

use crate::action::{Country, ProxyPool, RequestMode};

/// What has to happen before the page is read.
///
/// Two values, because the choice worth making is whether to pay for settling
/// time at all. Anything finer belongs in the request parameters, where a
/// caller who knows the page can say exactly what to wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Wait {
    /// Read the page as soon as it arrives.
    #[default]
    Now,
    /// Give the page time to stop fetching things, up to this many
    /// milliseconds, then read it regardless.
    Settled {
        /// The longest the page gets before it is read anyway.
        millis: u32,
    },
}

impl Wait {
    /// The span a settled wait allows, in milliseconds. Zero when nothing is
    /// waited for.
    pub const fn millis(self) -> u32 {
        match self {
            Wait::Now => 0,
            Wait::Settled { millis } => millis,
        }
    }
}

/// The settings one attempt uses.
///
/// Small on purpose. Every value here is a label a model has to learn and a
/// promise the crate has to keep across releases, so the set grows only when
/// something is worth both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct Action {
    /// How the page is fetched.
    pub mode: RequestMode,
    /// Which pool the request leaves from.
    pub proxy: ProxyPool,
    /// What has to happen before the page is read.
    pub wait: Wait,
}

impl Action {
    /// An action with a mode and the defaults for the rest.
    pub const fn new(mode: RequestMode) -> Action {
        Action {
            mode,
            proxy: ProxyPool::Isp,
            wait: Wait::Now,
        }
    }

    /// The same action from another pool.
    pub const fn with_proxy(mut self, proxy: ProxyPool) -> Action {
        self.proxy = proxy;
        self
    }

    /// The same action with a settling wait.
    pub const fn with_wait(mut self, wait: Wait) -> Action {
        self.wait = wait;
        self
    }
}

/// Where a decision came from.
///
/// Worth reporting because the four differ in what a caller should do about a
/// bad outcome. A caller decision is the caller's own; a memory decision says
/// the caller's records drove it and clearing them changes the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum RouteSource {
    /// The rules decided, with nothing remembered about this site.
    #[default]
    Heuristic,
    /// Weights decided.
    Model,
    /// The caller fixed the settings and the router passed them through.
    Caller,
    /// What the caller remembers about this site changed the answer.
    Memory,
    /// The client took a different action than the router picked, on purpose,
    /// so that something is learned about the arms nobody pulls. A decision
    /// marked this way is not a recommendation and should not be read as one.
    Explore,
}

/// A routing answer.
///
/// The action is the whole of what the first attempt should use. `country` sits
/// outside it because a country is not part of the reduced set of actions a
/// model scores: which country to try is a rule, and in this version only a
/// caller sets one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RouteDecision {
    /// The settings for the first attempt.
    pub action: Action,
    /// Where the request should appear to come from, when something pinned it.
    pub country: Option<Country>,
    /// Where escalation starts if the first attempt fails.
    ///
    /// Zero means start at the top and skip nothing. A higher number says the
    /// router already knows the cheap steps do not work here, so the ladder
    /// begins that many steps in. It is only ever a starting point: the policy
    /// engine decides everything after the first answer comes back.
    pub start_rung: u8,
    /// How sure the router is, from zero to one.
    ///
    /// A rule that settles the question on its own reports close to one. Cold
    /// start reports the middle, and a caller pin reports one because there
    /// was nothing to be unsure about.
    pub confidence: f32,
    /// Which layer answered.
    pub source: RouteSource,
}

impl Default for RouteDecision {
    fn default() -> RouteDecision {
        RouteDecision {
            action: Action::default(),
            country: None,
            start_rung: 0,
            confidence: 0.5,
            source: RouteSource::Heuristic,
        }
    }
}

impl RouteDecision {
    /// A decision that uses this action and nothing else.
    pub fn new(action: Action, source: RouteSource, confidence: f32) -> RouteDecision {
        RouteDecision {
            action,
            country: None,
            start_rung: 0,
            confidence: confidence.clamp(0.0, 1.0),
            source,
        }
    }

    /// Pin the country this decision asks for.
    pub fn with_country(mut self, country: Option<Country>) -> RouteDecision {
        self.country = country;
        self
    }

    /// Start the escalation this many steps in.
    pub const fn starting_at(mut self, rung: u8) -> RouteDecision {
        self.start_rung = rung;
        self
    }

    /// How the page is fetched.
    pub const fn mode(&self) -> RequestMode {
        self.action.mode
    }

    /// Which pool the request leaves from.
    pub const fn proxy(&self) -> ProxyPool {
        self.action.proxy
    }

    /// What has to happen before the page is read.
    pub const fn wait(&self) -> Wait {
        self.action.wait
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

    #[test]
    fn the_accessors_read_the_action() {
        let action = Action::new(RequestMode::Browser)
            .with_proxy(ProxyPool::Residential)
            .with_wait(Wait::Settled { millis: 10_000 });
        let decision = RouteDecision::new(action, RouteSource::Heuristic, 0.8);

        assert_eq!(decision.mode(), RequestMode::Browser);
        assert_eq!(decision.proxy(), ProxyPool::Residential);
        assert_eq!(decision.wait().millis(), 10_000);
        assert_eq!(Wait::Now.millis(), 0);
    }

    #[test]
    fn confidence_cannot_leave_its_range() {
        assert_eq!(
            RouteDecision::new(Action::default(), RouteSource::Model, 4.0).confidence,
            1.0
        );
        assert_eq!(
            RouteDecision::new(Action::default(), RouteSource::Model, -1.0).confidence,
            0.0
        );
    }

    #[test]
    fn the_default_decision_is_the_default_settings() {
        let decision = RouteDecision::default();

        assert_eq!(decision.mode(), RequestMode::Smart);
        assert_eq!(decision.proxy(), ProxyPool::Isp);
        assert_eq!(decision.wait(), Wait::Now);
        assert_eq!(decision.start_rung, 0);
        assert_eq!(decision.source, RouteSource::Heuristic);
    }
}
