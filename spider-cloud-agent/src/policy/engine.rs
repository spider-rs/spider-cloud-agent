//! The decision itself.
//!
//! [`Policy::decide`] takes what one attempt saw and what the operation has spent so
//! far, and answers with the next move. It does no input or output, starts no timers
//! and sleeps for nothing. The send loop owns all of that, and calls in here between
//! calls.
//!
//! That split is what makes a whole escalation strategy testable without a network.
//! `tests/policy_sim.rs` drives this from a table of scripted responses and checks the
//! decisions and the total spend.

use crate::credits::Credits;
use crate::error::BudgetKind;
use crate::params::RequestParams;
use crate::policy::backoff::Backoff;
use crate::policy::budget::Budget;
use crate::policy::ladder::{CountryPool, Ladder, Rung, Step};
use crate::policy::rule::{default_rules, Decision, Rule, Trigger};
use crate::response::Hint;
use crate::status::{ApiClass, ApiStatus, PageClass, PageStatus};
use std::time::Duration;

/// How far one attempt got on the call plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reached {
    /// The service answered with this status.
    Api(ApiStatus),
    /// The call did not answer in time.
    TimedOut,
    /// The call never reached the service.
    ConnectFailed,
}

/// What one attempt saw.
///
/// Both status planes are here and they stay apart, the same way they do on a recorded
/// attempt. `page` is absent when no site was reached.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Observed {
    /// How far the call got, and the status if it got an answer.
    pub api: Reached,
    /// The status the site returned, when the call reached one.
    pub page: Option<PageStatus>,
    /// Whether the body came back with nothing in it.
    pub empty_content: bool,
    /// What this attempt cost.
    pub cost: Credits,
    /// How long this attempt took, end to end and without any wait before it.
    pub elapsed: Duration,
    /// The wait the service asked for, read from `Retry-After` and falling back to
    /// `RateLimit-Reset`.
    pub retry_after: Option<Duration>,
    /// Whether the request that produced this attempt carried cookies or headers the
    /// caller supplied. A login wall is only worth a second call when it did.
    pub sent_credentials: bool,
    /// Whether the request that produced this attempt already kept state between calls.
    pub sent_session: bool,
}

impl Observed {
    /// One attempt that reached the service, given the two status codes it saw.
    ///
    /// This is a deliberate hole in the rule that only the transport mints a status.
    /// Both status types keep their `pub(crate)` constructors and the live path still
    /// goes through them, so a response body can no more become an [`ApiStatus`] than
    /// it could before. What this adds is a way to build an attempt out of two plain
    /// numbers, which is what replaying recorded crawl history needs and what lets the
    /// whole escalation strategy be driven offline in `tests/policy_sim.rs`. A decision
    /// layer that cannot be fed without a network is a decision layer nobody checks.
    pub fn seen(api: u16, page: Option<u16>) -> Observed {
        Observed {
            api: Reached::Api(ApiStatus::new(api)),
            page: page.map(PageStatus::new),
            empty_content: false,
            cost: Credits::ZERO,
            elapsed: Duration::ZERO,
            retry_after: None,
            sent_credentials: false,
            sent_session: false,
        }
    }

    /// An attempt that did not answer in time.
    pub fn timed_out() -> Observed {
        Observed {
            api: Reached::TimedOut,
            ..Observed::seen(0, None)
        }
    }

    /// An attempt that never reached the service.
    pub fn connect_failed() -> Observed {
        Observed {
            api: Reached::ConnectFailed,
            ..Observed::seen(0, None)
        }
    }

    /// Mark the body as having come back with nothing in it.
    pub fn blank(mut self) -> Observed {
        self.empty_content = true;
        self
    }

    /// Record what the attempt cost.
    pub fn costing(mut self, cost: Credits) -> Observed {
        self.cost = cost;
        self
    }

