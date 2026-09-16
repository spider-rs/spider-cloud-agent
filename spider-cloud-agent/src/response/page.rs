//! A fetched page, a fetch that failed, and the advice that comes with a failure.

use crate::response::content::Body;
use crate::response::costs::{Costs, Credits};
use crate::response::metadata::Metadata;
use crate::status::{PageClass, PageStatus};
use std::collections::BTreeMap;
use std::time::Duration;
use url::Url;

/// Everything the transport read out of one response, before it is sorted into a
/// success or a failure.
#[derive(Debug, Clone)]
pub(crate) struct PageParts {
    pub url: Url,
    pub status: PageStatus,
    pub body: Body,
    pub duration: Duration,
    pub costs: Costs,
    pub metadata: Option<Metadata>,
    pub links: Option<Vec<Url>>,
    pub headers: Option<BTreeMap<String, String>>,
    pub cookies: Option<BTreeMap<String, String>>,
    pub error: Option<String>,
}

#[cfg(test)]
// Test-only helper: a test that cannot build its own fixture should stop
// loudly rather than quietly assert nothing.
#[allow(clippy::expect_used)]
impl PageParts {
    /// A minimal set of parts, for tests that only care about the status.
    pub(crate) fn for_test(url: &str, code: u16) -> Self {
        PageParts {
            url: Url::parse(url).expect("a test url"),
            status: PageStatus::new(code),
            body: Body::Text("hello".into()),
            duration: Duration::from_millis(120),
            costs: Costs {
                total_cost: Credits::new(4.0).into(),
                ..Costs::default()
            },
            metadata: None,
            links: None,
            headers: None,
            cookies: None,
            error: None,
        }
    }
}

/// A page the target site served.
///
/// A fetched `Page` exists only for a 2xx target status. That is a guarantee of the type, not a
/// convention: the constructor checks the status and hands back a [`FailedPage`] when it
/// is anything else, and the fields cannot be filled in from outside the crate. So code
/// holding a `Page` does not have to check whether the fetch worked. Converted
/// documents from `transform` are the explicit exception: nothing was fetched,
/// so their target status can be unknown (zero).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Page {
    /// The address the content was served from, after redirects.
    pub url: Url,
    /// The status the site returned, or unknown for a converted document.
    pub status: PageStatus,
    /// The content.
    pub body: Body,
    /// How long the fetch took.
    pub duration: Duration,
    /// What the fetch cost.
    pub costs: Costs,
    /// Page metadata, when the request asked for it.
    pub metadata: Option<Metadata>,
    /// Links found on the page, when the request asked for them.
    pub links: Option<Vec<Url>>,
    /// Response headers, when the request asked for them.
    pub headers: Option<BTreeMap<String, String>>,
    /// Cookies set on the response, as names and values. Legacy wire strings
    /// are split at semicolons and the first equals sign in each pair.
    pub cookies: Option<BTreeMap<String, String>>,
}

impl Page {
    /// Sort one response into a page or a failure.
    ///
    /// The status is the only thing that decides. A non-2xx target status can never
    /// produce a `Page`.
    // Both sides of this result are whole responses, so one is always going to be large.
    // Boxing either would buy an allocation on every fetch to quiet a lint.
    #[allow(clippy::result_large_err)]
    pub(crate) fn try_from_parts(parts: PageParts) -> Result<Page, FailedPage> {
        if !parts.status.is_ok() {
            return Err(FailedPage::from_parts(parts));
        }
        Ok(Self::document_from_parts(parts))
    }

    /// Transform has no target HTTP response. Keep its unknown status explicit.
    pub(crate) fn document_from_parts(parts: PageParts) -> Page {
        Page {
            url: parts.url,
            status: parts.status,
            body: parts.body,
            duration: parts.duration,
            costs: parts.costs,
            metadata: parts.metadata,
            links: parts.links,
            headers: parts.headers,
            cookies: parts.cookies,
        }
    }

