//! What an operation is allowed to spend, in credits, in time and in calls.
//!
//! The budget is enforced twice. It is mirrored onto the request, so the service
//! refuses to run past the cap even if this client stops asking, and it is checked here
//! before every retry and every escalation, so the client stops before it sends
//! something it cannot afford.

use crate::credits::{Credits, WholeCredits};
use crate::error::BudgetKind;
use crate::params::RequestParams;
use std::time::Duration;

/// The caps an operation runs under.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// The most the whole operation may spend. `None` leaves it to the account
    /// balance.
    pub credits: Option<Credits>,
    /// The most any one page may spend.
    pub per_page_credits: Option<Credits>,
    /// The longest the whole operation may take, sleeps included.
    pub wall: Option<Duration>,
    /// The most calls the operation may make.
    pub attempts: u8,
}

/// How many calls an operation makes when nobody said otherwise: the first one plus a
/// walk of the standard ladder.
pub const DEFAULT_ATTEMPTS: u8 = 5;

/// The spend to assume for one page when the last attempt reported none.
///
/// This is an assumption, not a price. It exists because a step estimate is the last
/// observed cost scaled by the step, and some attempts are free: a target 500 and a
/// target 503 are not billed, and a call that never reached a site is not billed
/// either. Scaling zero gives zero, so without a floor the credit cap could refuse
/// nothing on exactly the failure most likely to start an escalation.
///
/// The floor is there to stop a cap from going blind, not to predict a bill, so it
/// errs toward letting a call through rather than refusing one the account could have
/// afforded. Single fetches of example.com measured against a live account on
/// 2026-09-15 cost 0.109 to 0.191 credits, plain and rendered alike, so a tenth of a
/// credit sits under all of them.
///
/// It was one credit until the unit bug was found. That value was not compensating for
/// the bug: the floor was already written in credits while every cost read off the wire
/// was in dollars, which is the mismatch itself. Read against real prices it was about
/// ten times a cheap fetch, which refuses rather than lets through, so it contradicted
/// the rule above.
pub const ASSUMED_MINIMUM_COST: Credits = Credits::new(0.1);

impl Default for Budget {
    /// No credit or time cap, and five calls.
    fn default() -> Budget {
        Budget {
            credits: None,
            per_page_credits: None,
            wall: None,
            attempts: DEFAULT_ATTEMPTS,
        }
    }
}

impl Budget {
    /// A budget that stops on nothing but the account balance.
    pub fn unlimited() -> Budget {
        Budget {
            attempts: u8::MAX,
            ..Budget::default()
        }
    }

    /// Cap what the whole operation may spend.
    pub fn with_credits(mut self, credits: Credits) -> Budget {
        self.credits = Some(credits);
        self
    }

    /// Cap what any one page may spend.
    pub fn with_per_page_credits(mut self, credits: Credits) -> Budget {
        self.per_page_credits = Some(credits);
        self
    }

    /// Cap how long the operation may take, sleeps included.
    pub fn with_wall(mut self, wall: Duration) -> Budget {
        self.wall = Some(wall);
        self
    }

    /// Cap how many calls the operation may make.
    pub fn with_attempts(mut self, attempts: u8) -> Budget {
        self.attempts = attempts;
        self
    }

    /// Copy the credit caps onto a request.
    ///
    /// The whole operation cap rounds down to a whole number on the way out, because
    /// the service takes that one as a `u64` and answers a decimal with a 400. The per
    /// page cap keeps its fraction.
    ///
    /// The service reads both as a byte ceiling rather than as a spend ceiling, so
    /// sending them is a second line and not a guarantee. A crawl of 100 pages under a
    /// cap of one credit ran to the end and cost 172 credits when this was measured on
    /// 2026-09-15. The cap this client keeps for itself is the one that stops a run.
    ///
    /// Anything already set on the request is left alone. A caller who set a tighter
    /// cap by hand keeps it.
    pub fn apply(&self, params: &mut RequestParams) {
        if params.max_credits_allowed.is_none() {
            params.max_credits_allowed = self.credits.map(WholeCredits::floor);
        }
        if params.max_credits_per_page.is_none() {
            params.max_credits_per_page = self.per_page_credits;
        }
    }