    /// Record how long the attempt took.
    pub fn taking(mut self, elapsed: Duration) -> Observed {
        self.elapsed = elapsed;
        self
    }

    /// Record the wait the service asked for.
    pub fn retry_after(mut self, after: Duration) -> Observed {
        self.retry_after = Some(after);
        self
    }

    /// Read off the request that produced this attempt what a decision may need to
    /// know about its shape.
    ///
    /// Only two things so far: whether the caller supplied cookies or headers, and
    /// whether state was already being kept between calls. Both decide whether a login
    /// wall is worth another call.
    pub fn for_request(mut self, params: &RequestParams) -> Observed {
        self.sent_credentials = params.cookies.is_some()
            || params
                .headers
                .as_ref()
                .is_some_and(|headers| !headers.is_empty());
        self.sent_session = params.session == Some(true);
        self
    }

    /// Whether this attempt was billed.
    ///
    /// A fetch that reached the site is billed even when the site refused it, because
    /// the work was done either way. A target 500 and a target 503 are the exceptions,
    /// and a call that never reached a site is not billed at all.
    pub fn was_billed(&self) -> bool {
        match self.page {
            Some(page) => page.consumes_credits(),
            None => matches!(
                self.api_class(),
                Some(ApiClass::Ok) | Some(ApiClass::NoContent)
            ),
        }
    }

    /// What the call status means, when there was one.
    pub fn api_class(&self) -> Option<ApiClass> {
        match self.api {
            Reached::Api(status) => Some(status.class_with_retry_after(self.retry_after)),
            Reached::TimedOut | Reached::ConnectFailed => None,
        }
    }

    /// What the site status means, when a site was reached.
    pub fn page_class(&self) -> Option<PageClass> {
        self.page.map(PageStatus::class)
    }

    /// Whether the site said yes and sent nothing.
    ///
    /// A 204 is not this. The service saying it has nothing to return is a result, and
    /// a site serving an empty page is a failure that looks like a success.
    pub fn is_blank_success(&self) -> bool {
        self.empty_content
            && matches!(self.api_class(), Some(ApiClass::Ok))
            && self.page.is_none_or(PageStatus::is_ok)
    }

    /// Whether something asked this caller to slow down, on either plane.
    pub fn asked_to_slow_down(&self) -> bool {
        matches!(
            self.api_class(),
            Some(ApiClass::RateLimited { .. }) | Some(ApiClass::Draining { .. })
        ) || self.page_class() == Some(PageClass::TargetRateLimited)
    }

    /// The single parameter most worth changing next.
    ///
    /// A hint is one step and the policy may go further, so this is what a caller is
    /// told when the policy has decided to stop.
    pub fn hint(&self) -> Hint {
        match self.page_class() {
            Some(class) => Hint::for_class(class),
            None => Hint::Permanent,
        }
    }
}

impl Trigger {
    /// Whether this trigger describes what the attempt saw.
    ///
    /// The page plane wins whenever there is one. A call that succeeded while the fetch
    /// inside it was refused is a failed fetch, so [`ApiClass::Ok`] matches only when
    /// no site was reached.
    pub fn matches(&self, observed: &Observed) -> bool {
        match self {
            Trigger::Timeout => observed.api == Reached::TimedOut,
            Trigger::Connect => observed.api == Reached::ConnectFailed,
            Trigger::EmptyContent => observed.is_blank_success(),
            Trigger::Page(class) => observed.page_class() == Some(*class),
            Trigger::Api(ApiClass::Ok) => {
                observed.page.is_none() && matches!(observed.api_class(), Some(ApiClass::Ok))
            }
            Trigger::Api(class) => match observed.api_class() {
                // Compared by variant, not by value, so a rate limit written with no
                // wait time still matches one that arrived with a header.
                Some(seen) => std::mem::discriminant(&seen) == std::mem::discriminant(class),
                None => false,
            },
        }
    }
}

