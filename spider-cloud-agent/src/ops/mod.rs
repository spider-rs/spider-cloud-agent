//! One builder per endpoint.
//!
//! Every builder is the same shape: name what you want, adjust as little as you
//! need to, then `send`. `send` hands back the one page that worked. `send_all`
//! hands back everything, failures included, which is what a crawl and a batch
//! are actually for.
//!
//! Every operation ends. The budget's wall, fifteen minutes unless the caller
//! says otherwise, is one deadline over every send and every sleep in the
//! walk, and an empty 204 is an empty `Pages` from `send_all` rather than a
//! decode failure. A router, recorder or policy the caller supplied runs on
//! the caller's thread between calls and is not under the deadline.
//!
//! The adjustable surface is short on purpose. Mode, proxy pool, country, wait
//! condition, profile, timeout, session and budget are the settings worth
//! choosing by hand; the service decides better than a client can about the
//! rest, and every setting promoted here is a promise to keep across releases.
//! Anything finer lives behind `params_mut`, which hands over the whole
//! documented parameter set and, with it, ownership of the result.

pub mod crawl;
pub mod data;
pub mod fetch;
pub mod links;
pub mod scrape;
pub mod screenshot;
pub mod search;
pub mod transform;

use std::future::Future;
use std::time::{Duration, Instant};

use serde::Serialize;
use url::Url;

use crate::client::{IntoUrl, Spider};
use crate::credits::Credits;
use crate::error::{BudgetKind, Error};
use crate::params::{RequestParams, ReturnFormat, ReturnFormatHandling};
use crate::policy::engine::Reached;
use crate::policy::{AttemptState, Budget, Next, Observed, Policy, Step, StopReason};
use crate::response::{Attempt, Body, FailedPage, Hint, Outcome, Page, PageResult, Pages};
use crate::routing;
use crate::status::{ApiStatus, PageStatus};
use crate::thrift::plan::Endpoint;
use crate::thrift::tokens::{approx_tokens, TokenBudget};
use crate::thrift::trim::Trimmer;
use crate::thrift::{Need, Plan, ThriftReport};
use crate::transport::{route, Route};
use crate::Result;
use spider_route::featurize;
use tokio::time::{sleep, timeout_at, Instant as DeadlineInstant};

/// The service's own address, used as the subject of a result that did not come
/// from fetching a page.
const SERVICE_URL: &str = "https://spider.cloud";

/// One absolute clock shared by sends, sleeps and synchronous checkpoints.
#[derive(Clone, Copy)]
pub(crate) struct Deadline(Option<DeadlineInstant>);

impl Deadline {
    pub(crate) fn new(wall: Option<Duration>) -> Result<Self> {
        let end = wall
            .map(|wall| {
                DeadlineInstant::now()
                    .checked_add(wall)
                    .ok_or_else(|| Error::Config("wall is too large for the clock".into()))
            })
            .transpose()?;
        Ok(Self(end))
    }

    pub(crate) fn expired(self) -> bool {
        self.0.is_some_and(|end| DeadlineInstant::now() >= end)
    }
}

/// Run one call under a wall, or under none.
///
/// `None` comes back when the wall ran out before the call finished. Dropping
/// the call is what ends it: nothing in the transport holds a resource past
/// its future, so a request cut off here leaves nothing behind.
pub(crate) async fn within<T, F>(deadline: Deadline, call: F) -> Option<Result<T>>
where
    F: Future<Output = Result<T>>,
{
    if deadline.expired() {
        return None;
    }
    match deadline.0 {
        Some(end) => timeout_at(end, call).await.ok(),
        None => Some(call.await),
    }
}

/// The error for a call the wall ended before the service answered.
pub(crate) fn out_of_time(attempts: Vec<Attempt>) -> Error {
    Error::BudgetExceeded {
        kind: BudgetKind::Time,
        attempts,
    }
}

/// The state every builder carries, and the one place a call is made.
pub(crate) struct Call<'a> {
    pub(crate) spider: &'a Spider,
    pub(crate) params: crate::params::RequestParams,
    pub(crate) budget: Budget,
    pub(crate) need: Option<Need>,
    pub(crate) max_tokens: Option<usize>,
    url: Option<Url>,
    url_error: Option<String>,
}

