//! One builder per endpoint.
//!
//! Every builder is the same shape: name what you want, adjust as little as you
//! need to, then `send`. `send` hands back the one page that worked. `send_all`
//! hands back everything, failures included, which is what a crawl and a batch
//! are actually for.
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
use crate::response::{Attempt, Body, Outcome, Page, PageResult, Pages};
use crate::routing;
use crate::status::{ApiStatus, PageStatus};
use crate::thrift::plan::Endpoint;
use crate::thrift::tokens::{approx_tokens, TokenBudget};
use crate::thrift::trim::Trimmer;
use crate::thrift::{Need, Plan, ThriftReport};
use crate::transport::{route, Route};
use crate::Result;
use spider_route::featurize;
use tokio::time::{sleep, timeout};

/// The service's own address, used as the subject of a result that did not come
/// from fetching a page.
const SERVICE_URL: &str = "https://spider.cloud";

/// How long an account read may take when the client's budget names no wall.
///
/// A balance or a page of the crawl record is a database read, and one that has
/// not answered in a minute is not going to. The page operations have no such
/// figure: a crawl can run for as long as the site is large, so only a wall the
/// caller set bounds those.
pub(crate) const DEFAULT_READ_WALL: Duration = Duration::from_secs(60);

/// The wall an account read runs under: the client's, or the default above.
pub(crate) fn read_wall(spider: &Spider) -> Duration {
    spider.budget().wall.unwrap_or(DEFAULT_READ_WALL)
}

/// Run one call under a wall, or under none.
///
/// `None` comes back when the wall ran out before the call finished. Dropping
/// the call is what ends it: nothing in the transport holds a resource past
/// its future, so a request cut off here leaves nothing behind.
pub(crate) async fn within<T, F>(wall: Option<Duration>, call: F) -> Option<Result<T>>
where
    F: Future<Output = Result<T>>,
{
    match wall {
        Some(wall) => timeout(wall, call).await.ok(),
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
#[derive(Debug)]
pub(crate) struct Call<'a> {
    pub(crate) spider: &'a Spider,
    pub(crate) params: crate::params::RequestParams,
    pub(crate) budget: Budget,
    pub(crate) need: Option<Need>,
    pub(crate) max_tokens: Option<usize>,
    url: Option<Url>,
    url_error: Option<String>,
}

impl<'a> Call<'a> {
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
        let target = self.target()?;
        let policy = match self.spider.policy() {
            Some(policy) => policy.clone(),
            None => Policy::for_target(target.as_str()),
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
        let mut call_error: Option<Error>;
        let current = endpoint_for(&plan, route);
        let mut wire_bytes = 0usize;
        // The wall is measured on the clock rather than summed from the attempts,
        // because a call that never answers reports no duration to sum.
        let started = Instant::now();

        loop {
            let mut attempt_bytes = 0u32;
            self.budget.apply(&mut self.params);
            let body = make_body(&self.params);

            // A call is held to whatever is left of the wall. Without this a
            // service that accepted the request and never answered held the
            // caller for as long as the socket stayed open, and the budget only
            // ever looked at the clock between calls.
            let remaining = self.budget.remaining_wall(started.elapsed());
            if remaining == Some(Duration::ZERO) {
                return Err(out_of_time(attempts));
            }
            let before = Instant::now();
            let sent = within(remaining, self.spider.raw().post(current, args, &body)).await;
            let mut wall_ran_out = false;

            let observed = match sent {
                None => {
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
                        call_error = None;
                        pages = reply.read(&target, format)?;
                        let mut observed =
                            Observed::seen(status.code(), representative(&pages).map(|s| s.code()))
                                .costing(pages.total_cost())
                                .taking(elapsed);
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
                        let observed = Observed::seen(status.code(), None).taking(elapsed);
                        match wait {
                            Some(after) => observed.retry_after(after),
                            None => observed,
                        }
                    }
                }
                Some(Err(Error::Transport(e))) if e.is_timeout() => {
                    call_error = Some(Error::Transport(e));
                    Observed::timed_out().taking(before.elapsed())
                }
                Some(Err(Error::Transport(e))) if e.is_connect() => {
                    call_error = Some(Error::Transport(e));
                    Observed::connect_failed().taking(before.elapsed())
                }
                // Anything else went wrong before a call could be judged, so
                // there is nothing for the policy to read.
                Some(Err(other)) => return Err(other),
            }
            // Read off the request whether it carried anything a session could
            // keep, which is what decides whether a login wall gets a second
            // call.
            .for_request(&self.params);

            state.record(&observed);
            attempts.push(Attempt::new(
                observed.elapsed,
                api_status_of(&observed),
                observed.page,
                observed.cost,
            ));

            // The wall ended the call, so the budget has already decided. The
            // policy is not asked, because whatever it answered would be a call
            // there is no time left for.
            let next = if wall_ran_out {
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

            match next {
                Next::Accept => {
                    let report = self.settle(&mut pages, wire_bytes);
                    return Ok(Outcome::new(pages, attempts)
                        .reporting(report)
                        .routed(decision));
                }
                Next::Retry { after } => sleep(after).await,
                Next::Escalate { step, after, .. } => {
                    sleep(after).await;
                    escalate(&step, &mut self.params, &plan, &caller);
                }
                Next::Stop(reason) => {
                    return Err(stopped(reason, attempts, &pages, call_error));
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
        self.budget.apply(&mut self.params);
        let body = make_body(&self.params);
        // One call, held to the wall the same way the send loop holds its
        // calls. A search against a service that went quiet hung here too.
        let reply = within(self.budget.wall, self.spider.raw().post(route, &[], &body))
            .await
            .ok_or_else(|| out_of_time(Vec::new()))??
            .into_result()?;
        let body: serde_json::Value = reply.json()?;
        let cost = charged(&body);
        let value: T = serde_json::from_value(body).map_err(Error::Decode)?;
        let attempt = Attempt::new(reply.elapsed, reply.status, None, cost);
        Ok(Outcome::new(value, vec![attempt]))
    }
}

/// What the service said an answer that is not a page cost.
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
        other => other
            .get("costs")
            .and_then(|costs| serde_json::from_value::<crate::response::Costs>(costs.clone()).ok())
            .map(|costs| costs.total())
            .unwrap_or(Credits::ZERO),
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
fn stopped(
    reason: StopReason,
    attempts: Vec<Attempt>,
    pages: &Pages,
    call_error: Option<Error>,
) -> Error {
    match reason {
        StopReason::OutOfCredits => Error::InsufficientCredits,
        StopReason::Budget(kind) => Error::BudgetExceeded { kind, attempts },
        // Nothing reached a site, so what stopped it is a fact about the call
        // rather than about a page.
        _ => match call_error {
            Some(error) if pages.is_empty() => error,
            _ => Error::Exhausted {
                attempts,
                last: pages.failed().next().cloned().map(Box::new),
            },
        },
    }
}

/// Take the one page that worked, or say what stopped it.
pub(crate) fn first_page(outcome: Outcome<Pages>) -> Result<Outcome<Page>> {
    let Outcome {
        value,
        attempts,
        cost,
        thrift,
        route,
    } = outcome;
    let last = value.failed().next().cloned();
    match value.into_ok().into_iter().next() {
        Some(page) => Ok(Outcome {
            value: page,
            attempts,
            cost,
            thrift,
            route,
        }),
        None => Err(Error::Exhausted {
            attempts,
            last: last.map(Box::new),
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