/// What the operation has done so far, and what it is allowed to do.
///
/// The send loop owns one of these and hands it back on every decision. The budget
/// lives here beside the counters it bounds, so one policy can drive calls running
/// under different caps.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AttemptState {
    /// The caps this operation runs under.
    pub budget: Budget,
    /// How many calls have been made.
    pub attempts: u8,
    /// How many times the current step has been sent again.
    pub retries_at_step: u8,
    /// How many ladder steps have been applied. Zero means the request is still as the
    /// caller or the router left it.
    pub step: usize,
    /// What the operation has spent.
    pub spent: Credits,
    /// How long the operation has taken, sleeps included.
    pub elapsed: Duration,
    /// What the most recent attempt cost, which is what a step estimate scales.
    pub last_cost: Credits,
}

impl AttemptState {
    /// A fresh operation under these caps.
    pub fn new(budget: Budget) -> AttemptState {
        AttemptState {
            budget,
            attempts: 0,
            retries_at_step: 0,
            step: 0,
            spent: Credits::ZERO,
            elapsed: Duration::ZERO,
            last_cost: Credits::ZERO,
        }
    }

    /// Fold in what an attempt saw. Call this once per call, before deciding.
    ///
    /// A cost that cannot be a bill is counted as nothing. A negative figure
    /// would read as a refund and pull the running total under the cap, and a
    /// figure that is not a number poisons every sum it touches, after which no
    /// sum is ever over the cap. Either one arriving on the wire would switch
    /// the credit cap off for the rest of the operation.
    pub fn record(&mut self, observed: &Observed) {
        let cost = if observed.cost.get().is_finite() && observed.cost.get() > 0.0 {
            observed.cost
        } else {
            Credits::ZERO
        };
        self.attempts = self.attempts.saturating_add(1);
        self.spent += cost;
        self.last_cost = cost;
        self.elapsed = self.elapsed.saturating_add(observed.elapsed);
    }

    /// Fold in a decision. Call this once per decision, before acting on it.
    pub fn follow(&mut self, next: &Next) {
        match next {
            Next::Retry { after } => {
                self.retries_at_step = self.retries_at_step.saturating_add(1);
                self.elapsed = self.elapsed.saturating_add(*after);
            }
            Next::Escalate { index, after, .. } => {
                self.step = index.saturating_add(1);
                self.retries_at_step = 0;
                self.elapsed = self.elapsed.saturating_add(*after);
            }
            Next::Accept | Next::Stop(_) => {}
        }
    }
}

/// The next move.
///
/// Not marked as open to new variants, unlike the status classes. A caller has to act
/// on every one of these, so a new move is a change they need to see rather than one a
/// wildcard arm should swallow.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Take what came back.
    Accept,
    /// Wait, then send the same request again.
    Retry {
        /// How long to wait first.
        after: Duration,
    },
    /// Wait, apply this step to the request, then send it.
    Escalate {
        /// Where this step sits on the ladder. A step that was named rather than
        /// chosen reports the length of the ladder, because there is nothing above it.
        index: usize,
        /// The step to apply.
        step: Step,
        /// How long to wait first, which is zero unless something asked this caller to
        /// slow down.
        after: Duration,
    },
    /// Stop, and say why.
    Stop(StopReason),
}

/// Why an operation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    /// The answer will not change. The hint names the one parameter worth changing if
    /// the caller disagrees.
    Rejected {
        /// What to change before asking again, when anything would help.
        hint: Hint,
    },
    /// The account has no credits left. Never retried, from any state, because every
    /// attempt after that fails the same way and some of them still cost money.
    OutOfCredits,
    /// A cap was reached.
    Budget(BudgetKind),
    /// The retries ran out on something climbing the ladder cannot fix.
    RetriesExhausted,
    /// Every step on the ladder has been tried.
    LadderExhausted,
    /// No rule named what this attempt saw, so nothing further was spent on it.
    Unhandled,
}