impl std::fmt::Debug for Call<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("spider", &self.spider)
            .field("params", &self.params)
            .field("budget", &self.budget)
            .field("need", &self.need.as_ref().map(|_| "<redacted>"))
            .field("max_tokens", &self.max_tokens)
            .field("url", &self.url.as_ref().map(|_| "<redacted>"))
            .field("url_error", &self.url_error.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl<'a> Call<'a> {
    /// Recheck admission immediately before every send. The run reservation is the
    /// concurrency gate; a snapshot alone would let two tasks spend the same credit.
    fn admit(
        &mut self,
        attempts: &[Attempt],
        estimate: Credits,
    ) -> Result<Option<crate::client::Reservation>> {
        let spent: Credits = attempts
            .iter()
            .map(|a| Credits(a.cost.get().max(a.reserved.get())))
            .sum();
        self.budget
            .preflight(attempts.len().min(255) as u8, spent, estimate)
            .map_err(|kind| {
                let error = Error::BudgetExceeded {
                    kind,
                    attempts: attempts.to_vec(),
                };
                match self.spider.run_budget() {
                    Some(run) => error.accounted(attempts.to_vec(), Some(run.snapshot())),
                    None => error,
                }
            })?;
        match self.spider.run_budget() {
            None => Ok(None),
            Some(run) => run
                .reserve(estimate)
                .map(|reservation| {
                    let cap = spent + reservation.allowance;
                    self.budget.credits = Some(
                        self.budget
                            .credits
                            .map_or(cap, |old| Credits(old.get().min(cap.get()))),
                    );
                    Some(reservation)
                })
                .ok_or_else(|| {
                    Error::BudgetExceeded {
                        kind: BudgetKind::Credits,
                        attempts: attempts.to_vec(),
                    }
                    .accounted(attempts.to_vec(), Some(run.snapshot()))
                }),
        }
    }

    fn cap_for_run(&mut self) {
        if let Some(run) = self.spider.run_budget() {
            let left = run.remaining();
            self.budget.credits = Some(
                self.budget
                    .credits
                    .map_or(left, |cap| Credits(cap.get().min(left.get()))),
            );
        }
    }

    fn call_failure(&self, error: Error, attempts: Vec<Attempt>) -> Error {
        error.accounted(
            attempts,
            self.spider.run_budget().map(crate::RunBudget::snapshot),
        )
    }

    /// A call against one address. A bad address is held until `send`.
    pub(crate) fn new(spider: &'a Spider, url: impl IntoUrl) -> Call<'a> {
        let mut call = Call::bare(spider);
        match url.into_url() {
            Ok(url) => {
                call.params.url = Some(url.to_string());
                call.url = Some(url);
            }
            Err(e) => call.url_error = Some(format!("that is not an address: {e}")),
        }
        call
    }

    /// A call with no address, for the endpoints that do not read a page.
    pub(crate) fn bare(spider: &'a Spider) -> Call<'a> {
        Call {
            spider,
            params: crate::params::RequestParams::default(),
            budget: spider.budget(),
            need: None,
            max_tokens: None,
            url: None,
            url_error: None,
        }
    }

    /// The address, once it is needed.
    fn target(&self) -> Result<Url> {
        if let Some(message) = &self.url_error {
            return Err(Error::Config(message.clone()));
        }
        match &self.url {
            Some(url) => Ok(url.clone()),
            // The endpoints that read no page still produce results that need an
            // address on them. The service's own is the honest one to use: the
            // result came from there and from nowhere else.
            None => Url::parse(SERVICE_URL).map_err(|e| {
                Error::Config(format!(
                    "the built in address {SERVICE_URL} does not parse: {e}"
                ))
            }),
        }
    }

    /// The format the request asked for, when it asked for exactly one.
    ///
    /// The wire returns a field called `raw` whatever was requested, so this is
    /// what tells markdown from markup on the way back.
    fn format(&self) -> Option<ReturnFormat> {
        match self.params.return_format.as_ref()? {
            ReturnFormatHandling::Single(format) => Some(*format),
            ReturnFormatHandling::Multi(formats) if formats.len() == 1 => formats.first().copied(),
            ReturnFormatHandling::Multi(_) => None,
        }
    }

    /// Make the calls, escalating under the policy until it says to stop.
    ///
    /// The loop is short because the deciding is not here: [`Policy::decide`]
    /// answers with the next move, this waits for it, patches the request with
    /// the step it named, and calls again. Nothing in this function knows why a
    /// 403 escalates, which is what keeps the rule table testable without a
    /// network.
    ///
    /// `make_body` builds the request from the current parameters, because a
    /// step patches parameters and the endpoints wrap them differently.
    pub(crate) async fn run<B, F>(&mut self, route: Route, make_body: F) -> Result<Outcome<Pages>>
    where
        B: Serialize,
        F: Fn(&crate::params::RequestParams) -> B,
    {
        self.run_at(route, &[], make_body).await
    }

    /// The same, for a route that carries its target in the path.
    ///
    /// `/fetch` names the domain and the path in the address instead of in a
    /// `url` field, so the send loop has to be able to fill a route's
    /// placeholders. Everything else about the walk is the same, which is the
    /// reason this is one function and not a second loop.
    pub(crate) async fn run_at<B, F>(
        &mut self,
        route: Route,
        args: &[&str],
        make_body: F,
    ) -> Result<Outcome<Pages>>
    where
        B: Serialize,
        F: Fn(&crate::params::RequestParams) -> B,
    {
        self.run_at_inner(route, args, make_body)
            .await
            .map_err(|error| match self.spider.run_budget() {
                Some(run) => error.with_run(run.snapshot()),
                None => error,
            })
    }

    async fn run_at_inner<B, F>(
        &mut self,
        route: Route,
        args: &[&str],
        make_body: F,
    ) -> Result<Outcome<Pages>>
    where
        B: Serialize,
        F: Fn(&crate::params::RequestParams) -> B,
    {
        // The clock starts before routing and is never reset by an attempt.
        self.cap_for_run();
        let deadline = Deadline::new(self.budget.wall)?;
        let target = self.target()?;
        let policy = match self.spider.policy() {
            Some(policy) => policy.clone(),
            None => {
                let mut policy = Policy::for_target(target.as_str());
                policy.backoff = crate::policy::Backoff::seeded(self.spider.jitter_seed);
                policy
            }
        };
        // The plan is settled before anything is sent, because the format it
        // chooses is what tells markdown from markup on the way back. The
        // snapshot is the request as the caller left it, and it is what keeps
        // an explicit setting of theirs from being argued with.
        let plan = self.plan();
        let caller = self.params.clone();

        // The router answers before anything is sent, which is the whole point
        // of it: the cheapest settings likely to work cost nothing to choose
        // and a wrong first attempt costs a call. What it answers fills in what
        // the caller left alone and never overrides what they set.
        let remembered = self.spider.site_memory().get(&target);
        let input = routing::input(
            &target,
            routing::declared_need(self.need.as_ref()),
            routing::pins_from(&caller),
            remembered.as_ref(),
        );
        let picked = self.spider.router().route(&input);
        // A caller who fixed a setting is not explored. The pin would win over
        // the drawn arm anyway, and the outcome and the recorded row would then
        // name an action that was never sent.
        let decision = if input.pins.any() {
            picked
        } else {
            self.spider
                .explorer()
                .choose(&target, &picked, &self.budget)
                .unwrap_or(picked)
        };

        routing::apply_decision(&decision, &mut self.params, &caller);
        plan.apply_over(&mut self.params, &caller);
        let format = self.format();

        let mut state = AttemptState::new(self.budget);
        // A decision that says the cheap steps are hopeless here starts the
        // walk past them rather than paying for attempts it expects to fail.
        state.step = usize::from(decision.start_rung);

        let features = self.spider.recorder().map(|_| featurize(&input));
        let mut routed_attempt = true;
        let mut attempts: Vec<Attempt> = Vec::new();
        let mut pages = Pages::default();
        // Whether any attempt got a page back, and the last failed page it got.
        // Both outlive `pages`, which describes the last attempt only, because
        // a call that failed after a page came back is still a walk that
        // reached the site, and the error has to say so.
        let mut pages_came_back = false;
        let mut last_failed: Option<FailedPage> = None;
        let mut call_error: Option<Error>;
        let current = endpoint_for(&plan, route);
        let mut wire_bytes = 0usize;
        let mut estimate = Budget::floor(Credits::ZERO);
        let mut run_overrun = None;
        let mut page_overrun = None;
        loop {
            let mut attempt_bytes = 0u32;

            // A call is held to whatever is left of the wall. Without this a
            // service that accepted the request and never answered held the
            // caller for as long as the socket stayed open, and the budget only
            // ever looked at the clock between calls.
            if deadline.expired() {
                return Err(out_of_time(attempts));
            }
            if attempts.len() >= usize::from(policy.max_attempts) {
                return Err(Error::BudgetExceeded {
                    kind: BudgetKind::Attempts,
                    attempts,
                });
            }
            let reservation = self.admit(&attempts, estimate)?;
            state.budget = self.budget;
            let mut wire_budget = self.budget;
            wire_budget.credits = self.budget.remaining_credits(state.spent);
            wire_budget.apply(&mut self.params);
            let body = make_body(&self.params);
            let before = Instant::now();
            let sent = within(
                deadline,
                self.spider
                    .raw()
                    .post_with_limit(current, args, &body, self.spider.response_limit),
            )
            .await;
            let mut wall_ran_out = false;
            let mut charge_unknown = false;

            let observed = match sent {
                None => {
                    charge_unknown = true;
                    call_error = None;
                    pages = Pages::default();
                    wall_ran_out = true;
                    Observed::timed_out().taking(before.elapsed())
                }
                Some(Ok(reply)) => {
                    wire_bytes = reply.body.len();
                    attempt_bytes = wire_bytes.min(u32::MAX as usize) as u32;
                    let elapsed = reply.elapsed;
                    let status = reply.status;
                    let wait = reply.retry_after.or(reply.rate_limit.reset);

                    // A body status of 400 or above is copied onto the call's
                    // status, so a page the site refused arrives looking like a
                    // failed call. Sorting it back onto the target plane is what
                    // makes a refusal something to escalate rather than something
                    // to give up on.
                    if reply.is_success() || reply.is_mirrored_page_status() {
                        charge_unknown = !reply.body.is_empty()
                            && serde_json::from_slice::<serde_json::Value>(&reply.body)
                                .ok()
                                .as_ref()
                                .is_none_or(|body| !has_charge(body));
                        call_error = None;
                        pages = match reply.read_pages(&target, format, current == route::TRANSFORM)
                        {
                            Ok(pages) => pages,
                            Err(error) => {
                                let body =
                                    serde_json::from_slice::<serde_json::Value>(&reply.body).ok();
                                let cost = body.as_ref().map(charged).unwrap_or(Credits::ZERO);
                                let mut attempt = Attempt::new(elapsed, status, None, cost);
                                attempt.charge_unknown =
                                    body.as_ref().is_none_or(|body| !has_charge(body));
                                if let Some(reservation) = reservation {
                                    let _ = reservation.settle(cost, attempt.charge_unknown);
                                }
                                attempts.push(attempt);
                                return Err(self.call_failure(error, attempts));
                            }
                        };
                        if !pages.is_empty() {
                            pages_came_back = true;
                        }
                        if let Some(cap) = self.budget.per_page_credits {
                            if let Some(page) = pages.0.iter().find(|page| page.cost() > cap) {
                                page_overrun = Some(crate::response::BudgetOverrun {
                                    scope: crate::response::BudgetScope::Page,
                                    cap,
                                    spent: page.cost(),
                                });
                            }
                        }
                        if let Some(failed) = pages.failed().next() {
                            last_failed = Some(failed.clone());
                        }
                        // A mirrored envelope supplies no independent API code.
                        // Zero means unknown, never a success minted from page data.
                        let api = reply.operation_status();
                        let page = representative(&pages);
                        let mut observed = Observed::seen(0, None);
                        observed.api = Reached::Api(api);
                        observed.page = if current == route::TRANSFORM {
                            page.filter(|s| !s.is_unknown())
                        } else {
                            page
                        };
                        let mut observed = observed.costing(pages.total_cost()).taking(elapsed);
                        // An empty body is only a failure when the caller wanted
                        // a body. Need::Metadata and Need::Fields deliberately ask
                        // for return_format=empty, so the page arrives with nothing
                        // in it and that is the request working exactly as asked.
                        // Treating it as a blank page sent the walk up the whole
                        // ladder against a request that had already succeeded, and
                        // charged for every step.
                        if nothing_came_back(&pages) && !body_was_declined(&self.params) {
                            observed = observed.blank();
                        }
                        match wait {
                            Some(after) => observed.retry_after(after),
                            None => observed,
                        }
                    } else {
                        call_error = reply.as_error();
                        pages = Pages::default();
                        let cost = serde_json::from_slice::<serde_json::Value>(&reply.body)
                            .ok()
                            .as_ref()
                            .map(charged)
                            .unwrap_or(Credits::ZERO);
                        let observed = Observed::seen(status.code(), None)
                            .taking(elapsed)
                            .costing(cost);
                        match wait {
                            Some(after) => observed.retry_after(after),
                            None => observed,
                        }
                    }
                }
                Some(Err(Error::Transport(e))) if e.is_timeout() => {
                    charge_unknown = true;
                    call_error = Some(Error::Transport(e));
                    Observed::timed_out().taking(before.elapsed())
                }
                Some(Err(Error::Transport(e))) if e.is_connect() => {
                    call_error = Some(Error::Transport(e));
                    Observed::connect_failed().taking(before.elapsed())
                }
                // The answer ran past the size the caller asked the client to
                // read. The call is recorded before the walk stops, so what
                // was spent on the attempts before it stays on the error, and
                // the walk stops rather than climbs, because a heavier request
                // buys a bigger answer.
                Some(Err(Error::ResponseTooLarge { limit, status })) => {
                    let mut attempt = Attempt::new(before.elapsed(), status, None, Credits::ZERO);
                    attempt.charge_unknown = true;
                    attempts.push(attempt);
                    return Err(Error::Exhausted {
                        attempts,
                        last: last_failed.map(Box::new),
                        reason: StopReason::Unhandled,
                        source: Some(Box::new(Error::ResponseTooLarge { limit, status })),
                    });
                }
                // Anything else went wrong before a call could be judged, so
                // there is nothing for the policy to read.
                Some(Err(other)) => {
                    let mut attempt =
                        Attempt::new(before.elapsed(), ApiStatus::new(0), None, Credits::ZERO);
                    attempt.charge_unknown =
                        matches!(&other, Error::Transport(e) if !e.is_connect() && !e.is_builder());
                    if let Some(reservation) = reservation {
                        let _ = reservation.settle(attempt.cost, attempt.charge_unknown);
                    }
                    attempts.push(attempt);
                    return Err(self.call_failure(other, attempts));
                }
            }
            // Read off the request whether it carried anything a session could
            // keep, which is what decides whether a login wall gets a second
            // call.
            .for_request(&self.params);

            state.record(&observed);
            if let Some(reservation) = reservation {
                run_overrun = reservation
                    .settle(observed.cost, charge_unknown)
                    .or(run_overrun);
            }
            attempts.push(Attempt::new(
                observed.elapsed,
                api_status_of(&observed),
                observed.page,
                observed.cost,
            ));
            if let Some(attempt) = attempts.last_mut() {
                attempt.charge_unknown = charge_unknown;
                if charge_unknown {
                    attempt.reserved = Credits(estimate.get().max(attempt.cost.get()));
                    state.spent += Credits((estimate.get() - attempt.cost.get()).max(0.0));
                }
            }

            // The wall ended the call, so the budget has already decided. The
            // policy is not asked, because whatever it answered would be a call
            // there is no time left for.
            let next = if wall_ran_out || deadline.expired() {
                Next::Stop(StopReason::Budget(BudgetKind::Time))
            } else {
                policy.decide(&observed, &state)
            };

            // What the attempt says about the site, folded in before the next
            // move is acted on. The policy has already judged whether this
            // attempt got what the caller asked for, and asking a second time
            // here would be a second answer to drift apart from the first.
            let result = routing::outcome_of(&observed, next == Next::Accept, attempt_bytes);
            if routing::describes_the_site(&observed) {
                self.spider.site_memory().observe(&target, &result);
            }
            self.spider.router().observe(&input, &result);

            // Only the first attempt was routed. Every one after it was chosen
            // by the ladder, so a row for it would describe a decision the
            // router did not make.
            if routed_attempt {
                if let (Some(recorder), Some(features)) = (self.spider.recorder(), &features) {
                    recorder.observe(features, &decision, &result);
                }
                routed_attempt = false;
            }

            state.follow(&next);

            if deadline.expired() {
                return Err(out_of_time(attempts));
            }

            match next {
                Next::Accept => {
                    let report = self.settle(&mut pages, wire_bytes);
                    if deadline.expired() {
                        return Err(out_of_time(attempts));
                    }
                    let mut outcome = Outcome::new(pages, attempts)
                        .with_cap(self.budget.credits)
                        .reporting(report)
                        .routed(decision);
                    outcome.overrun = run_overrun.or(outcome.overrun).or(page_overrun);
                    return Ok(outcome);
                }
                Next::Retry { after } => {
                    estimate = Budget::floor(if observed.was_billed() {
                        state.last_cost
                    } else {
                        Credits::ZERO
                    });
                    if within(deadline, async {
                        sleep(after).await;
                        Ok(())
                    })
                    .await
                    .is_none()
                    {
                        return Err(out_of_time(attempts));
                    }
                }
                Next::Escalate { step, after, .. } => {
                    estimate = step.estimate(if observed.was_billed() {
                        state.last_cost
                    } else {
                        Credits::ZERO
                    });
                    if within(deadline, async {
                        sleep(after).await;
                        Ok(())
                    })
                    .await
                    .is_none()
                    {
                        return Err(out_of_time(attempts));
                    }
                    escalate(&step, &mut self.params, &plan, &caller);
                }
                Next::Stop(reason) => {
                    return Err(stopped(
                        reason,
                        attempts,
                        pages_came_back,
                        last_failed,
                        call_error,
                    ));
                }
            }
        }
    }

    /// The parameters the stated need asks for.
    pub(crate) fn plan(&self) -> Plan {
        match &self.need {
            Some(need) => Plan::for_need(need),
            // No need was stated, so nothing is switched off on the caller's
            // behalf and the service answers as it would.
            None => Plan::none(),
        }
    }

    /// Trim what came back, and say what that saved.
    ///
    /// Boilerplate is learned across the whole set first, because a block that
    /// repeats is only visible from more than one page. The token ceiling is
    /// shared out afterwards in proportion to what each page costs.
    fn settle(&self, pages: &mut Pages, wire_bytes: usize) -> ThriftReport {
        let bodies: Vec<String> = pages
            .0
            .iter()
            .map(|result| match result {
                PageResult::Ok(page) => page.text().unwrap_or_default().to_string(),
                PageResult::Failed(_) => String::new(),
            })
            .collect();
        let tokens_in: usize = bodies.iter().map(|body| approx_tokens(body)).sum();

        if self.need.is_none() && self.max_tokens.is_none() {
            return ThriftReport::untrimmed(wire_bytes, tokens_in);
        }

        let mut trimmer = match &self.need {
            Some(need) => Trimmer::for_need(need),
            None => Trimmer::default(),
        };
        trimmer.learn(&bodies);

        let budget = match self.max_tokens {
            Some(total) => TokenBudget::total(total),
            None => TokenBudget::unlimited(),
        };
        let first_pass: Vec<String> = bodies
            .iter()
            .map(|body| trimmer.trim(body, None).text)
            .collect();
        let costs: Vec<usize> = first_pass.iter().map(|text| approx_tokens(text)).collect();
        let shares = budget.split(&costs);

        let mut reasons = Vec::new();
        let mut returned_bytes = 0usize;
        let mut tokens_out = 0usize;
        for ((result, before), ceiling) in pages.0.iter_mut().zip(&first_pass).zip(shares) {
            let PageResult::Ok(page) = result else {
                continue;
            };
            let trimmed = trimmer.trim(before, budget.total.map(|_| ceiling));
            returned_bytes += trimmed.text.len();
            tokens_out += approx_tokens(&trimmed.text);
            reasons.extend(trimmed.reasons);
            replace_text(&mut page.body, trimmed.text);
            // Extractions and metadata are payload too. Counting only the text
            // body reported a hundred per cent saving on a request that did hand
            // the caller its answer, which is a true number and a dishonest one.
            let aside = payload_beside_the_text(page);
            returned_bytes += aside.0;
            tokens_out += aside.1;
        }

        ThriftReport {
            wire_bytes,
            returned_bytes,
            approx_tokens_in: tokens_in,
            approx_tokens_out: tokens_out,
            trimmed: reasons,
        }
    }

    /// Make one call and read the body as a value rather than as pages.
    ///
    /// Used by the endpoints whose answer is not a page: a result list, a stored
    /// table, a balance. Nothing here escalates, because none of these failures
    /// is about a site refusing a fetch.
    pub(crate) async fn run_json<B, T, F>(
        &mut self,
        route: Route,
        make_body: F,
    ) -> Result<Outcome<T>>
    where
        B: Serialize,
        T: serde::de::DeserializeOwned,
        F: Fn(&crate::params::RequestParams) -> B,
    {
        self.run_json_inner(route, make_body)
            .await
            .map_err(|error| match self.spider.run_budget() {
                Some(run) => error.with_run(run.snapshot()),
                None => error,
            })
    }

    async fn run_json_inner<B, T, F>(&mut self, route: Route, make_body: F) -> Result<Outcome<T>>
    where
        B: Serialize,
        T: serde::de::DeserializeOwned,
        F: Fn(&crate::params::RequestParams) -> B,
    {
        self.cap_for_run();
        let deadline = Deadline::new(self.budget.wall)?;
        // One call, held to the wall the same way the send loop holds its
        // calls. A search against a service that went quiet hung here too.
        if deadline.expired() {
            return Err(out_of_time(Vec::new()));
        }
        let reservation = self.admit(&[], Budget::floor(Credits::ZERO))?;
        self.budget.apply(&mut self.params);
        let body = make_body(&self.params);
        let before = Instant::now();
        let reply = match within(
            deadline,
            self.spider
                .raw()
                .post_with_limit(route, &[], &body, self.spider.response_limit),
        )
        .await
        .unwrap_or_else(|| {
            let mut attempt =
                Attempt::new(before.elapsed(), ApiStatus::new(0), None, Credits::ZERO);
            attempt.charge_unknown = true;
            Err(out_of_time(vec![attempt]))
        }) {
            Ok(reply) => reply,
            // Recorded the way the send loop records it, so the one call this
            // made is on the error rather than lost with it.
            Err(Error::ResponseTooLarge { limit, status }) => {
                return Err(Error::Exhausted {
                    attempts: vec![Attempt {
                        charge_unknown: true,
                        ..Attempt::new(before.elapsed(), status, None, Credits::ZERO)
                    }],
                    last: None,
                    reason: StopReason::Unhandled,
                    source: Some(Box::new(Error::ResponseTooLarge { limit, status })),
                });
            }
            Err(other) => {
                if matches!(other, Error::BudgetExceeded { .. }) {
                    return Err(other);
                }
                let mut attempt =
                    Attempt::new(before.elapsed(), ApiStatus::new(0), None, Credits::ZERO);
                attempt.charge_unknown =
                    matches!(&other, Error::Transport(e) if !e.is_connect() && !e.is_builder());
                if let Some(reservation) = reservation {
                    let _ = reservation.settle(attempt.cost, attempt.charge_unknown);
                }
                return Err(self.call_failure(other, vec![attempt]));
            }
        };
        let body = reply.json::<serde_json::Value>();
        let cost = body.as_ref().map(charged).unwrap_or(Credits::ZERO);
        let mut attempt = Attempt::new(reply.elapsed, reply.status, None, cost);
        attempt.charge_unknown =
            reply.is_success() && body.as_ref().map_or(true, |body| !has_charge(body));
        let run_overrun =
            reservation.and_then(|reservation| reservation.settle(cost, attempt.charge_unknown));
        if let Some(error) = reply.as_error() {
            return Err(self.call_failure(error, vec![attempt]));
        }
        let body = body.map_err(|error| self.call_failure(error, vec![attempt.clone()]))?;
        let value: T = serde_json::from_value(body)
            .map_err(|error| self.call_failure(Error::Decode(error), vec![attempt.clone()]))?;
        if deadline.expired() {
            return Err(out_of_time(vec![attempt]));
        }
        let mut outcome = Outcome::new(value, vec![attempt]).with_cap(self.budget.credits);
        outcome.overrun = run_overrun.or(outcome.overrun);
        Ok(outcome)
    }
}

/// Whether every item supplied a readable total, including an explicit zero.
fn has_charge(body: &serde_json::Value) -> bool {
    match body {
        serde_json::Value::Array(items) => !items.is_empty() && items.iter().all(has_charge),
        other => reported_charge(other).is_some(),
    }
}

/// Recover the total independently of page fields and cost breakdown fields.
/// A malformed optional field must not erase a readable bill beside it.
fn reported_charge(body: &serde_json::Value) -> Option<Credits> {
    let total = body.get("costs")?.get("total_cost")?;
    serde_json::from_value::<crate::credits::Usd>(total.clone())
        .ok()
        .map(Credits::from)
}

/// What the service said an answer cost, even if its page fields cannot be decoded.
///
/// A page carries its own cost block and the send loop reads it. The answers
/// that are not pages were recorded as zero whatever the body said, so a search
/// spent nothing as far as the report and the budget were concerned, and an
/// operation a budget cannot see is the failure this crate exists to prevent.
///
/// The cost of a search is still short by the query itself: measured on
/// 2026-09-15, a one result search moved the balance by 10 credits and the
/// service sent no cost block at all with the result list. What arrives is
/// reported, and nothing is invented for what does not.
fn charged(body: &serde_json::Value) -> Credits {
    match body {
        serde_json::Value::Array(items) => items.iter().map(charged).sum(),
        other => reported_charge(other).unwrap_or(Credits::ZERO),
    }
}

/// Take one step up the ladder.
///
/// The step patches parameters, and the plan is written over what it left
/// behind. Nothing on the ladder asks for a fatter return format today, and
/// this pairing is what keeps that true when a rung is added later. It is one
/// function rather than two lines in the send loop so that it can be checked
/// without a socket.
fn escalate(step: &Step, params: &mut RequestParams, plan: &Plan, caller: &RequestParams) {
    step.apply(params);
    // A mode the caller named is theirs, here as much as it is when the router
    // answers. Every rung on the ladder renders the page, so without this a
    // caller who asked for plain HTTP to keep the bill down was billed for a
    // browser they had refused, and nothing in the result said so.
    if let Some(mode) = caller.request {
        params.request = Some(mode);
    }
    plan.apply_over(params, caller);
}

/// Where a plan sends the call.
///
/// Only a page request is moved. A crawl that asked for links keeps crawling
/// and asks for the links of every page it visits, which is not the same
/// operation as reading the links off one page.
fn endpoint_for(plan: &Plan, requested: Route) -> Route {
    if requested != route::SCRAPE {
        return requested;
    }
    match plan.endpoint {
        Some(Endpoint::Links) => route::LINKS,
        Some(Endpoint::Screenshot) => route::SCREENSHOT,
        Some(Endpoint::Scrape) | None => requested,
    }
}

/// Put trimmed text back in the body it came out of, keeping the form it had.
///
/// A body that is not text is left alone. There is nothing useful to cut out of
/// an image, and turning one into a string is not this layer's decision.
fn replace_text(body: &mut Body, text: String) {
    match body {
        Body::Text(held) | Body::Markdown(held) | Body::Html(held) | Body::Xml(held) => {
            *held = text
        }
        _ => {}
    }
}

/// The status to record for an attempt, including the ones that never got an
/// answer.
fn api_status_of(observed: &Observed) -> ApiStatus {
    match observed.api {
        Reached::Api(status) => status,
        // Nothing answered, so there is no status to record. Zero is the one
        // value no service can send, which is what makes it readable as "never
        // got there" rather than as a code.
        Reached::TimedOut | Reached::ConnectFailed => ApiStatus::new(0),
    }
}

/// The status that stands for a whole response.
///
/// A served page wins over a refused one, so a crawl with some failures in it
/// reads as a success and is not sent again at a dearer setting.
fn representative(pages: &Pages) -> Option<PageStatus> {
    pages
        .first_ok()
        .map(|page| page.status)
        .or_else(|| pages.0.first().map(|result| result.status()))
}

/// The bytes and tokens a caller receives that are not the page text.
///
/// `Need::Fields` and `Need::Metadata` deliberately return no body, so the
/// answer arrives in the extractions or the metadata block, and a request that
/// asked for links gets its answer in the link list. A report that measured
/// only the text called all of those a total saving, which flattered the
/// number by ignoring the thing the caller asked for.
fn payload_beside_the_text(page: &Page) -> (usize, usize) {
    let mut bytes = 0usize;

    if let Body::Fields(fields) = &page.body {
        if let Ok(encoded) = serde_json::to_string(fields) {
            bytes += encoded.len();
        }
    }

    if let Some(meta) = &page.metadata {
        if let Ok(encoded) = serde_json::to_string(meta) {
            bytes += encoded.len();
        }
    }

    // Links are the payload on a links request and half the payload on a page
    // request that asked for both. Leaving them out reported a request that
    // handed back ninety addresses as having returned nothing.
    if let Some(links) = &page.links {
        bytes += links.iter().map(|link| link.as_str().len()).sum::<usize>();
    }

    // Structured payload is denser than prose, so the prose ratio would
    // understate it. Counting it by bytes over four keeps one rule for both.
    (bytes, bytes.div_ceil(4))
}

/// Whether the request asked the service not to send a body.
///
/// `return_format=empty` is how a caller says it wants the extractions or the
/// metadata and none of the page. A response with no body is then the correct
/// answer, not a silent failure.
fn body_was_declined(params: &RequestParams) -> bool {
    match params.return_format.as_ref() {
        Some(ReturnFormatHandling::Single(format)) => *format == ReturnFormat::Empty,
        Some(ReturnFormatHandling::Multi(formats)) => {
            !formats.is_empty() && formats.iter().all(|f| *f == ReturnFormat::Empty)
        }
        None => false,
    }
}

/// Whether the response is the failure that looks like a success.
///
/// No pages at all, or every served page blank. A refusal is not this: it has a
/// status that says what happened.
fn nothing_came_back(pages: &Pages) -> bool {
    if pages.is_empty() {
        return true;
    }
    let mut served = pages.ok().peekable();
    served.peek().is_some() && pages.ok().all(Page::is_blank)
}

/// Turn the policy's reason for stopping into the error the caller sees.
///
/// When no page ever came back, [`Error::Accounted`] keeps the call error, an
/// [`Error::Api`], [`Error::Auth`] or [`Error::Transport`], because what
/// stopped the walk is a fact about the call rather than about a page. Only
/// when pages came back and the walk then stopped is it [`Error::Exhausted`],
/// carrying the policy's reason and, when a call was what stopped it, that
/// call's error as the source. A rate limit that runs out after a page came
/// back is therefore an exhausted walk and not a bare rate limit.
fn stopped(
    reason: StopReason,
    attempts: Vec<Attempt>,
    pages_came_back: bool,
    last_failed: Option<FailedPage>,
    call_error: Option<Error>,
) -> Error {
    match reason {
        StopReason::OutOfCredits => Error::InsufficientCredits.accounted(attempts, None),
        StopReason::Budget(kind) => Error::BudgetExceeded { kind, attempts },
        _ => match call_error {
            Some(error) if !pages_came_back => error.accounted(attempts, None),
            source => Error::Exhausted {
                attempts,
                last: last_failed.map(Box::new),
                reason,
                source: source.map(Box::new),
            },
        },
    }
}

/// Take the one page that worked, or say what stopped it.
///
/// The walk has already ended in [`Error::Exhausted`] with the policy's own
/// reason when the site refused every attempt, so the only way to get here
/// with no served page is a policy that accepted a failed page as the answer.
/// That is the site's answer settling, and the reason says so, with the hint
/// off the page it settled on. An empty 204 is the other way in: the service
/// accepted the call and had no page to send, so there is no single page and
/// no last failure, and the error carries neither.
pub(crate) fn first_page(outcome: Outcome<Pages>) -> Result<Outcome<Page>> {
    let Outcome {
        overrun,
        value,
        attempts,
        cost,
        thrift,
        route,
    } = outcome;
    let last = value.failed().next().cloned();
    match value.into_ok().into_iter().next() {
        Some(page) => Ok(Outcome {
            overrun,
            value: page,
            attempts,
            cost,
            thrift,
            route,
        }),
        None => Err(Error::Exhausted {
            attempts,
            reason: StopReason::Rejected {
                hint: last.as_ref().map_or(Hint::Permanent, |failed| failed.hint),
            },
            last: last.map(Box::new),
            source: None,
        }),
    }
}

/// How long a request timeout may be, in whole seconds, before the wire runs out
/// of room for it.
const MAX_TIMEOUT_SECS: u64 = u8::MAX as u64;

/// Turn a duration into the whole seconds the request timeout field holds.
pub(crate) fn timeout_secs(timeout: Duration) -> u8 {
    timeout.as_secs().clamp(1, MAX_TIMEOUT_SECS) as u8
}

/// The settings worth choosing by hand, on every builder that reads a page.
///
/// Written once here rather than eight times, so the curated surface cannot
/// drift apart between endpoints.
macro_rules! curated_surface {
    // The operations that fetch a page and can hand back its links in the same
    // answer. Written as an arm rather than as part of the common surface so
    // that an operation which fetches nothing cannot offer it.
    ($builder:ident, page_links) => {
        curated_surface!($builder);

        impl<'a> $builder<'a> {
            /// Ask for the links found on the page alongside its content.
            ///
            /// They arrive in the same answer, so a run that is already paying
            /// for pages gets the link graph without a second call.
            ///
            /// ```no_run
            /// # use spider_cloud_agent::{Need, Spider};
            /// # async fn show(spider: Spider) -> spider_cloud_agent::Result<()> {
            /// let page = spider.scrape("https://example.com")
            ///     .need(Need::Markdown)
            ///     .page_links(true)
            ///     .send()
            ///     .await?;
            /// let found = page.links.as_deref().unwrap_or_default();
            /// # let _ = found;
            /// # Ok(())
            /// # }
            /// ```
            pub fn page_links(mut self, on: bool) -> Self {
                self.call.params.return_page_links = Some(on);
                self
            }
        }
    };
    ($builder:ident) => {
        impl<'a> $builder<'a> {
            /// Whether the page is rendered before it is read.
            ///
            /// Leave it alone unless you already know: the default starts cheap
            /// and moves up on its own when the first answer looks empty.
            pub fn mode(mut self, mode: $crate::params::RequestMode) -> Self {
                self.call.params.request = Some(mode);
                self
            }

            /// Which pool the request leaves from.
            ///
            /// Residential addresses cost more per page, so they are worth
            /// asking for once the cheaper pool has been turned away.
            pub fn proxy(mut self, pool: $crate::params::ProxyPool) -> Self {
                self.call.params.proxy = Some(pool);
                self
            }

            /// The country to appear to be in.
            ///
            /// Changes prices, stock and language on plenty of sites, and
            /// sometimes whether the page is served at all.
            pub fn country(mut self, country: $crate::params::Country) -> Self {
                self.call.params.country_code = Some(country);
                self
            }

            /// What has to happen on the page before it is read.
            pub fn wait_for(mut self, wait: $crate::params::WaitFor) -> Self {
                self.call.params.wait_for = Some(wait);
                self
            }

            /// A coarse identity to present, which fills in a user agent and the
            /// window size that belongs with it.
            ///
            /// Anything you set yourself is left alone.
            pub fn profile(mut self, profile: $crate::params::Profile) -> Self {
                self.call.params.apply_profile(profile);
                self
            }

            /// How long one page has to come back.
            ///
            /// Rounded to whole seconds, which is what the wire carries, and
            /// held between one second and 255.
            pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
                self.call.params.request_timeout = Some($crate::ops::timeout_secs(timeout));
                self
            }

            /// Keep cookies and headers across the requests made to one site.
            ///
            /// Off by default, because a shared session makes otherwise
            /// independent requests affect each other.
            pub fn session(mut self, on: bool) -> Self {
                self.call.params.session = Some(on);
                self
            }

            /// What this operation may spend, in credits, time and calls.
            ///
            /// This client stops on the credit cap itself. The caps are also
            /// copied onto the request, where the service reads them as a byte
            /// ceiling rather than as a spend ceiling, so that side bounds the
            /// size of a page and not the size of a bill.
            pub fn budget(mut self, budget: $crate::policy::Budget) -> Self {
                self.call.budget = budget;
                self
            }

            /// What you actually want back.
            ///
            /// This is the setting that saves the most, and it works by asking
            /// for less rather than by cutting what arrives. Naming a need
            /// switches off metadata, headers, cookies, page links, structured
            /// data and embeddings unless the need is the one asking for them.
            /// Anything you set yourself is left alone.
            ///
            /// ```no_run
            /// # use spider_cloud_agent::{Need, Spider};
            /// # async fn show(spider: Spider) -> spider_cloud_agent::Result<()> {
            /// let page = spider.scrape("https://example.com")
            ///     .need(Need::Markdown)
            ///     .max_tokens(4000)
            ///     .send()
            ///     .await?;
            /// # let _ = page;
            /// # Ok(())
            /// # }
            /// ```
            pub fn need(mut self, need: $crate::thrift::Need) -> Self {
                self.call.need = Some(need);
                self
            }

            /// The most this operation may hand back, in tokens.
            ///
            /// Counted with an estimate rather than a tokenizer, and shared out
            /// across the pages in proportion to what each one costs. A page
            /// cut short ends on a sentence and says how much went.
            pub fn max_tokens(mut self, tokens: usize) -> Self {
                self.call.max_tokens = Some(tokens);
                self
            }

            /// The whole documented parameter set, to change directly.
            ///
            /// The escape hatch. Everything the API accepts is here, including
            /// the settings the curated surface leaves out on purpose, and a
            /// request built this way is yours to get right.
            pub fn params_mut(&mut self) -> &mut $crate::params::RequestParams {
                &mut self.call.params
            }
        }
    };
}

pub(crate) use curated_surface;

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
    use crate::params::ReturnFormat;
    use crate::policy::Ladder;
    use crate::thrift::Need;

    #[test]
    fn f2_a_malformed_breakdown_does_not_erase_a_readable_total() {
        let body = serde_json::json!({
            "costs": { "total_cost": 0.0002, "compute_cost": {} },
            "status": "not a status"
        });
        assert!(has_charge(&body));
        assert_eq!(charged(&body), Credits(2.0));
        assert!(!has_charge(&serde_json::json!({"costs": {}})));
    }

    #[test]
    fn f1_default_wall_reaches_page_and_search_calls() {
        let spider = Spider::builder()
            .key("not-a-real-key")
            .base_url(Url::parse("https://example.com").unwrap())
            .build()
            .unwrap();
        // All page builders use new or bare; search uses bare and run_json.
        // Check the default here, and exercise every send path with a short
        // wall in wire::f1_wall_ends_every_operation.
        for call in [
            Call::new(&spider, "https://example.com"),
            Call::bare(&spider),
        ] {
            assert_eq!(call.budget.wall, Some(Duration::from_secs(900)));
            assert!(Deadline::new(call.budget.wall).unwrap().0.is_some());
        }
    }

    #[test]
    fn curated_surface_membership_is_explicit() {
        let source = include_str!("mod.rs");
        let surface = source
            .split_once("macro_rules! curated_surface {")
            .unwrap()
            .1
            .split_once("pub(crate) use curated_surface;")
            .unwrap()
            .0;
        let (page_links, common) = surface.split_once("($builder:ident) =>").unwrap();
        let methods = |arm: &str| -> Vec<String> {
            let code = arm
                .lines()
                .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
                .collect::<Vec<_>>()
                .join("\n");
            let tokens: Vec<_> = code
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .filter(|token| !token.is_empty())
                .collect();
            tokens
                .windows(2)
                .filter(|pair| pair[0] == "fn")
                .map(|pair| pair[1].to_owned())
                .collect()
        };
        assert_eq!(methods(page_links), ["page_links"]);
        let expected = [
            "mode",
            "proxy",
            "country",
            "wait_for",
            "profile",
            "timeout",
            "session",
            "budget",
            "need",
            "max_tokens",
            "params_mut",
        ];
        assert_eq!(methods(common), expected);
        let vocabulary = include_str!("../../../docs/action-vocabulary.md");
        for name in expected.into_iter().chain(["page_links"]) {
            assert!(
                vocabulary
                    .lines()
                    .any(|line| line.starts_with(&format!("| `{name}` |"))),
                "{name} needs a line in docs/action-vocabulary.md"
            );
        }
    }

    #[test]
    fn a_caller_that_declined_a_body_is_not_treated_as_a_blank_page() {
        // Need::Metadata and Need::Fields ask for return_format=empty on purpose.
        // Found live: the walk escalated five times against a request that had
        // already worked, and charged for every step.
        let mut params = RequestParams {
            return_format: Some(ReturnFormatHandling::Single(ReturnFormat::Empty)),
            ..RequestParams::default()
        };
        assert!(body_was_declined(&params));

        params.return_format = Some(ReturnFormatHandling::Multi(vec![ReturnFormat::Empty]));
        assert!(body_was_declined(&params));
    }

    #[test]
    fn a_caller_that_wanted_a_body_still_notices_a_blank_page() {
        let mut params = RequestParams::default();
        assert!(!body_was_declined(&params));

        params.return_format = Some(ReturnFormatHandling::Single(ReturnFormat::Markdown));
        assert!(!body_was_declined(&params));

        // Asking for markdown and an empty slot still wants markdown.
        params.return_format = Some(ReturnFormatHandling::Multi(vec![
            ReturnFormat::Markdown,
            ReturnFormat::Empty,
        ]));
        assert!(!body_was_declined(&params));

        // An empty list is not a declined body, it is an unset field.
        params.return_format = Some(ReturnFormatHandling::Multi(vec![]));
        assert!(!body_was_declined(&params));
    }

    #[test]
    fn a_step_up_the_ladder_keeps_the_plan_the_need_asked_for() {
        let caller = RequestParams::url("https://example.com");
        let plan = Plan::for_need(&Need::Markdown);
        let mut params = caller.clone();
        plan.apply_over(&mut params, &caller);

        for step in Ladder::standard().0 {
            // Something that reaches for the whole page, the way a rung added
            // later might.
            params.return_format = Some(ReturnFormat::Raw.into());
            params.metadata = Some(true);

            escalate(&step, &mut params, &plan, &caller);

            assert_eq!(
                params.return_format,
                Some(ReturnFormat::Markdown.into()),
                "the {} step left a fat return format behind",
                step.label
            );
            assert_eq!(params.metadata, Some(false), "after {}", step.label);
            // And the step itself still took effect.
            assert!(params.request.is_some(), "after {}", step.label);
        }
    }

    #[test]
    fn a_step_never_argues_with_a_setting_the_caller_made() {
        let mut caller = RequestParams::url("https://example.com");
        caller.return_format = Some(ReturnFormat::Raw.into());
        let plan = Plan::for_need(&Need::Markdown);
        let mut params = caller.clone();

        for step in Ladder::standard().0 {
            escalate(&step, &mut params, &plan, &caller);
        }

        assert_eq!(params.return_format, Some(ReturnFormat::Raw.into()));
    }

    #[test]
    fn only_a_page_request_is_moved_to_another_endpoint() {
        let links = Plan::for_need(&Need::Links);
        assert_eq!(endpoint_for(&links, route::SCRAPE), route::LINKS);
        // A crawl that wants links keeps crawling.
        assert_eq!(endpoint_for(&links, route::CRAWL), route::CRAWL);

        let shot = Plan::for_need(&Need::screenshot());
        assert_eq!(endpoint_for(&shot, route::SCRAPE), route::SCREENSHOT);

        let markdown = Plan::for_need(&Need::Markdown);
        assert_eq!(endpoint_for(&markdown, route::SCRAPE), route::SCRAPE);
        assert_eq!(endpoint_for(&Plan::none(), route::SCRAPE), route::SCRAPE);
    }

    #[test]
    fn trimmed_text_goes_back_in_the_shape_it_came_out_of() {
        let mut body = Body::Markdown("# one".into());
        replace_text(&mut body, "# two".into());
        assert_eq!(body, Body::Markdown("# two".into()));

        // A picture has nothing to trim and is left as it is.
        let mut shot = Body::Screenshot(bytes::Bytes::from_static(b"png"));
        replace_text(&mut shot, "nonsense".into());
        assert_eq!(shot, Body::Screenshot(bytes::Bytes::from_static(b"png")));
    }
}