    /// What is left of the credit cap after `spent`, or `None` when there is no cap.
    pub fn remaining_credits(&self, spent: Credits) -> Option<Credits> {
        self.credits
            .map(|cap| Credits((cap.get() - spent.get()).max(0.0)))
    }

    /// What is left of the time cap after `elapsed`, or `None` when there is no cap.
    pub fn remaining_wall(&self, elapsed: Duration) -> Option<Duration> {
        self.wall.map(|cap| cap.saturating_sub(elapsed))
    }

    /// Whether another call fits in the attempt count.
    pub fn allows_attempt(&self, made: u8) -> Result<(), BudgetKind> {
        if made >= self.attempts {
            return Err(BudgetKind::Attempts);
        }
        Ok(())
    }

    /// Raise a basis of nothing to [`ASSUMED_MINIMUM_COST`].
    ///
    /// An unbilled attempt reports no cost, and a cap fed an estimate of zero can
    /// refuse nothing. Anything that scales an observed cost runs it through here
    /// first, so a free attempt does not turn the cap off.
    pub fn floor(basis: Credits) -> Credits {
        if basis.get() > 0.0 {
            basis
        } else {
            ASSUMED_MINIMUM_COST
        }
    }

    /// Whether spending `estimate` more, after already spending `spent`, stays inside
    /// the caps.
    ///
    /// An estimate of nothing is read as [`ASSUMED_MINIMUM_COST`] rather than as free,
    /// so the run after an unbilled attempt is still held to the cap. See
    /// [`Budget::floor`].
    pub fn allows_spend(&self, spent: Credits, estimate: Credits) -> Result<(), BudgetKind> {
        let estimate = Budget::floor(estimate);

        if let Some(per_page) = self.per_page_credits {
            if estimate.get() > per_page.get() {
                return Err(BudgetKind::Credits);
            }
        }
        if let Some(cap) = self.credits {
            if spent.get() + estimate.get() > cap.get() {
                return Err(BudgetKind::Credits);
            }
        }
        Ok(())
    }