impl StopReason {
    /// Which cap ran out, when a cap is why this stopped.
    pub fn budget_kind(self) -> Option<BudgetKind> {
        match self {
            StopReason::Budget(kind) => Some(kind),
            _ => None,
        }
    }
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::Rejected { hint } => write!(f, "rejected, try {hint:?}"),
            StopReason::OutOfCredits => f.write_str("no credits left on the account"),
            StopReason::Budget(kind) => write!(f, "budget exceeded: {kind}"),
            StopReason::RetriesExhausted => f.write_str("out of retries"),
            StopReason::LadderExhausted => f.write_str("out of escalation steps"),
            StopReason::Unhandled => f.write_str("no rule for what came back"),
        }
    }
}

/// The label on the step that turns on keeping state between calls.
pub const SESSION_LABEL: &str = "session";

/// The least to wait after a site rate limits the fetch and names no figure of its own.
///
/// Read back out of [`Hint::for_class`] rather than written down a second time. The
/// response layer already tells a caller how long to wait for this exact class, and two
/// copies of one number in one crate drift apart. Half a second is right for the
/// service's own rate limit, which is about how often this account is calling. A site
/// that just refused the fetch deserves longer.
pub fn target_slow_down() -> Duration {
    match Hint::for_class(PageClass::TargetRateLimited) {
        Hint::SlowDown { after } => after,
        // Unreachable while the response layer maps this class to a wait, and not worth
        // a panic if it ever stops.
        _ => Duration::from_secs(5),
    }
}

/// The rules, the ladder and the retry curve, together.
///
/// A policy is data. It can be printed, compared and swapped out, and the same policy
/// that drives live calls drives the offline simulator.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// Read in order, first match wins.
    pub rules: Vec<Rule>,
    /// Where an escalation goes.
    pub ladder: Ladder,
    /// How long to wait before sending the same request again.
    pub backoff: Backoff,
    /// The most calls one operation may make, whatever a budget allows.
    pub max_attempts: u8,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy::standard()
    }
}

impl Policy {
    /// The default rules on both planes, the standard ladder, and the default retry
    /// curve.
    pub fn standard() -> Policy {
        Policy {
            rules: default_rules(),
            ladder: Ladder::standard(),
            backoff: Backoff::default(),
            max_attempts: crate::policy::budget::DEFAULT_ATTEMPTS,
        }
    }

    /// The standard policy with the country rotation read from the address being
    /// fetched.
    pub fn for_target(url: &str) -> Policy {
        Policy {
            ladder: Ladder::for_target(url),
            ..Policy::standard()
        }
    }

    /// The standard policy rotating through a caller's own countries.
    pub fn with_countries(pool: &CountryPool) -> Policy {
        Policy {
            ladder: Ladder::with_countries(pool),
            ..Policy::standard()
        }
    }

    /// Replace the retry curve.
    pub fn with_backoff(mut self, backoff: Backoff) -> Policy {
        self.backoff = backoff;
        self
    }

    /// Replace the rule table.
    pub fn with_rules(mut self, rules: Vec<Rule>) -> Policy {
        self.rules = rules;
        self
    }

    /// Replace the ceiling on calls per operation.
    pub fn with_max_attempts(mut self, max_attempts: u8) -> Policy {
        self.max_attempts = max_attempts;
        self
    }

