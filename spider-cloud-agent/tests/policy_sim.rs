// Integration tests are their own crate root, so the allow blocks inside the
// library's test modules do not reach here. A test that cannot set itself up
// should stop loudly rather than quietly measure nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::string_slice
)]

//! An offline simulator for the escalation policy.
//!
//! Nothing here opens a socket. A test writes down what the service and the site are
//! going to say, the simulator drives [`Policy::decide`] over that script, and the test
//! asserts the exact sequence of moves and what the run spent.
//!
//! That is the point of keeping the policy pure. A strategy that only runs inside a
//! network loop can be argued about. This one can be checked.

use spider_cloud_agent::params::{Country, ProxyPool, RequestMode, RequestParams};
use spider_cloud_agent::policy::{
    target_slow_down, AttemptState, Budget, Ladder, Next, Observed, Policy, StopReason,
    ASSUMED_MINIMUM_COST, SESSION_LABEL,
};
use spider_cloud_agent::response::Hint;
use spider_cloud_agent::{BudgetKind, Credits, WholeCredits};
use std::time::Duration;

/// One scripted answer to one attempt.
#[derive(Debug, Clone, Copy)]
struct Scripted {
    api: u16,
    page: Option<u16>,
    empty: bool,
    cost: f64,
    elapsed: Duration,
    retry_after: Option<Duration>,
    reached: Reach,
}

/// How far the scripted attempt got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    Service,
    TimedOut,
    ConnectFailed,
}

impl Scripted {
    /// The service and the site both answered.
    fn seen(api: u16, page: Option<u16>) -> Scripted {
        Scripted {
            api,
            page,
            empty: false,
            cost: 0.0,
            elapsed: Duration::from_millis(200),
            retry_after: None,
            reached: Reach::Service,
        }
    }

    fn timed_out() -> Scripted {
        Scripted {
            reached: Reach::TimedOut,
            ..Scripted::seen(0, None)
        }
    }

    fn connect_failed() -> Scripted {
        Scripted {
            reached: Reach::ConnectFailed,
            ..Scripted::seen(0, None)
        }
    }

    fn blank(mut self) -> Scripted {
        self.empty = true;
        self
    }

    fn costing(mut self, cost: f64) -> Scripted {
        self.cost = cost;
        self
    }

    fn retry_after(mut self, after: Duration) -> Scripted {
        self.retry_after = Some(after);
        self
    }

    fn observed(&self) -> Observed {
        let mut observed = match self.reached {
            Reach::Service => Observed::seen(self.api, self.page),
            Reach::TimedOut => Observed::timed_out(),
            Reach::ConnectFailed => Observed::connect_failed(),
        };
        if self.empty {
            observed = observed.blank();
        }
        observed = observed.costing(Credits(self.cost)).taking(self.elapsed);
        if let Some(after) = self.retry_after {
            observed = observed.retry_after(after);
        }
        observed
    }
}