    /// Whether waiting `sleep` and then making another call still fits in the time
    /// cap.
    ///
    /// Sleeping out the whole remaining budget and then calling with no time left is
    /// the failure this stops, which is what a run of rate limits does to a caller who
    /// only checks the clock afterwards.
    pub fn allows_sleep(&self, elapsed: Duration, sleep: Duration) -> Result<(), BudgetKind> {
        match self.wall {
            Some(cap) if elapsed.saturating_add(sleep) >= cap => Err(BudgetKind::Time),
            _ => Ok(()),
        }
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
    use crate::credits::Usd;
    use crate::response::costs::Costs;

    #[test]
    fn the_caps_are_copied_onto_the_request() {
        let budget = Budget::default()
            .with_credits(Credits(500.0))
            .with_per_page_credits(Credits(50.0));
        let mut params = RequestParams::url("https://example.com");
        budget.apply(&mut params);

        assert_eq!(params.max_credits_allowed, Some(WholeCredits::new(500)));
        assert_eq!(params.max_credits_per_page, Some(Credits(50.0)));
    }

    #[test]
    fn a_tighter_cap_set_by_hand_survives() {
        let budget = Budget::default().with_credits(Credits(500.0));
        let mut params = RequestParams::url("https://example.com");
        params.max_credits_allowed = Some(WholeCredits::new(10));
        budget.apply(&mut params);

        assert_eq!(params.max_credits_allowed, Some(WholeCredits::new(10)));
    }

    #[test]
    fn spend_is_refused_once_the_estimate_runs_past_the_cap() {
        let budget = Budget::default().with_credits(Credits(100.0));
        assert_eq!(budget.allows_spend(Credits(60.0), Credits(30.0)), Ok(()));
        assert_eq!(
            budget.allows_spend(Credits(60.0), Credits(50.0)),
            Err(BudgetKind::Credits)
        );
    }

    #[test]
    fn the_per_page_cap_refuses_one_expensive_step_on_its_own() {
        let budget = Budget::unlimited().with_per_page_credits(Credits(20.0));
        assert_eq!(budget.allows_spend(Credits::ZERO, Credits(19.0)), Ok(()));
        assert_eq!(
            budget.allows_spend(Credits::ZERO, Credits(21.0)),
            Err(BudgetKind::Credits)
        );
    }

    #[test]
    fn a_free_attempt_does_not_turn_the_cap_off() {
        assert_eq!(Budget::floor(Credits::ZERO), ASSUMED_MINIMUM_COST);
        assert_eq!(Budget::floor(Credits(4.0)), Credits(4.0));

        // Without the floor this reads as spending nothing and always fits. The
        // cap is written against the floor rather than as a number, so moving
        // the floor does not quietly change what this proves.
        let budget =
            Budget::unlimited().with_per_page_credits(Credits(ASSUMED_MINIMUM_COST.get() / 2.0));
        assert_eq!(
            budget.allows_spend(Credits::ZERO, Credits::ZERO),
            Err(BudgetKind::Credits)
        );
    }

    /// The defect this pins. `Costs::total` used to hand back the wire number
    /// unconverted, so a real page read as 1.09e-5 against a cap in credits and
    /// no credit budget could ever trip. These are the costs measured against a
    /// live account on 2026-09-15, in the unit the cap is written in.
    #[test]
    fn a_credit_cap_refuses_a_real_measured_spend_that_runs_past_it() {
        let cheap = Costs {
            total_cost: Usd::new(1.0870316666666668e-05),
            ..Costs::default()
        }
        .total();
        assert!((cheap.get() - 0.108703).abs() < 1e-6, "{cheap}");

        // A crawl page measured at 1.73 credits, against a cap of 2.
        let page = Credits::from_usd(1.7282492833333333e-04);
        let budget = Budget::default().with_credits(Credits(2.0));

        assert_eq!(budget.allows_spend(Credits::ZERO, page), Ok(()));
        assert_eq!(
            budget.allows_spend(page, page),
            Err(BudgetKind::Credits),
            "two of these is more than the cap allows"
        );
        assert_eq!(
            budget.allows_spend(Credits::ZERO, Credits(2.5)),
            Err(BudgetKind::Credits)
        );

        // Read as the wire sends it, every one of these fits, which is the bug.
        assert_eq!(
            budget.allows_spend(Credits::ZERO, Credits(1.7282492833333333e-04)),
            Ok(())
        );
    }

    #[test]
    fn nothing_is_capped_when_nothing_was_capped() {
        let budget = Budget::unlimited();
        assert_eq!(budget.allows_spend(Credits(1e9), Credits(1e9)), Ok(()));
        assert_eq!(budget.remaining_credits(Credits(5.0)), None);
        assert_eq!(budget.remaining_wall(Duration::from_secs(5)), None);
        assert_eq!(
            budget.allows_sleep(Duration::from_secs(86_400), Duration::from_secs(86_400)),
            Ok(())
        );
    }

    #[test]
    fn a_sleep_that_would_eat_the_clock_is_refused() {
        let budget = Budget::default().with_wall(Duration::from_secs(10));
        assert_eq!(
            budget.allows_sleep(Duration::from_secs(2), Duration::from_secs(3)),
            Ok(())
        );
        assert_eq!(
            budget.allows_sleep(Duration::from_secs(8), Duration::from_secs(3)),
            Err(BudgetKind::Time)
        );
    }

    #[test]
    fn the_attempt_count_runs_out() {
        let budget = Budget::default().with_attempts(2);
        assert_eq!(budget.allows_attempt(0), Ok(()));
        assert_eq!(budget.allows_attempt(1), Ok(()));
        assert_eq!(budget.allows_attempt(2), Err(BudgetKind::Attempts));
    }

    #[test]
    fn remaining_never_goes_below_zero() {
        let budget = Budget::default().with_credits(Credits(10.0));
        assert_eq!(budget.remaining_credits(Credits(25.0)), Some(Credits::ZERO));
        assert_eq!(
            Budget::default()
                .with_wall(Duration::from_secs(1))
                .remaining_wall(Duration::from_secs(9)),
            Some(Duration::ZERO)
        );
    }
}
