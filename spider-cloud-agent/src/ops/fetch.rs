//! Read one path under the stored config for it.

use crate::client::Spider;
use crate::ops::{curated_surface, first_page, Call};
use crate::response::{Outcome, Page, Pages};
use crate::transport::route;
use crate::Result;

/// A request for one path on one host, answered under a stored config.
///
/// Built by [`Spider::fetch`], and not the same operation as
/// [`Spider::scrape`]. Scrape sends an address and the settings you chose.
/// This names the host and the path in the address itself, and the service
/// answers with a configuration it already holds for that path: which
/// selectors to pull, whether the page needs a browser, what to wait for. The
/// body is overrides on top of that rather than the whole request, so the
/// settings on this builder are worth less here than they are on a scrape and
/// the service's own answer is usually the better one.
///
/// Three things to know before calling it. The first call on a path the service
/// has never seen goes away and works one out, which takes far longer than a
/// scrape and can fail with a 503 saying so. The query string is not part of
/// the target, so the address is the host and the path and nothing else. And
/// the answer usually arrives as [`crate::Body::Fields`] rather than as page
/// text, because pulling named fields is what the stored config is for.
///
/// Naming a [`crate::Need`] here argues with that config and generally wins.
/// Measured against example.com on 2026-09-15, asking for markdown cost 0.1087
/// credits and still came back as fields; leaving the need alone cost 0.0090
/// for the same answer.
///
/// ```no_run
/// # async fn run() -> spider_cloud_agent::Result<()> {
/// # let spider = spider_cloud_agent::Spider::new()?;
/// let page = spider.fetch("example.com", "/").send().await?;
/// println!("{}", page.body.len());
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Fetch<'a> {
    call: Call<'a>,
    domain: String,
    path: String,
}

impl<'a> Fetch<'a> {
    pub(crate) fn new(
        spider: &'a Spider,
        domain: impl Into<String>,
        path: impl Into<String>,
    ) -> Fetch<'a> {
        let domain = domain.into();
        let path = path.into();
        // The route template already carries the slash between the two slots,
        // so a path handed in with one would render a doubled slash and reach
        // a different page than the caller named.
        let rendered = path.trim_start_matches('/').to_string();
        let mut call = Call::new(spider, format!("https://{domain}/{rendered}").as_str());
        // The target is the address, not a field in the body. The service
        // refuses a `url` override outright, so sending one would be bytes
        // paid for and dropped.
        call.params.url = None;
        Fetch {
            call,
            domain,
            path: rendered,
        }
    }

    /// Read the page.
    ///
    /// Fails with [`crate::Error::Exhausted`] when the site never served it,
    /// carrying the last failure and what the attempts cost.
    pub async fn send(self) -> Result<Outcome<Page>> {
        first_page(self.send_all().await?)
    }

    /// Read the page and keep the answer whatever it was.
    pub async fn send_all(mut self) -> Result<Outcome<Pages>> {
        let domain = self.domain.clone();
        let path = self.path.clone();
        self.call
            .run_at(route::FETCH, &[&domain, &path], |params| params.clone())
            .await
    }
}

curated_surface!(Fetch, page_links);