/// One move, flattened to what a test wants to assert.
#[derive(Debug, Clone, PartialEq)]
enum Move {
    Accept,
    Retry(Duration),
    Escalate(&'static str, Duration),
    Stop(StopReason),
}

impl Move {
    fn of(next: &Next) -> Move {
        match next {
            Next::Accept => Move::Accept,
            Next::Retry { after } => Move::Retry(*after),
            Next::Escalate { step, after, .. } => Move::Escalate(step.label, *after),
            Next::Stop(reason) => Move::Stop(*reason),
        }
    }
}

/// What a whole run produced.
#[derive(Debug)]
struct Run {
    moves: Vec<Move>,
    spent: Credits,
    slept: Duration,
    attempts: u8,
    /// The request as it stood when the run ended, with every applied step written on.
    params: RequestParams,
}

/// Drive a policy over a script until it accepts or stops.
///
/// Running past the end of the script panics rather than repeating the last answer,
/// because a policy asking for more calls than the test wrote down is the bug the test
/// is there to find.
fn run(policy: &Policy, budget: Budget, script: &[Scripted]) -> Run {
    let mut state = AttemptState::new(budget);
    let mut params = RequestParams::url("https://example.com/thing");
    budget.apply(&mut params);

    let mut moves = Vec::new();
    let mut slept = Duration::ZERO;

    for (index, scripted) in script.iter().enumerate() {
        let observed = scripted.observed();
        state.record(&observed);

        let next = policy.decide(&observed, &state);
        moves.push(Move::of(&next));

        match &next {
            Next::Retry { after } => slept += *after,
            Next::Escalate { step, after, .. } => {
                slept += *after;
                step.apply(&mut params);
            }
            Next::Accept | Next::Stop(_) => {
                state.follow(&next);
                return Run {
                    moves,
                    spent: state.spent,
                    slept,
                    attempts: state.attempts,
                    params,
                };
            }
        }

        state.follow(&next);
        assert!(
            index + 1 < script.len(),
            "the policy asked for call {} and the script has {}",
            index + 2,
            script.len()
        );
    }

    unreachable!("the loop returns on the first accept or stop")
}

/// The first move a policy makes on one scripted answer, from a fresh state.
fn first_move(policy: &Policy, scripted: Scripted) -> Move {
    let mut state = AttemptState::new(Budget::unlimited());
    let observed = scripted.observed();
    state.record(&observed);
    Move::of(&policy.decide(&observed, &state))
}

/// The default retry curve's first wait, which several cells below land on.
const FIRST_WAIT: Duration = Duration::from_millis(500);

/// The floor on a wait after a site rate limits the fetch.
const SLOW_DOWN: Duration = Duration::from_secs(5);

#[test]
fn every_cell_of_the_call_plane_table() {
    let policy = Policy::standard();
    let cases: Vec<(&str, Scripted, Move)> = vec![
        ("200", Scripted::seen(200, None), Move::Accept),
        ("204", Scripted::seen(204, None), Move::Accept),
        (
            "400",
            Scripted::seen(400, None),
            Move::Stop(StopReason::Rejected {
                hint: Hint::Permanent,
            }),
        ),
        (
            "401",
            Scripted::seen(401, None),
            Move::Stop(StopReason::Rejected {
                hint: Hint::Permanent,
            }),
        ),
        (
            "402",
            Scripted::seen(402, None),
            Move::Stop(StopReason::OutOfCredits),
        ),
        (
            "413",
            Scripted::seen(413, None),
            Move::Stop(StopReason::Rejected {
                hint: Hint::Permanent,
            }),
        ),
        ("429", Scripted::seen(429, None), Move::Retry(FIRST_WAIT)),
        ("500", Scripted::seen(500, None), Move::Retry(FIRST_WAIT)),
        ("503", Scripted::seen(503, None), Move::Retry(FIRST_WAIT)),
        ("timeout", Scripted::timed_out(), Move::Retry(FIRST_WAIT)),
        (
            "connect",
            Scripted::connect_failed(),
            Move::Retry(FIRST_WAIT),
        ),
    ];
    assert_eq!(cases.len(), 11, "a cell was added or dropped");

    for (name, scripted, expected) in cases {
        assert_eq!(first_move(&policy, scripted), expected, "call plane {name}");
    }
}

#[test]
fn every_cell_of_the_page_plane_table() {
    let policy = Policy::standard();
    let blocked = Move::Escalate("browser", Duration::ZERO);
    let permanent = Move::Stop(StopReason::Rejected {
        hint: Hint::Permanent,
    });

    let cases: Vec<(&str, Scripted, Move)> = vec![
        ("2xx", Scripted::seen(200, Some(200)), Move::Accept),
        (
            "2xx, no content",
            Scripted::seen(200, Some(200)).blank(),
            blocked.clone(),
        ),
        ("400", Scripted::seen(200, Some(400)), permanent.clone()),
        (
            "401",
            Scripted::seen(200, Some(401)),
            Move::Stop(StopReason::Rejected {
                hint: Hint::NeedsSession,
            }),
        ),
        ("403", Scripted::seen(200, Some(403)), blocked),
        ("404", Scripted::seen(200, Some(404)), permanent),
        (
            "429",
            Scripted::seen(200, Some(429)),
            Move::Retry(SLOW_DOWN),
        ),
        (
            "5xx",
            Scripted::seen(200, Some(503)),
            Move::Retry(FIRST_WAIT),
        ),
    ];
    assert_eq!(cases.len(), 8, "a cell was added or dropped");

    for (name, scripted, expected) in cases {
        assert_eq!(first_move(&policy, scripted), expected, "page plane {name}");
    }
}

#[test]
fn a_call_plane_five_hundred_retries_once_then_climbs() {
    let run = run(
        &Policy::standard(),
        Budget::unlimited(),
        &[
            Scripted::seen(500, None),
            Scripted::seen(500, None),
            Scripted::seen(200, Some(200)),
        ],
    );
    assert_eq!(
        run.moves,
        [
            Move::Retry(FIRST_WAIT),
            Move::Escalate("browser", Duration::ZERO),
            Move::Accept,
        ]
    );
}

#[test]
fn a_call_that_never_answered_retries_once_then_climbs() {
    for scripted in [Scripted::timed_out(), Scripted::connect_failed()] {
        let run = run(
            &Policy::standard(),
            Budget::unlimited(),
            &[scripted, scripted, Scripted::seen(200, Some(200))],
        );
        assert_eq!(
            run.moves,
            [
                Move::Retry(FIRST_WAIT),
                Move::Escalate("browser", Duration::ZERO),
                Move::Accept,
            ],
            "{scripted:?}"
        );
    }
}

#[test]
fn the_accounts_own_rate_limit_retries_three_times_and_never_climbs() {
    let limited = Scripted::seen(429, None);
    let run = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited(),
        &[limited; 4],
    );
    assert_eq!(
        run.moves,
        [
            Move::Retry(Duration::from_millis(500)),
            Move::Retry(Duration::from_secs(1)),
            Move::Retry(Duration::from_secs(2)),
            Move::Stop(StopReason::RetriesExhausted),
        ]
    );
    assert_eq!(run.params.request, None, "nothing was escalated");
}

