//! What an attempt looked like, and what to do about it.
//!
//! A rule is a pair: the thing that was seen, and the move that answers it. The table
//! of them is ordinary data, so it can be read, printed and replaced without touching
//! the engine that walks it.

use crate::policy::ladder::Step;
use crate::status::{ApiClass, PageClass};

/// The thing an attempt was seen to do.
///
/// The two status planes are separate variants because they mean different things. A
/// 403 from the service means the key was refused. A 403 from the site means the fetch
/// was refused, and that one is worth escalating.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Trigger {
    /// The service answered with this class. A rate limit or a drain matches whatever
    /// wait time came with it, so writing one with no wait time still matches one that
    /// carried a header.
    Api(ApiClass),
    /// The site answered with this class.
    Page(PageClass),
    /// The site answered with a 2xx and nothing in it.
    EmptyContent,
    /// The call did not answer in time.
    Timeout,
    /// The call never reached the service.
    Connect,
}

impl Trigger {
    /// Whether exhausting the retries on this trigger should stop rather than climb the
    /// ladder.
    ///
    /// True for the service's own rate limit and for load shedding. Both are about how
    /// often this account is calling, not about how hard the page is, so a heavier
    /// request buys the same refusal at a higher price.
    pub fn retry_only(&self) -> bool {
        matches!(
            self,
            Trigger::Api(ApiClass::RateLimited { .. }) | Trigger::Api(ApiClass::Draining { .. })
        )
    }

    /// Whether this trigger names a class on the call plane.
    pub fn is_api_plane(&self) -> bool {
        matches!(self, Trigger::Api(_) | Trigger::Timeout | Trigger::Connect)
    }
}

/// The move that answers a trigger.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Decision {
    /// Take what came back. Nothing further is spent.
    Accept,
    /// Send the same request again, up to `max` times, then climb the ladder.
    RetrySame {
        /// How many times the same request may be sent again before the answer counts
        /// as settled.
        max: u8,
    },
    /// Move to the next step on the ladder.
    Escalate,
    /// Move to a named step, whatever the ladder would have chosen.
    EscalateTo(Step),
    /// Stop. Nothing in the request will change this answer.
    Fail,
}

/// One trigger and the move that answers it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// The thing that was seen.
    pub when: Trigger,
    /// The move that answers it.
    pub then: Decision,
}

impl Rule {
    /// Pair a trigger with a move.
    pub fn new(when: Trigger, then: Decision) -> Rule {
        Rule { when, then }
    }
}