    /// What to do next.
    ///
    /// Pure. Give it the same attempt and the same state and it answers the same way,
    /// which is what lets the simulator assert an exact sequence of moves.
    pub fn decide(&self, observed: &Observed, state: &AttemptState) -> Next {
        // Checked before the table so it holds from any state, including one with
        // retries left and steps to climb.
        if observed.api_class() == Some(ApiClass::InsufficientCredits) {
            return Next::Stop(StopReason::OutOfCredits);
        }

        let Some(rule) = self.rules.iter().find(|rule| rule.when.matches(observed)) else {
            return Next::Stop(StopReason::Unhandled);
        };

        match &rule.then {
            Decision::Accept => Next::Accept,
            // The one place a Fail is reconsidered. A login wall is the only refusal a
            // request parameter can answer, and only in the narrow shape
            // `session_step` describes.
            Decision::Fail => match self.session_step(observed) {
                Some(step) => self.escalate(observed, state, Some(step)),
                None => Next::Stop(StopReason::Rejected {
                    hint: observed.hint(),
                }),
            },
            Decision::RetrySame { max } => {
                if state.retries_at_step < *max {
                    self.retry(observed, state)
                } else if rule.when.retry_only() {
                    Next::Stop(StopReason::RetriesExhausted)
                } else {
                    self.escalate(observed, state, None)
                }
            }
            Decision::Escalate => self.escalate(observed, state, None),
            Decision::EscalateTo(step) => self.escalate(observed, state, Some(step.clone())),
        }
    }

    /// How long to wait before the next call.
    ///
    /// The curve, raised to [`target_slow_down`] when a site rate limited the fetch and
    /// sent no figure of its own. A site that did send one is honoured as it stands, up
    /// or down, and the floor never pushes a wait past [`Backoff::cap`], because a
    /// caller who set a ceiling meant it.
    fn wait_before(&self, observed: &Observed, state: &AttemptState) -> Duration {
        let curve = self
            .backoff
            .delay(u32::from(state.retries_at_step), observed.retry_after);

        if observed.page_class() == Some(PageClass::TargetRateLimited)
            && observed.retry_after.is_none()
        {
            curve.max(target_slow_down().min(self.backoff.cap))
        } else {
            curve
        }
    }

    /// What to assume the next call costs, given what the last one did.
    ///
    /// An unbilled attempt reports nothing, and scaling nothing gives nothing, so the
    /// basis drops to zero and [`Budget::floor`] picks it up from there.
    fn basis(observed: &Observed, state: &AttemptState) -> Credits {
        if observed.was_billed() {
            state.last_cost
        } else {
            Credits::ZERO
        }
    }

    /// The one step that answers a login wall, when the request is in a shape where it
    /// would do anything.
    ///
    /// `session` carries cookies and headers from one call to the next. It does not log
    /// anybody in. So it is worth a call only when the caller already supplied cookies
    /// or headers for it to carry, and has not already asked for it. With neither, the
    /// second call sends the same anonymous request and buys the same wall.
    pub fn session_step(&self, observed: &Observed) -> Option<Step> {
        if observed.page_class() != Some(PageClass::NeedsLogin)
            || !observed.sent_credentials
            || observed.sent_session
        {
            return None;
        }

        // Keeping state changes no cost driver, so the next call is priced like the
        // one before it.
        Some(Step::new(SESSION_LABEL, 1.0, vec![Rung::Session(true)]))
    }

    /// Send the same request again, if the caps leave room for it.
    fn retry(&self, observed: &Observed, state: &AttemptState) -> Next {
        let after = self.wait_before(observed, state);

        if let Err(kind) = self.room_for_another_call(state, after, Policy::basis(observed, state))
        {
            return Next::Stop(StopReason::Budget(kind));
        }

        Next::Retry { after }
    }

    /// Climb, if there is anywhere to climb to and the caps leave room for it.
    fn escalate(&self, observed: &Observed, state: &AttemptState, forced: Option<Step>) -> Next {
        let after = if observed.asked_to_slow_down() {
            self.wait_before(observed, state)
        } else {
            Duration::ZERO
        };

        let (index, step) = match forced {
            // A named step takes the operation off the ladder, so it reports the end of
            // it and there is nothing above it to climb to.
            Some(step) => (self.ladder.len(), step),
            None => {
                // Spending more on a site that is rate limiting the fetch buys the same
                // refusal at a higher price. Coming from somewhere else is the move with
                // a chance, so this jumps rather than walks.
                let index = if observed.page_class() == Some(PageClass::TargetRateLimited) {
                    self.ladder.first_geo_from(state.step)
                } else {
                    Some(state.step)
                };

                match index.and_then(|index| self.ladder.get(index).map(|step| (index, step))) {
                    Some((index, step)) => (index, step.clone()),
                    None => return Next::Stop(StopReason::LadderExhausted),
                }
            }
        };

        let estimate = step.estimate(Policy::basis(observed, state));
        if let Err(kind) = self.room_for_another_call(state, after, estimate) {
            return Next::Stop(StopReason::Budget(kind));
        }

        Next::Escalate { index, step, after }
    }

