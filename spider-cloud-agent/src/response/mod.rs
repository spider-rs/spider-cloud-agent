//! What the API gives back, typed.
//!
//! The shape to know is [`PageResult`]. A fetch is either a [`Page`], which exists only
//! for a 2xx target status, or a [`FailedPage`], which carries the status, the credits
//! it burned anyway, and a [`Hint`] naming the parameter to change next.
//!
//! An operation returns an [`Outcome`], which derefs to the value and also carries the
//! trail of [`Attempt`]s it took to get there and what they cost.

pub mod content;
pub mod costs;
pub mod metadata;
pub mod page;
pub mod search;

pub use content::{Body, MultiBody};
pub use costs::{Costs, Credits, Usd, CREDITS_PER_USD};
pub use metadata::Metadata;
pub use page::{FailedPage, Hint, Page, PageResult, Pages};
pub use search::{SearchEntry, SearchResults};

use crate::status::{ApiStatus, PageStatus};
use crate::thrift::ThriftReport;
use spider_route::RouteDecision;
use std::ops::Deref;
use std::time::Duration;

/// One call to the API, recorded whether it worked or not.
///
/// Both status planes are here and they stay apart. `api` is the call to spider.cloud,
/// `page` is what the site returned, and `page` is absent when the call failed before a
/// site was reached.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Attempt {
    /// How long the call took, end to end.
    pub elapsed: Duration,
    /// The status of the call to spider.cloud.
    pub api: ApiStatus,
    /// The status the target site returned, when the call got that far.
    pub page: Option<PageStatus>,
    /// What this attempt cost.
    pub cost: Credits,
}

impl Attempt {
    /// Record an attempt. Internal to the client.
    pub(crate) fn new(
        elapsed: Duration,
        api: ApiStatus,
        page: Option<PageStatus>,
        cost: Credits,
    ) -> Attempt {
        Attempt {
            elapsed,
            api,
            page,
            cost,
        }
    }
}

/// A value, plus what it took to get it.
///
/// `Outcome<T>` derefs to `T`, so a fetch result reads as the page itself and the trail
/// is there when it is wanted.
///
/// ```no_run
/// # use spider_cloud_agent::response::{Outcome, Page};
/// # fn show(outcome: Outcome<Page>) {
/// let text = outcome.text();          // reads through to the page
/// let spent = outcome.cost;           // what the whole operation cost
/// let tries = outcome.attempts.len(); // how many calls that took
/// # let _ = (text, spent, tries);
/// # }
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Outcome<T> {
    /// What the operation produced.
    pub value: T,
    /// Every call made, in order.
    pub attempts: Vec<Attempt>,
    /// What the whole operation cost.
    pub cost: Credits,
    /// What the response weighed, and what the trimming took off it.
    pub thrift: ThriftReport,
    /// What the router chose for the first attempt, and why.
    ///
    /// Absent for the operations that never fetch a page, such as reading the
    /// balance, because there was nothing to route. When exploration replaced
    /// the router's own answer, this is the action that was actually sent and
    /// its source says so.
    pub route: Option<RouteDecision>,
}

impl<T> Outcome<T> {
    /// Build an outcome. Internal to the client.
    pub(crate) fn new(value: T, attempts: Vec<Attempt>) -> Outcome<T> {
        let cost = attempts.iter().map(|a| a.cost).sum();
        Outcome {
            value,
            attempts,
            cost,
            thrift: ThriftReport::default(),
            route: None,
        }
    }

    /// Attach what the router chose. Internal to the client.
    pub(crate) fn routed(mut self, decision: RouteDecision) -> Outcome<T> {
        self.route = Some(decision);
        self
    }

    /// Attach what the response weighed. Internal to the client.
    pub(crate) fn reporting(mut self, thrift: ThriftReport) -> Outcome<T> {
        self.thrift = thrift;
        self
    }

    /// Take the value and drop the trail.
    pub fn into_value(self) -> T {
        self.value
    }

    /// How many calls the operation took.
    pub fn attempt_count(&self) -> usize {
        self.attempts.len()
    }

    /// How long every call took together.
    pub fn elapsed(&self) -> Duration {
        self.attempts.iter().map(|a| a.elapsed).sum()
    }

    /// Whether the operation needed more than the first call.
    pub fn escalated(&self) -> bool {
        self.attempts.len() > 1
    }

    /// Apply a function to the value, keeping the trail.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        Outcome {
            value: f(self.value),
            attempts: self.attempts,
            cost: self.cost,
            thrift: self.thrift,
            route: self.route,
        }
    }
}

impl<T> Deref for Outcome<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
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
    use super::page::PageParts;
    use super::*;

    fn attempt(code: u16, page: Option<u16>, cost: f64) -> Attempt {
        Attempt::new(
            Duration::from_millis(100),
            ApiStatus::new(code),
            page.map(PageStatus::new),
            Credits(cost),
        )
    }

    #[test]
    fn an_outcome_adds_up_its_attempts() {
        let page = PageResult::from_parts(PageParts::for_test("https://example.com/a", 200))
            .into_ok()
            .expect("a page");
        let outcome = Outcome::new(
            page,
            vec![attempt(200, Some(403), 4.0), attempt(200, Some(200), 6.0)],
        );
        assert_eq!(outcome.cost, Credits(10.0));
        assert_eq!(outcome.attempt_count(), 2);
        assert!(outcome.escalated());
        assert_eq!(outcome.elapsed(), Duration::from_millis(200));
    }

    #[test]
    fn an_outcome_reads_through_to_its_value() {
        let page = PageResult::from_parts(PageParts::for_test("https://example.com/a", 200))
            .into_ok()
            .expect("a page");
        let outcome = Outcome::new(page, vec![attempt(200, Some(200), 4.0)]);
        // Deref, so the page's own methods are reached without naming the field.
        assert_eq!(outcome.text(), Some("hello"));
        assert_eq!(outcome.status.code(), 200);
        assert!(!outcome.escalated());
        assert_eq!(outcome.into_value().url.as_str(), "https://example.com/a");
    }

    #[test]
    fn an_attempt_keeps_the_two_planes_apart() {
        let a = attempt(200, Some(403), 4.0);
        assert_eq!(a.api.to_string(), "api status 200");
        assert_eq!(
            a.page.expect("a target status").to_string(),
            "target status 403"
        );
    }

    #[test]
    fn an_attempt_that_never_reached_a_site_has_no_target_status() {
        let a = attempt(401, None, 0.0);
        assert!(a.page.is_none());
    }

    #[test]
    fn mapping_keeps_the_trail() {
        let outcome = Outcome::new(1u8, vec![attempt(200, Some(200), 2.0)]);
        let mapped = outcome.map(|v| v as u32 + 1);
        assert_eq!(mapped.value, 2);
        assert_eq!(mapped.cost, Credits(2.0));
        assert_eq!(mapped.attempt_count(), 1);
    }
}