#[test]
fn a_blocked_page_walks_the_ladder_and_stops_when_the_credits_run_out() {
    let blocked = Scripted::seen(200, Some(403)).costing(5.0);
    let policy = Policy::standard().with_max_attempts(u8::MAX);

    // Estimates are the last cost scaled by the step: 20, 25, 40, then 45. Sixty
    // credits pays for the first three and refuses the fourth before sending it.
    let tight = run(
        &policy,
        Budget::unlimited()
            .with_credits(Credits(60.0))
            .with_attempts(u8::MAX),
        &[blocked; 4],
    );
    assert_eq!(
        tight.moves,
        [
            Move::Escalate("browser", Duration::ZERO),
            Move::Escalate("browser+wait", Duration::ZERO),
            Move::Escalate("residential", Duration::ZERO),
            Move::Stop(StopReason::Budget(BudgetKind::Credits)),
        ]
    );
    assert_eq!(tight.attempts, 4);
    assert_eq!(tight.spent, Credits(20.0));

    // With room to finish, the same script walks every step and then says so, rather
    // than sending a fifth call it has nowhere to send.
    let roomy = run(
        &policy,
        Budget::unlimited().with_credits(Credits(10_000.0)),
        &[blocked; 5],
    );
    assert_eq!(
        roomy.moves,
        [
            Move::Escalate("browser", Duration::ZERO),
            Move::Escalate("browser+wait", Duration::ZERO),
            Move::Escalate("residential", Duration::ZERO),
            Move::Escalate("residential+geo", Duration::ZERO),
            Move::Stop(StopReason::LadderExhausted),
        ]
    );
    assert_eq!(roomy.spent, Credits(25.0));

    // The request the walk built is the one the steps describe, and nothing else.
    assert_eq!(roomy.params.request, Some(RequestMode::Browser));
    assert_eq!(roomy.params.proxy, Some(ProxyPool::Residential));
    assert_eq!(
        roomy.params.country_code.as_ref().map(Country::as_str),
        Some("us")
    );
    assert!(roomy.params.wait_for.is_some());
    assert_eq!(roomy.params.stealth, None);
    assert_eq!(roomy.params.fingerprint, None);
    assert_eq!(roomy.params.user_agent, None);
}