/// The rules used when the caller names none.
///
/// Order matters, and only in one place: [`Trigger::EmptyContent`] comes before
/// [`PageClass::Ok`], because a blank 2xx is both of those and the blank reading is the
/// useful one. A test in this module holds that order in place.
pub fn default_rules() -> Vec<Rule> {
    vec![
        // The site said yes and sent nothing. The most common real failure, and the one
        // that looks like a success to anything only reading status codes.
        Rule::new(Trigger::EmptyContent, Decision::Escalate),
        // The page plane. A call can succeed while the fetch inside it failed, so these
        // are read first whenever there is a site status to read.
        Rule::new(Trigger::Page(PageClass::Ok), Decision::Accept),
        Rule::new(Trigger::Page(PageClass::BadRequest), Decision::Fail),
        // Read as a stop unless the request is in the one shape a `session` rung would
        // change. See `Policy::session_step`.
        Rule::new(Trigger::Page(PageClass::NeedsLogin), Decision::Fail),
        // The headline path. A refused fetch is what the ladder exists for.
        Rule::new(Trigger::Page(PageClass::Blocked), Decision::Escalate),
        // Nothing in the request brings back a page that is not there.
        //
        // Some sites serve a 404 to a client they have decided not to serve, and that
        // one would yield to a heavier request. Nothing reaching this table can tell the
        // two apart, so a soft block dressed as a 404 stops here alongside a real one.
        // That is the deliberate cost of never spending a call on a page that is gone.
        Rule::new(Trigger::Page(PageClass::NotFound), Decision::Fail),
        Rule::new(
            Trigger::Page(PageClass::TargetRateLimited),
            Decision::RetrySame { max: 1 },
        ),
        Rule::new(
            Trigger::Page(PageClass::ServerError),
            Decision::RetrySame { max: 1 },
        ),
        // The call plane. These fire when no site was reached.
        Rule::new(Trigger::Api(ApiClass::Ok), Decision::Accept),
        // Nothing to return is a result, not an error.
        Rule::new(Trigger::Api(ApiClass::NoContent), Decision::Accept),
        Rule::new(Trigger::Api(ApiClass::BadRequest), Decision::Fail),
        Rule::new(Trigger::Api(ApiClass::Unauthorized), Decision::Fail),
        // Every attempt after the balance hits zero fails the same way, and some of
        // them still cost money.
        Rule::new(Trigger::Api(ApiClass::InsufficientCredits), Decision::Fail),
        Rule::new(Trigger::Api(ApiClass::PayloadTooLarge), Decision::Fail),
        Rule::new(
            Trigger::Api(ApiClass::RateLimited { retry_after: None }),
            Decision::RetrySame { max: 3 },
        ),
        Rule::new(
            Trigger::Api(ApiClass::ServerError),
            Decision::RetrySame { max: 1 },
        ),
        Rule::new(
            Trigger::Api(ApiClass::Draining { retry_after: None }),
            Decision::RetrySame { max: 3 },
        ),
        Rule::new(Trigger::Timeout, Decision::RetrySame { max: 1 }),
        Rule::new(Trigger::Connect, Decision::RetrySame { max: 1 }),
    ]
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

    fn position(rules: &[Rule], trigger: &Trigger) -> usize {
        rules
            .iter()
            .position(|rule| &rule.when == trigger)
            .unwrap_or_else(|| panic!("no rule for {trigger:?}"))
    }

    #[test]
    fn a_blank_page_is_read_before_a_served_one() {
        let rules = default_rules();
        assert!(
            position(&rules, &Trigger::EmptyContent)
                < position(&rules, &Trigger::Page(PageClass::Ok)),
            "a blank 2xx would be accepted instead of escalated"
        );
    }

    #[test]
    fn every_mapped_class_on_both_planes_has_a_rule() {
        let rules = default_rules();
        for class in [
            PageClass::Ok,
            PageClass::BadRequest,
            PageClass::NeedsLogin,
            PageClass::Blocked,
            PageClass::NotFound,
            PageClass::TargetRateLimited,
            PageClass::ServerError,
        ] {
            position(&rules, &Trigger::Page(class));
        }
        for class in [
            ApiClass::Ok,
            ApiClass::NoContent,
            ApiClass::BadRequest,
            ApiClass::Unauthorized,
            ApiClass::InsufficientCredits,
            ApiClass::PayloadTooLarge,
            ApiClass::RateLimited { retry_after: None },
            ApiClass::ServerError,
            ApiClass::Draining { retry_after: None },
        ] {
            position(&rules, &Trigger::Api(class));
        }
        position(&rules, &Trigger::Timeout);
        position(&rules, &Trigger::Connect);
    }

    #[test]
    fn running_out_of_credits_is_never_a_retry() {
        let rules = default_rules();
        let index = position(&rules, &Trigger::Api(ApiClass::InsufficientCredits));
        assert_eq!(rules[index].then, Decision::Fail);
    }

    #[test]
    fn a_missing_page_is_never_a_retry_and_never_an_escalation() {
        let rules = default_rules();
        let index = position(&rules, &Trigger::Page(PageClass::NotFound));
        assert_eq!(rules[index].then, Decision::Fail);
    }

    #[test]
    fn every_transient_api_status_has_a_retry_rule_the_caller_agrees_with() {
        use crate::status::api::TRANSIENT;
        use crate::status::ApiStatus;

        let rules = default_rules();
        for code in TRANSIENT {
            let status = ApiStatus::new(code);
            let index = position(&rules, &Trigger::Api(status.class()));
            assert!(
                matches!(rules[index].then, Decision::RetrySame { .. }),
                "api {code} is transient but its rule does not retry"
            );
            let error = crate::Error::Api {
                status,
                message: None,
                retry_after: None,
            };
            assert!(
                error.is_retryable(),
                "api {code} is retried but not retryable"
            );
        }

        // The other direction: a status the table retries is one the caller is told
        // to retry, and nothing else is.
        for code in 100..=599u16 {
            let status = ApiStatus::new(code);
            let retried = rules.iter().any(|rule| {
                rule.when == Trigger::Api(status.class())
                    && matches!(rule.then, Decision::RetrySame { .. })
            });
            let error = crate::Error::Api {
                status,
                message: None,
                retry_after: None,
            };
            assert_eq!(error.is_retryable(), retried, "api {code}");
        }
    }

    #[test]
    fn only_the_accounts_own_limits_stop_instead_of_climbing() {
        assert!(Trigger::Api(ApiClass::RateLimited { retry_after: None }).retry_only());
        assert!(Trigger::Api(ApiClass::Draining { retry_after: None }).retry_only());
        assert!(!Trigger::Api(ApiClass::ServerError).retry_only());
        assert!(!Trigger::Page(PageClass::TargetRateLimited).retry_only());
        assert!(!Trigger::Timeout.retry_only());
        assert!(!Trigger::Connect.retry_only());
    }

    #[test]
    fn the_planes_are_labelled() {
        assert!(Trigger::Api(ApiClass::Ok).is_api_plane());
        assert!(Trigger::Timeout.is_api_plane());
        assert!(Trigger::Connect.is_api_plane());
        assert!(!Trigger::Page(PageClass::Ok).is_api_plane());
        assert!(!Trigger::EmptyContent.is_api_plane());
    }
}