    /// What the fetch cost.
    pub fn cost(&self) -> Credits {
        self.costs.total()
    }

    /// The content as text, when the body is text.
    pub fn text(&self) -> Option<&str> {
        self.body.as_str()
    }

    /// Whether the site answered with a 2xx and nothing in it.
    ///
    /// This is the failure that does not look like one. A heavier request usually fixes
    /// it.
    pub fn is_blank(&self) -> bool {
        self.body.is_empty()
    }
}

/// A fetch the target site did not serve.
///
/// Most of these still cost credits, which is why the cost is carried here and not only
/// on the success. See [`crate::status::PageStatus::consumes_credits`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FailedPage {
    /// The address that was asked for.
    pub url: Url,
    /// The status the site returned.
    pub status: PageStatus,
    /// What the API said went wrong, when it said anything.
    pub error: Option<String>,
    /// What the attempt cost. A 403, a 404 and a 429 are all billed.
    pub costs: Costs,
    /// What to change before trying again.
    pub hint: Hint,
}

impl FailedPage {
    fn from_parts(parts: PageParts) -> FailedPage {
        FailedPage {
            url: parts.url,
            hint: Hint::for_class(parts.status.class()),
            status: parts.status,
            error: parts.error,
            costs: parts.costs,
        }
    }

    /// What the attempt cost.
    pub fn cost(&self) -> Credits {
        self.costs.total()
    }

    /// Whether this attempt was billed.
    pub fn was_billed(&self) -> bool {
        self.status.consumes_credits()
    }
}

/// What to change before asking for the same page again.
///
/// Every variant names a public request parameter and nothing else, so acting on a hint
/// is a change the caller can make and read back.
///
/// New variants can appear as the escalation ladder grows, so match with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Hint {
    /// Set `request` to `browser`. The page needs scripts to run before it has content.
    TryBrowser,
    /// Switch the proxy pool to residential.
    TryResidentialProxy,
    /// Keep the settings and ask from another country.
    TryDifferentCountry,
    /// The page is behind a login. Supply a session.
    NeedsSession,
    /// Nothing in the request will change this answer.
    Permanent,
    /// Wait, then send the same request again. Spending more here makes it worse.
    SlowDown {
        /// How long to wait before the next attempt.
        after: Duration,
    },
}

impl Hint {
    /// The first thing to change for a given failure.
    ///
    /// A hint is one step, not a plan. The policy engine reads the same class and may go
    /// further, for instance to [`Hint::TryDifferentCountry`] once a residential attempt
    /// has already failed.
    pub fn for_class(class: PageClass) -> Hint {
        match class {
            // A 2xx that reached here had nothing in it, and content that is not there
            // usually needs scripts to run.
            PageClass::Ok => Hint::TryBrowser,
            PageClass::Blocked => Hint::TryResidentialProxy,
            PageClass::NeedsLogin => Hint::NeedsSession,
            PageClass::TargetRateLimited => Hint::SlowDown {
                after: DEFAULT_SLOW_DOWN,
            },
            PageClass::BadRequest | PageClass::NotFound => Hint::Permanent,
            PageClass::ServerError => Hint::TryBrowser,
            _ => Hint::TryBrowser,
        }
    }

    /// Whether acting on this hint is worth an attempt.
    pub fn is_actionable(self) -> bool {
        !matches!(self, Hint::Permanent)
    }
}

/// How long to wait after a target rate limit, when the site did not say.
const DEFAULT_SLOW_DOWN: Duration = Duration::from_secs(5);

/// One fetch, sorted by whether the site served it.
///
/// Matching on this is how a caller gets a [`Page`], and it is the only way.
// A served page carries more than a refused one does. That size gap is the shape of the
// data, not something to hide behind a box on the hot path.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum PageResult {
    /// The site served the page.
    Ok(Page),
    /// The site did not.
    Failed(FailedPage),
}