#[test]
fn a_walk_can_end_on_a_page_the_heavier_request_got() {
    let blocked = Scripted::seen(200, Some(403)).costing(5.0);
    let run = run(
        &Policy::standard(),
        Budget::unlimited(),
        &[
            blocked,
            blocked,
            Scripted::seen(200, Some(200)).costing(40.0),
        ],
    );
    assert_eq!(
        run.moves,
        [
            Move::Escalate("browser", Duration::ZERO),
            Move::Escalate("browser+wait", Duration::ZERO),
            Move::Accept,
        ]
    );
    assert_eq!(run.spent, Credits(50.0));
}

#[test]
fn running_out_of_credits_never_retries_and_never_climbs_from_any_state() {
    let policy = Policy::standard().with_max_attempts(u8::MAX);
    let observed = Scripted::seen(402, None).observed();

    for step in 0..=policy.ladder.len() {
        for retries in 0..4u8 {
            let mut state = AttemptState::new(Budget::unlimited());
            state.step = step;
            state.retries_at_step = retries;
            state.attempts = 1;
            state.last_cost = Credits(5.0);
            state.spent = Credits(5.0);

            assert_eq!(
                policy.decide(&observed, &state),
                Next::Stop(StopReason::OutOfCredits),
                "step {step}, retries {retries}"
            );
        }
    }
}

#[test]
fn running_out_of_credits_ends_a_walk_that_was_going_well() {
    let run = run(
        &Policy::standard(),
        Budget::unlimited(),
        &[
            Scripted::seen(200, Some(403)).costing(5.0),
            Scripted::seen(402, None),
        ],
    );
    assert_eq!(
        run.moves,
        [
            Move::Escalate("browser", Duration::ZERO),
            Move::Stop(StopReason::OutOfCredits),
        ]
    );
    assert_eq!(run.attempts, 2);
}

#[test]
fn a_page_that_is_not_there_is_never_retried_and_never_escalated() {
    let policy = Policy::standard().with_max_attempts(u8::MAX);
    let observed = Scripted::seen(200, Some(404)).costing(1.0).observed();
    let expected = Next::Stop(StopReason::Rejected {
        hint: Hint::Permanent,
    });

    for step in 0..policy.ladder.len() {
        for retries in 0..3u8 {
            let mut state = AttemptState::new(Budget::unlimited());
            state.step = step;
            state.retries_at_step = retries;
            state.attempts = 1;
            state.last_cost = Credits(1.0);

            assert_eq!(
                policy.decide(&observed, &state),
                expected,
                "step {step}, retries {retries}"
            );
        }
    }
}

#[test]
fn a_rate_limit_sleeps_exactly_what_the_service_asked_for() {
    let asked = Duration::from_secs(7);
    let run = run(
        &Policy::standard(),
        Budget::unlimited(),
        &[
            Scripted::seen(429, None).retry_after(asked),
            Scripted::seen(200, Some(200)),
        ],
    );
    assert_eq!(run.moves, [Move::Retry(asked), Move::Accept]);
    assert_eq!(run.slept, asked);
}

