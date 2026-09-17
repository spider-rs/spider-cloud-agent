//! The boundary between deciding and fetching.
//!
//! A [`Router`] answers one question with no input or output of any kind:
//! given the shape of a request, what should the first attempt use. Everything
//! after that answer belongs to the policy engine in the client, which reads
//! what came back and decides where to go next.
//!
//! The trait exists so the shipped rules, a set of weights, and whatever a
//! caller writes themselves are all the same thing to the client. That is also
//! what keeps the no weights path honest: it is not a fallback bolted on, it
//! is one implementation of the same trait, tested on its own.

use crate::decision::RouteDecision;
use crate::features::{RouteInput, StatusClass};
use std::sync::Arc;

/// Which layer of the router answered, and at what version.
///
/// Recorded next to every decision so a run can be read back later. A model
/// number is the version of the weights, not of the crate, because the two
/// move independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum RouterVersion {
    /// Rules only, no weights involved.
    #[default]
    Heuristic,
    /// Weights, at this version.
    Model(u16),
}

/// How one attempt ended.
///
/// Coarse, because this is what a caller folds into its own records and hands
/// back as [`crate::SiteMemory`] next time. Anything finer would be a response
/// feature, and those belong to the escalation half of the problem rather than
/// to this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct AttemptOutcome {
    /// Whether the attempt got what the caller asked for.
    pub success: bool,
    /// How the attempt ended.
    pub status: StatusClass,
    /// How many bytes of content came back.
    pub bytes: u32,
    /// How long it took, in milliseconds.
    pub millis: u32,
}

impl AttemptOutcome {
    /// An attempt that worked.
    pub const fn ok(bytes: u32, millis: u32) -> AttemptOutcome {
        AttemptOutcome {
            success: true,
            status: StatusClass::Ok,
            bytes,
            millis,
        }
    }

    /// An attempt that did not.
    pub const fn failed(status: StatusClass, millis: u32) -> AttemptOutcome {
        AttemptOutcome {
            success: false,
            status,
            bytes: 0,
            millis,
        }
    }
}

/// Picks the settings for a first attempt.
///
/// Implementations answer from local state only. A `route` call that reaches
/// the network has broken the contract this crate exists for: the decision has
/// to be cheaper than the call it is deciding about.
pub trait Router: Send + Sync {
    /// Pick the settings for the first attempt.
    fn route(&self, input: &RouteInput<'_>) -> RouteDecision;

    /// Take note of how an attempt ended.
    ///
    /// The default does nothing, which is the honest answer for a router with
    /// no state. A caller that wants adaptivity keeps its own records and
    /// passes them back through [`crate::SiteMemory`], which is what lets per
    /// site learning happen without any site reaching the weights.
    fn observe(&self, _input: &RouteInput<'_>, _outcome: &AttemptOutcome) {}

    /// Which layer this is, for the record.
    fn version(&self) -> RouterVersion {
        RouterVersion::Heuristic
    }
}

impl<T: Router + ?Sized> Router for Arc<T> {
    fn route(&self, input: &RouteInput<'_>) -> RouteDecision {
        (**self).route(input)
    }

    fn observe(&self, input: &RouteInput<'_>, outcome: &AttemptOutcome) {
        (**self).observe(input, outcome);
    }

    fn version(&self) -> RouterVersion {
        (**self).version()
    }
}

impl<T: Router + ?Sized> Router for &T {
    fn route(&self, input: &RouteInput<'_>) -> RouteDecision {
        (**self).route(input)
    }

    fn observe(&self, input: &RouteInput<'_>, outcome: &AttemptOutcome) {
        (**self).observe(input, outcome);
    }

    fn version(&self) -> RouterVersion {
        (**self).version()
    }
}

impl<T: Router + ?Sized> Router for Box<T> {
    fn route(&self, input: &RouteInput<'_>) -> RouteDecision {
        (**self).route(input)
    }

    fn observe(&self, input: &RouteInput<'_>, outcome: &AttemptOutcome) {
        (**self).observe(input, outcome);
    }

    fn version(&self) -> RouterVersion {
        (**self).version()
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
    use crate::action::RequestMode;
    use crate::decision::{Action, RouteSource};
    use crate::features::DeclaredNeed;
    use crate::HeuristicRouter;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use url::Url;

    /// A caller's own router, which is the case the trait exists for.
    struct AlwaysRendered {
        seen: AtomicUsize,
    }

    impl Router for AlwaysRendered {
        fn route(&self, _input: &RouteInput<'_>) -> RouteDecision {
            RouteDecision::new(Action::new(RequestMode::Browser), RouteSource::Caller, 1.0)
        }

        fn observe(&self, _input: &RouteInput<'_>, _outcome: &AttemptOutcome) {
            self.seen.fetch_add(1, Ordering::Relaxed);
        }

        fn version(&self) -> RouterVersion {
            RouterVersion::Model(7)
        }
    }

    fn route_through(router: &impl Router) -> RouteDecision {
        let url = Url::parse("https://example.com/a").unwrap();
        router.route(&RouteInput::new(&url, DeclaredNeed::Markdown))
    }

    #[test]
    fn a_wrapped_router_behaves_like_the_router_it_wraps() {
        let own = Arc::new(AlwaysRendered {
            seen: AtomicUsize::new(0),
        });
        let url = Url::parse("https://example.com/a").unwrap();
        let input = RouteInput::new(&url, DeclaredNeed::Markdown);

        assert_eq!(route_through(&own).mode(), RequestMode::Browser);
        assert_eq!(own.version(), RouterVersion::Model(7));

        own.observe(&input, &AttemptOutcome::ok(1_000, 200));
        assert_eq!(own.seen.load(Ordering::Relaxed), 1);

        let boxed: Box<dyn Router> = Box::new(HeuristicRouter::new());
        assert_eq!(boxed.version(), RouterVersion::Heuristic);
        // The default `observe` has to be callable and has to do nothing.
        boxed.observe(&input, &AttemptOutcome::failed(StatusClass::Blocked, 900));
    }

    #[test]
    fn the_shipped_router_reports_that_no_weights_are_involved() {
        assert_eq!(HeuristicRouter::new().version(), RouterVersion::Heuristic);
    }

    #[test]
    fn an_outcome_says_what_it_is() {
        let ok = AttemptOutcome::ok(2_048, 310);
        assert!(ok.success);
        assert_eq!(ok.status, StatusClass::Ok);

        let refused = AttemptOutcome::failed(StatusClass::Blocked, 120);
        assert!(!refused.success);
        assert_eq!(refused.bytes, 0);
        assert!(refused.status.is_refusal());
    }
}
