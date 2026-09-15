//! Fetch a site, following its links.

use crate::client::{IntoUrl, Spider};
use crate::ops::{curated_surface, first_page, Call};
use crate::response::{Outcome, Page, Pages};
use crate::transport::route;
use crate::Result;

/// A request for a site.
///
/// Built by [`Spider::crawl`]. A crawl with no caps will read the whole site, so
/// set a page limit or a budget unless that is what you meant.
#[derive(Debug)]
pub struct Crawl<'a> {
    call: Call<'a>,
}

impl<'a> Crawl<'a> {
    pub(crate) fn new(spider: &'a Spider, url: impl IntoUrl) -> Crawl<'a> {
        Crawl {
            call: Call::new(spider, url),
        }
    }

    /// How many pages the crawl may visit.
    ///
    /// Not part of the curated surface for the other endpoints, because it only
    /// means anything here.
    pub fn limit(mut self, pages: u32) -> Crawl<'a> {
        self.call.params.limit = Some(pages);
        self
    }

    /// How many links deep from the starting page the crawl may go.
    pub fn depth(mut self, depth: u32) -> Crawl<'a> {
        self.call.params.depth = Some(depth);
        self
    }

    /// Every page the crawl found, served or not.
    ///
    /// This is the one to use. A crawl of any size will have refusals in it, and
    /// which pages those were is usually the point.
    pub async fn send_all(mut self) -> Result<Outcome<Pages>> {
        self.call.run(route::CRAWL, |params| params.clone()).await
    }

    /// The first page the crawl served.
    ///
    /// Useful when the crawl was a way of reaching one page, and wrong when it
    /// was a crawl.
    pub async fn send(self) -> Result<Outcome<Page>> {
        first_page(self.send_all().await?)
    }
}

curated_surface!(Crawl);