#[test]
fn the_clock_budget_bounds_the_total_sleep() {
    let asked = Duration::from_secs(7);
    let limited = Scripted::seen(429, None).retry_after(asked);

    let run = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited().with_wall(Duration::from_secs(10)),
        &[limited; 2],
    );

    // One sleep fits inside ten seconds. A second would run the clock out before the
    // call it is waiting for, so the run stops instead of sleeping again.
    assert_eq!(
        run.moves,
        [
            Move::Retry(asked),
            Move::Stop(StopReason::Budget(BudgetKind::Time)),
        ]
    );
    assert_eq!(run.slept, asked);
    assert!(run.slept < Duration::from_secs(10));
}

#[test]
fn a_run_of_rate_limits_cannot_hang_a_caller() {
    let limited = Scripted::seen(429, None).retry_after(Duration::from_secs(30));
    let wall = Duration::from_secs(60);

    let run = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited().with_wall(wall),
        &[limited; 2],
    );
    assert!(run.slept < wall, "slept {:?} of {wall:?}", run.slept);
    assert_eq!(
        run.moves.last(),
        Some(&Move::Stop(StopReason::Budget(BudgetKind::Time)))
    );
}

#[test]
fn a_site_rate_limit_moves_country_rather_than_buying_a_heavier_refusal() {
    let limited = Scripted::seen(200, Some(429)).costing(2.0);
    let run = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited(),
        &[
            limited,
            limited,
            Scripted::seen(200, Some(200)).costing(9.0),
        ],
    );

    assert_eq!(
        run.moves,
        [
            Move::Retry(SLOW_DOWN),
            Move::Escalate("residential+geo", SLOW_DOWN),
            Move::Accept,
        ]
    );
    assert_eq!(
        run.params.country_code.as_ref().map(Country::as_str),
        Some("us")
    );
}

#[test]
fn a_page_that_came_back_empty_is_escalated_rather_than_accepted() {
    let run = run(
        &Policy::standard(),
        Budget::unlimited(),
        &[
            Scripted::seen(200, Some(200)).blank().costing(1.0),
            Scripted::seen(200, Some(200)).costing(4.0),
        ],
    );
    assert_eq!(
        run.moves,
        [Move::Escalate("browser", Duration::ZERO), Move::Accept]
    );
    assert_ne!(run.moves[0], Move::Accept);
    assert_eq!(run.params.request, Some(RequestMode::Browser));
    assert_eq!(run.spent, Credits(5.0));
}

#[test]
fn a_blank_page_keeps_climbing_while_it_stays_blank() {
    let blank = Scripted::seen(200, Some(200)).blank().costing(1.0);
    let run = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited(),
        &[blank, blank, Scripted::seen(200, Some(200)).costing(5.0)],
    );
    assert_eq!(
        run.moves,
        [
            Move::Escalate("browser", Duration::ZERO),
            Move::Escalate("browser+wait", Duration::ZERO),
            Move::Accept,
        ]
    );
    assert!(run
        .params
        .wait_for
        .as_ref()
        .is_some_and(|wait| wait.idle_network.is_some()));
}

#[test]
fn nothing_to_return_is_accepted_as_an_empty_result() {
    let run = run(
        &Policy::standard(),
        Budget::unlimited(),
        &[Scripted::seen(204, None).blank()],
    );
    assert_eq!(run.moves, [Move::Accept]);
    assert_eq!(run.spent, Credits::ZERO);
}

#[test]
fn every_ladder_only_gets_dearer() {
    let ladders = [
        ("standard", Ladder::standard()),
        (
            "a national domain, which rotates twice",
            Ladder::for_target("https://shop.example.de/a"),
        ),
    ];

    for (name, ladder) in ladders {
        assert!(ladder.len() >= 4, "{name} is too short to be a ladder");
        for pair in ladder.0.windows(2) {
            assert!(
                pair[1].est_multiplier > pair[0].est_multiplier,
                "{name}: {} at {} does not cost more than {} at {}",
                pair[1].label,
                pair[1].est_multiplier,
                pair[0].label,
                pair[0].est_multiplier
            );
        }
    }
}