    /// Whether another call, after waiting `after` and expecting to spend `estimate`,
    /// stays inside every cap.
    fn room_for_another_call(
        &self,
        state: &AttemptState,
        after: Duration,
        estimate: Credits,
    ) -> Result<(), BudgetKind> {
        if state.attempts >= self.max_attempts {
            return Err(BudgetKind::Attempts);
        }
        state.budget.allows_attempt(state.attempts)?;
        state.budget.allows_sleep(state.elapsed, after)?;
        state.budget.allows_spend(state.spent, estimate)
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

    fn state() -> AttemptState {
        AttemptState::new(Budget::unlimited())
    }

    #[test]
    fn a_served_page_is_accepted() {
        let mut s = state();
        let seen = Observed::seen(200, Some(200));
        s.record(&seen);
        assert_eq!(Policy::standard().decide(&seen, &s), Next::Accept);
    }

    #[test]
    fn a_blank_served_page_climbs_instead() {
        let mut s = state();
        let seen = Observed::seen(200, Some(200)).blank().costing(Credits(2.0));
        s.record(&seen);
        match Policy::standard().decide(&seen, &s) {
            Next::Escalate { step, .. } => assert_eq!(step.label, "browser"),
            other => panic!("a blank page was not escalated: {other:?}"),
        }
    }

    #[test]
    fn nothing_to_return_is_a_result_not_a_blank_page() {
        let mut s = state();
        let seen = Observed::seen(204, None).blank();
        s.record(&seen);
        assert!(!seen.is_blank_success());
        assert_eq!(Policy::standard().decide(&seen, &s), Next::Accept);
    }

    #[test]
    fn a_call_that_succeeded_around_a_refused_fetch_is_judged_on_the_fetch() {
        let seen = Observed::seen(200, Some(403));
        assert!(!Trigger::Api(ApiClass::Ok).matches(&seen));
        assert!(Trigger::Page(PageClass::Blocked).matches(&seen));
    }

    #[test]
    fn a_rate_limit_written_without_a_wait_matches_one_that_carried_a_header() {
        let seen = Observed::seen(429, None).retry_after(Duration::from_secs(9));
        assert!(Trigger::Api(ApiClass::RateLimited { retry_after: None }).matches(&seen));
    }

    #[test]
    fn a_site_rate_limit_jumps_to_the_country_step_rather_than_walking() {
        let policy = Policy::standard();
        let mut s = state();
        let seen = Observed::seen(200, Some(429)).costing(Credits(1.0));
        s.record(&seen);
        // First it retries the same request.
        let first = policy.decide(&seen, &s);
        assert!(matches!(first, Next::Retry { .. }), "{first:?}");
        s.follow(&first);
        s.record(&seen);
        // Then it moves country, skipping the two steps that only cost more.
        match policy.decide(&seen, &s) {
            Next::Escalate { index, step, .. } => {
                assert_eq!(index, 3);
                assert_eq!(step.label, "residential+geo");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_accounts_own_rate_limit_never_climbs() {
        let policy = Policy::standard().with_max_attempts(u8::MAX);
        let mut s = state();
        let seen = Observed::seen(429, None);
        s.record(&seen);
        for _ in 0..3 {
            let next = policy.decide(&seen, &s);
            assert!(matches!(next, Next::Retry { .. }), "{next:?}");
            s.follow(&next);
            s.record(&seen);
        }
        assert_eq!(
            policy.decide(&seen, &s),
            Next::Stop(StopReason::RetriesExhausted)
        );
    }

    #[test]
    fn a_timeout_retries_once_then_climbs() {
        let policy = Policy::standard();
        let mut s = state();
        let seen = Observed::timed_out();
        s.record(&seen);
        let first = policy.decide(&seen, &s);
        assert!(matches!(first, Next::Retry { .. }), "{first:?}");
        s.follow(&first);
        s.record(&seen);
        match policy.decide(&seen, &s) {
            Next::Escalate { step, .. } => assert_eq!(step.label, "browser"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_login_wall_stops_with_the_session_hint() {
        let mut s = state();
        let seen = Observed::seen(200, Some(401));
        s.record(&seen);
        assert_eq!(
            Policy::standard().decide(&seen, &s),
            Next::Stop(StopReason::Rejected {
                hint: Hint::NeedsSession
            })
        );
    }

    #[test]
    fn the_ladder_runs_out_before_the_policy_pretends_otherwise() {
        let policy = Policy::standard().with_max_attempts(u8::MAX);
        let mut s = state();
        s.step = policy.ladder.len();
        let seen = Observed::seen(200, Some(403));
        s.record(&seen);
        assert_eq!(
            policy.decide(&seen, &s),
            Next::Stop(StopReason::LadderExhausted)
        );
    }

    #[test]
    fn an_unnamed_status_stops_rather_than_spends() {
        let mut s = state();
        let seen = Observed::seen(418, None);
        s.record(&seen);
        assert_eq!(
            Policy::standard().decide(&seen, &s),
            Next::Stop(StopReason::Unhandled)
        );
    }

    #[test]
    fn the_policy_ceiling_stops_an_operation_a_generous_budget_would_not() {
        let policy = Policy::standard().with_max_attempts(2);
        let mut s = state();
        s.attempts = 2;
        let seen = Observed::seen(200, Some(403));
        assert_eq!(
            policy.decide(&seen, &s),
            Next::Stop(StopReason::Budget(BudgetKind::Attempts))
        );
    }

    #[test]
    fn a_named_step_reports_the_end_of_the_ladder() {
        let step = Step::new("named", 2.0, Vec::new());
        let policy = Policy::standard().with_rules(vec![Rule::new(
            Trigger::Page(PageClass::Blocked),
            Decision::EscalateTo(step.clone()),
        )]);
        let mut s = state();
        let seen = Observed::seen(200, Some(403));
        s.record(&seen);
        match policy.decide(&seen, &s) {
            Next::Escalate {
                index, step: got, ..
            } => {
                assert_eq!(index, policy.ladder.len());
                assert_eq!(got, step);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn following_a_decision_moves_the_counters() {
        let mut s = state();
        s.follow(&Next::Retry {
            after: Duration::from_secs(2),
        });
        assert_eq!(s.retries_at_step, 1);
        assert_eq!(s.elapsed, Duration::from_secs(2));

        s.follow(&Next::Escalate {
            index: 2,
            step: Step::new("x", 1.0, Vec::new()),
            after: Duration::from_secs(1),
        });
        assert_eq!(s.step, 3);
        assert_eq!(s.retries_at_step, 0);
        assert_eq!(s.elapsed, Duration::from_secs(3));
    }

    #[test]
    fn recording_an_attempt_adds_up_the_spend_and_the_clock() {
        let mut s = state();
        s.record(
            &Observed::seen(200, Some(403))
                .costing(Credits(3.0))
                .taking(Duration::from_secs(1)),
        );
        s.record(
            &Observed::seen(200, Some(403))
                .costing(Credits(4.0))
                .taking(Duration::from_secs(2)),
        );
        assert_eq!(s.attempts, 2);
        assert_eq!(s.spent, Credits(7.0));
        assert_eq!(s.last_cost, Credits(4.0));
        assert_eq!(s.elapsed, Duration::from_secs(3));
    }

    #[test]
    fn a_cost_that_cannot_be_a_bill_is_counted_as_nothing() {
        let mut s = state();
        s.record(&Observed::seen(200, Some(403)).costing(Credits(3.0)));
        s.record(&Observed::seen(200, Some(403)).costing(Credits(-100.0)));
        assert_eq!(s.spent, Credits(3.0));
        assert_eq!(s.last_cost, Credits::ZERO);

        s.record(&Observed::seen(200, Some(403)).costing(Credits(f64::NAN)));
        assert_eq!(s.spent, Credits(3.0));
        s.record(&Observed::seen(200, Some(403)).costing(Credits(f64::INFINITY)));
        assert_eq!(s.spent, Credits(3.0));
        assert_eq!(s.attempts, 4);
    }

    #[test]
    fn the_unbilled_statuses_are_the_ones_the_response_layer_names() {
        // Only target 500 and 503 are free, and a call that reached no site is free too.
        assert!(!Observed::seen(200, Some(500)).was_billed());
        assert!(!Observed::seen(200, Some(503)).was_billed());
        assert!(!Observed::seen(500, None).was_billed());
        assert!(!Observed::timed_out().was_billed());

        for page in [200u16, 403, 404, 429, 502, 504] {
            assert!(
                Observed::seen(200, Some(page)).was_billed(),
                "target {page} should be billed"
            );
        }
        assert!(Observed::seen(200, None).was_billed());
    }

    #[test]
    fn an_escalation_after_a_free_attempt_is_priced_off_the_floor() {
        let policy = Policy::standard().with_max_attempts(u8::MAX);
        let floor = ASSUMED_MINIMUM_COST.get();
        let mut s =
            AttemptState::new(Budget::unlimited().with_per_page_credits(Credits(floor * 3.0)));
        // A free target 5xx reports no cost, so the basis drops to the floor and the
        // cheapest step estimates four floors, over a cap of three.
        let seen = Observed::seen(200, Some(503));
        s.record(&seen);
        s.retries_at_step = 1;

        assert_eq!(
            policy.decide(&seen, &s),
            Next::Stop(StopReason::Budget(BudgetKind::Credits))
        );
    }

    #[test]
    fn a_cost_reported_on_an_unbilled_attempt_is_not_used_as_a_basis() {
        // Target 500 and 503 are free. If a response reports a cost on one anyway,
        // scaling it would price every later step off a figure the account was never
        // charged, so the basis drops to the floor instead.
        let policy = Policy::standard().with_max_attempts(u8::MAX);
        let floor = ASSUMED_MINIMUM_COST.get();
        let mut s =
            AttemptState::new(Budget::unlimited().with_per_page_credits(Credits(floor * 10.0)));
        let seen = Observed::seen(200, Some(503)).costing(Credits(floor * 4.0));
        s.record(&seen);
        s.retries_at_step = 1;

        // The floor gives four floors for the cheapest step, inside the cap. Scaling
        // the reported cost would give sixteen, and refuse a step the account can
        // afford.
        match policy.decide(&seen, &s) {
            Next::Escalate { step, .. } => {
                assert_eq!(step.label, "browser");
                assert_eq!(step.estimate(Credits::ZERO), Credits(floor * 4.0));
            }
            other => panic!("the free attempt priced the next step: {other:?}"),
        }
    }

    #[test]
    fn a_stop_reason_prints_and_names_its_cap() {
        assert_eq!(
            StopReason::Budget(BudgetKind::Credits).budget_kind(),
            Some(BudgetKind::Credits)
        );
        assert_eq!(StopReason::OutOfCredits.budget_kind(), None);
        assert_eq!(
            StopReason::Budget(BudgetKind::Time).to_string(),
            "budget exceeded: time"
        );
    }
}