impl PageResult {
    /// Sort one response.
    pub(crate) fn from_parts(parts: PageParts) -> PageResult {
        match Page::try_from_parts(parts) {
            Ok(page) => PageResult::Ok(page),
            Err(failed) => PageResult::Failed(failed),
        }
    }

    /// Whether the site served the page.
    pub fn is_ok(&self) -> bool {
        matches!(self, PageResult::Ok(_))
    }

    /// The page, when there is one.
    pub fn ok(&self) -> Option<&Page> {
        match self {
            PageResult::Ok(page) => Some(page),
            PageResult::Failed(_) => None,
        }
    }

    /// The failure, when there was one.
    pub fn failed(&self) -> Option<&FailedPage> {
        match self {
            PageResult::Failed(failed) => Some(failed),
            PageResult::Ok(_) => None,
        }
    }

    /// Take the page, dropping the failure.
    pub fn into_ok(self) -> Option<Page> {
        match self {
            PageResult::Ok(page) => Some(page),
            PageResult::Failed(_) => None,
        }
    }

    /// The status the site returned, either way.
    pub fn status(&self) -> PageStatus {
        match self {
            PageResult::Ok(page) => page.status,
            PageResult::Failed(failed) => failed.status,
        }
    }

    /// The address, either way.
    pub fn url(&self) -> &Url {
        match self {
            PageResult::Ok(page) => &page.url,
            PageResult::Failed(failed) => &failed.url,
        }
    }

    /// What this fetch cost, either way.
    pub fn cost(&self) -> Credits {
        match self {
            PageResult::Ok(page) => page.cost(),
            PageResult::Failed(failed) => failed.cost(),
        }
    }

    /// What to change before trying again, for a failure.
    pub fn hint(&self) -> Option<Hint> {
        self.failed().map(|f| f.hint)
    }
}

/// The pages from one crawl or one batch.
#[derive(Debug, Clone, Default)]
pub struct Pages(pub Vec<PageResult>);

impl Pages {
    /// The pages the sites served.
    pub fn ok(&self) -> impl Iterator<Item = &Page> {
        self.0.iter().filter_map(PageResult::ok)
    }

    /// The fetches that failed.
    pub fn failed(&self) -> impl Iterator<Item = &FailedPage> {
        self.0.iter().filter_map(PageResult::failed)
    }

    /// Take just the pages that were served.
    pub fn into_ok(self) -> Vec<Page> {
        self.0.into_iter().filter_map(PageResult::into_ok).collect()
    }

    /// The first page that was served.
    pub fn first_ok(&self) -> Option<&Page> {
        self.ok().next()
    }

    /// Every link found across the set, in the order they were seen, with
    /// repeats removed.
    ///
    /// Empty unless the request asked for links, which the links operation does
    /// on its own and any page operation does through `page_links`.
    pub fn links(&self) -> Vec<Url> {
        let mut out: Vec<Url> = Vec::new();
        for page in self.ok() {
            for link in page.links.iter().flatten() {
                if !out.contains(link) {
                    out.push(link.clone());
                }
            }
        }
        out
    }

    /// What the whole set cost, failures included.
    pub fn total_cost(&self) -> Credits {
        self.0.iter().map(PageResult::cost).sum()
    }

    /// How many fetches are in the set.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<PageResult>> for Pages {
    fn from(results: Vec<PageResult>) -> Pages {
        Pages(results)
    }
}

impl IntoIterator for Pages {
    type Item = PageResult;
    type IntoIter = std::vec::IntoIter<PageResult>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
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
    fn a_non_2xx_status_cannot_produce_a_page() {
        for code in [400u16, 401, 403, 404, 429, 500, 502, 503, 526, 301, 999] {
            let parts = PageParts::for_test("https://example.com/a", code);
            let built = Page::try_from_parts(parts.clone());
            assert!(built.is_err(), "code {code} produced a page");

            let result = PageResult::from_parts(parts);
            assert!(!result.is_ok(), "code {code} produced a page");
            assert!(result.ok().is_none());
            assert!(result.into_ok().is_none());
        }
    }