#[test]
fn a_walk_that_runs_out_of_steps_stops_rather_than_escalating_further() {
    let blocked = Scripted::seen(200, Some(403)).costing(5.0);
    let policy = Policy::standard().with_max_attempts(u8::MAX);

    let walked = run(&policy, Budget::unlimited(), &[blocked; 5]);
    assert_eq!(
        walked.moves.last(),
        Some(&Move::Stop(StopReason::LadderExhausted))
    );

    let last_step = walked
        .moves
        .iter()
        .rev()
        .find_map(|m| match m {
            Move::Escalate(label, _) => Some(*label),
            _ => None,
        })
        .expect("at least one escalation");
    assert_eq!(
        last_step, "residential+geo",
        "the walk ends on the dearest step the ladder holds"
    );
}

#[test]
fn the_attempt_cap_stops_a_walk_the_credits_would_have_paid_for() {
    let blocked = Scripted::seen(200, Some(403)).costing(1.0);
    let run = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited().with_attempts(3),
        &[blocked; 3],
    );
    assert_eq!(
        run.moves,
        [
            Move::Escalate("browser", Duration::ZERO),
            Move::Escalate("browser+wait", Duration::ZERO),
            Move::Stop(StopReason::Budget(BudgetKind::Attempts)),
        ]
    );
    assert_eq!(run.attempts, 3);
}

#[test]
fn the_per_page_cap_refuses_a_step_the_total_would_have_allowed() {
    let blocked = Scripted::seen(200, Some(403)).costing(5.0);
    let run = run(
        &Policy::standard(),
        Budget::unlimited().with_per_page_credits(Credits(10.0)),
        &[blocked],
    );
    // The cheapest step is estimated at four times the last cost, which is twenty.
    assert_eq!(
        run.moves,
        [Move::Stop(StopReason::Budget(BudgetKind::Credits))]
    );
}

#[test]
fn the_budget_caps_reach_the_request_so_the_service_enforces_them_too() {
    let budget = Budget::default()
        .with_credits(Credits(400.0))
        .with_per_page_credits(Credits(40.0));
    let run = run(
        &Policy::standard(),
        budget,
        &[Scripted::seen(200, Some(200))],
    );

    assert_eq!(run.params.max_credits_allowed, Some(WholeCredits::new(400)));
    assert_eq!(run.params.max_credits_per_page, Some(Credits(40.0)));
}

#[test]
fn a_national_domain_rotates_to_its_own_country_first() {
    let blocked = Scripted::seen(200, Some(403)).costing(2.0);
    let run = run(
        &Policy::for_target("https://shop.example.de/a").with_max_attempts(u8::MAX),
        Budget::unlimited(),
        &[blocked; 6],
    );

    // Five steps, because the rotation has two countries in it.
    assert_eq!(
        run.moves,
        [
            Move::Escalate("browser", Duration::ZERO),
            Move::Escalate("browser+wait", Duration::ZERO),
            Move::Escalate("residential", Duration::ZERO),
            Move::Escalate("residential+geo", Duration::ZERO),
            Move::Escalate("residential+geo", Duration::ZERO),
            Move::Stop(StopReason::LadderExhausted),
        ]
    );
    assert_eq!(
        run.params.country_code.as_ref().map(Country::as_str),
        Some("us"),
        "the second rotation lands on the fallback"
    );

    // The first rotation went to the site's own country, not straight to the fallback.
    let ladder = Ladder::for_target("https://shop.example.de/a");
    assert_eq!(
        ladder
            .get(3)
            .and_then(|step| step.country())
            .map(Country::as_str),
        Some("de")
    );
}

