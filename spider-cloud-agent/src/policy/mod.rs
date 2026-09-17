//! When to try again, and what to change before you do.
//!
//! This is the decision layer, and it is the whole of it. It performs no calls, opens
//! no connections and sleeps for nothing. [`Policy::decide`] is a function of what one
//! attempt saw and what the operation has spent, and it answers with the next move.
//! The send loop does the waiting and the calling.
//!
//! Keeping it that way is the point. An escalation strategy that only exists inside a
//! network loop can be argued about but not checked. This one is driven from a table of
//! scripted responses in `tests/policy_sim.rs`, so a change in the rules shows up as a
//! change in the asserted sequence of moves and in the total spend.
//!
//! ```
//! use spider_cloud_agent::policy::{AttemptState, Budget, Next, Observed, Policy};
//!
//! let policy = Policy::for_target("https://example.com/pricing");
//! let mut state = AttemptState::new(Budget::default());
//!
//! // The site refused the fetch, which is what the ladder is for.
//! let seen = Observed::seen(200, Some(403));
//! state.record(&seen);
//!
//! match policy.decide(&seen, &state) {
//!     Next::Escalate { step, .. } => assert_eq!(step.label, "browser"),
//!     other => panic!("expected an escalation, got {other:?}"),
//! }
//! ```
//!
//! # What the ladder will and will not do
//!
//! Every step sets documented request parameters and nothing else, so an escalation is
//! always a change a caller could have made by hand and can read back off the request.
//! Two pools exist and the ladder uses both. Nothing here selects the software that
//! performs a rendered fetch, and nothing here touches the measures the service applies
//! on your behalf, which it decides better than a client can.

pub mod backoff;
pub mod budget;
pub mod engine;
pub mod ladder;
pub mod rule;

pub use backoff::{Backoff, Jitter};
pub use budget::{Budget, ASSUMED_MINIMUM_COST, DEFAULT_ATTEMPTS};
pub use engine::{
    target_slow_down, AttemptState, Next, Observed, Policy, Reached, StopReason, SESSION_LABEL,
};
pub use ladder::{CountryPool, Ladder, Rung, Step};
pub use rule::{default_rules, Decision, Rule, Trigger};