    #[test]
    fn a_2xx_status_produces_a_page() {
        for code in [200u16, 201, 204, 299] {
            let result = PageResult::from_parts(PageParts::for_test("https://example.com/a", code));
            let page = result.ok().expect("a page");
            assert_eq!(page.status.code(), code);
            assert!(page.status.is_ok());
        }
    }

    #[test]
    fn a_failure_keeps_the_cost_it_burned() {
        let result = PageResult::from_parts(PageParts::for_test("https://example.com/a", 403));
        let failed = result.failed().expect("a failure");
        assert_eq!(failed.cost(), Credits(4.0));
        assert!(failed.was_billed());

        let result = PageResult::from_parts(PageParts::for_test("https://example.com/a", 503));
        assert!(!result.failed().expect("a failure").was_billed());
    }

    #[test]
    fn hints_follow_the_class() {
        let cases = [
            (403u16, Hint::TryResidentialProxy),
            (401, Hint::NeedsSession),
            (404, Hint::Permanent),
            (400, Hint::Permanent),
            (
                429,
                Hint::SlowDown {
                    after: DEFAULT_SLOW_DOWN,
                },
            ),
            (500, Hint::TryBrowser),
            (526, Hint::TryBrowser),
        ];
        for (code, hint) in cases {
            let result = PageResult::from_parts(PageParts::for_test("https://example.com/a", code));
            assert_eq!(result.hint(), Some(hint), "code {code}");
        }
        assert!(!Hint::Permanent.is_actionable());
        assert!(Hint::TryResidentialProxy.is_actionable());
        assert_eq!(Hint::for_class(PageClass::Ok), Hint::TryBrowser);
    }

    #[test]
    fn a_set_reports_what_worked_and_what_it_all_cost() {
        let pages = Pages(vec![
            PageResult::from_parts(PageParts::for_test("https://example.com/a", 200)),
            PageResult::from_parts(PageParts::for_test("https://example.com/b", 403)),
            PageResult::from_parts(PageParts::for_test("https://example.com/c", 200)),
        ]);
        assert_eq!(pages.len(), 3);
        assert_eq!(pages.ok().count(), 2);
        assert_eq!(pages.failed().count(), 1);
        assert_eq!(
            pages.first_ok().expect("a page").url.as_str(),
            "https://example.com/a"
        );
        assert_eq!(pages.total_cost(), Credits(12.0));
        assert_eq!(pages.into_ok().len(), 2);
    }

    #[test]
    fn a_page_carries_what_the_request_asked_for() {
        let mut parts = PageParts::for_test("https://example.com/a", 200);
        parts.links = Some(vec![Url::parse("https://example.com/b").expect("a url")]);
        parts.metadata = Some(Metadata {
            title: Some("Example".into()),
            ..Metadata::default()
        });
        parts.headers = Some(BTreeMap::from([(
            "content-type".to_string(),
            "text/html".to_string(),
        )]));
        parts.cookies = Some(BTreeMap::from([("a".into(), "1".into())]));
        let page = PageResult::from_parts(parts).into_ok().expect("a page");
        assert_eq!(page.links.as_ref().expect("links").len(), 1);
        assert_eq!(page.headers.as_ref().expect("headers").len(), 1);
        assert_eq!(
            page.cookies
                .as_ref()
                .and_then(|cookies| cookies.get("a"))
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(page.text(), Some("hello"));
        assert!(!page.is_blank());
        assert_eq!(page.cost(), Credits(4.0));
    }

    #[test]
    fn a_2xx_with_nothing_in_it_is_still_a_page_but_reads_as_blank() {
        let mut parts = PageParts::for_test("https://example.com/a", 200);
        parts.body = Body::Empty;
        let page = PageResult::from_parts(parts).into_ok().expect("a page");
        assert!(page.is_blank());
    }
}