#[test]
fn a_seeded_curve_sleeps_the_same_spans_on_every_run() {
    use spider_cloud_agent::policy::{Backoff, Jitter};

    let policy = Policy::standard()
        .with_max_attempts(u8::MAX)
        .with_backoff(Backoff {
            jitter: Jitter::Seeded(7),
            ..Backoff::default()
        });
    let limited = Scripted::seen(503, None);

    let first = run(&policy, Budget::unlimited(), &[limited; 4]);
    let second = run(&policy, Budget::unlimited(), &[limited; 4]);
    assert_eq!(first.moves, second.moves);
    assert_eq!(first.slept, second.slept);

    // Jitter shortens some waits, so the seeded run cannot be the plain curve.
    let plain = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited(),
        &[limited; 4],
    );
    assert!(first.slept < plain.slept, "the draw did nothing");
    assert!(first.slept >= plain.slept / 2, "the draw went below half");
}

#[test]
fn the_two_planes_wait_differently_for_the_same_number() {
    let policy = Policy::standard();

    // Same code, no header on either, and the waits must not be the same. Our own rate
    // limit is about how often this account is calling. A site's is about the site.
    let ours = first_move(&policy, Scripted::seen(429, None));
    let theirs = first_move(&policy, Scripted::seen(200, Some(429)));

    assert_eq!(ours, Move::Retry(FIRST_WAIT));
    assert_eq!(theirs, Move::Retry(SLOW_DOWN));
    assert_ne!(ours, theirs, "the two planes were flattened into one wait");
}

#[test]
fn a_site_rate_limit_waits_at_least_the_floor_the_response_layer_names() {
    assert_eq!(
        target_slow_down(),
        SLOW_DOWN,
        "the policy and the response layer disagree about this wait"
    );

    match first_move(&Policy::standard(), Scripted::seen(200, Some(429))) {
        Move::Retry(after) => assert!(
            after >= target_slow_down(),
            "waited {after:?}, below the floor of {:?}",
            target_slow_down()
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_site_that_names_its_own_wait_is_taken_at_its_word() {
    let longer = Duration::from_secs(20);
    assert_eq!(
        first_move(
            &Policy::standard(),
            Scripted::seen(200, Some(429)).retry_after(longer)
        ),
        Move::Retry(longer),
        "a site asking for longer than the floor was cut short"
    );

    // Shorter than the floor is still the site's own figure, not a guess.
    let shorter = Duration::from_secs(2);
    assert_eq!(
        first_move(
            &Policy::standard(),
            Scripted::seen(200, Some(429)).retry_after(shorter)
        ),
        Move::Retry(shorter)
    );
}

#[test]
fn the_floor_never_pushes_a_wait_past_a_ceiling_the_caller_set() {
    use spider_cloud_agent::policy::Backoff;

    let policy = Policy::standard().with_backoff(Backoff {
        cap: Duration::from_secs(2),
        ..Backoff::default()
    });
    assert_eq!(
        first_move(&policy, Scripted::seen(200, Some(429))),
        Move::Retry(Duration::from_secs(2))
    );
}

#[test]
fn a_walk_that_opens_on_a_free_target_error_still_stops_at_the_credit_cap() {
    // A target 503 is not billed, so every attempt here reports a cost of nothing.
    // Without a floor on the estimate the cap would see zero forever and refuse
    // nothing, on exactly the failure most likely to start an escalation.
    let free = Scripted::seen(200, Some(503)).costing(0.0);
    let run = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited().with_credits(Credits(ASSUMED_MINIMUM_COST.get() * 6.0)),
        &[free; 6],
    );

    assert_eq!(
        run.moves,
        [
            Move::Retry(FIRST_WAIT),
            Move::Escalate("browser", Duration::ZERO),
            Move::Retry(FIRST_WAIT),
            Move::Escalate("browser+wait", Duration::ZERO),
            Move::Retry(FIRST_WAIT),
            // Residential is estimated at eight times the assumed minimum, which is
            // more than the six that are left.
            Move::Stop(StopReason::Budget(BudgetKind::Credits)),
        ]
    );
    assert_eq!(run.spent, Credits::ZERO, "nothing here was billed");
    // A tenth of a credit sits under every single fetch measured against a live
    // account on 2026-09-15, which ran 0.109 to 0.191 credits.
    assert_eq!(ASSUMED_MINIMUM_COST, Credits(0.1));
}

#[test]
fn a_free_attempt_does_not_buy_an_unlimited_walk() {
    let free = Scripted::seen(200, Some(500)).costing(0.0);
    let capped = run(
        &Policy::standard().with_max_attempts(u8::MAX),
        Budget::unlimited().with_per_page_credits(Credits(ASSUMED_MINIMUM_COST.get() * 3.0)),
        &[free; 2],
    );

    // The cheapest step is four times the assumed minimum, over a per page cap of three.
    assert_eq!(
        capped.moves,
        [
            Move::Retry(FIRST_WAIT),
            Move::Stop(StopReason::Budget(BudgetKind::Credits)),
        ]
    );
}

/// A request carrying cookies the caller supplied.
fn with_cookies() -> RequestParams {
    let mut params = RequestParams::url("https://example.com/account");
    params.cookies = Some("sid=abc".into());
    params
}

#[test]
fn a_login_wall_turns_on_the_session_when_the_caller_supplied_something_to_carry() {
    let policy = Policy::standard();
    let mut state = AttemptState::new(Budget::unlimited());

    let seen = Observed::seen(200, Some(401)).for_request(&with_cookies());
    state.record(&seen);

    match policy.decide(&seen, &state) {
        Next::Escalate { step, .. } => {
            assert_eq!(step.label, SESSION_LABEL);
            let mut params = with_cookies();
            step.apply(&mut params);
            assert_eq!(params.session, Some(true));
            // It changes that one parameter and nothing else.
            assert_eq!(params.request, None);
            assert_eq!(params.proxy, None);
        }
        other => panic!("expected the session step, got {other:?}"),
    }
}

#[test]
fn a_login_wall_stops_when_there_is_nothing_for_a_session_to_carry() {
    let policy = Policy::standard();
    let expected = Next::Stop(StopReason::Rejected {
        hint: Hint::NeedsSession,
    });

    // Nothing was supplied, so keeping state between calls would send the same
    // anonymous request twice.
    let bare =
        Observed::seen(200, Some(401)).for_request(&RequestParams::url("https://example.com"));

    // Cookies were supplied and the session is already on, so there is nothing left to
    // turn on.
    let mut already = with_cookies();
    already.session = Some(true);
    let repeated = Observed::seen(200, Some(401)).for_request(&already);

    for seen in [bare, repeated] {
        let mut state = AttemptState::new(Budget::unlimited());
        state.record(&seen);
        assert_eq!(policy.decide(&seen, &state), expected, "{seen:?}");
    }
}

#[test]
fn headers_count_as_something_to_carry_and_an_empty_map_does_not() {
    let mut with_headers = RequestParams::url("https://example.com");
    with_headers.headers = Some([("authorization".to_string(), "Bearer x".to_string())].into());

    let mut with_empty = RequestParams::url("https://example.com");
    with_empty.headers = Some(Default::default());

    let policy = Policy::standard();
    assert!(policy
        .session_step(&Observed::seen(200, Some(401)).for_request(&with_headers))
        .is_some());
    assert!(policy
        .session_step(&Observed::seen(200, Some(401)).for_request(&with_empty))
        .is_none());
}

#[test]
fn the_session_step_answers_a_login_wall_and_nothing_else() {
    let policy = Policy::standard();
    let params = with_cookies();

    for (api, page) in [
        (200u16, Some(403u16)),
        (200, Some(404)),
        (200, Some(200)),
        (401, None),
    ] {
        assert!(
            policy
                .session_step(&Observed::seen(api, page).for_request(&params))
                .is_none(),
            "api {api}, page {page:?}"
        );
    }
}
