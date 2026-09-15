//! Read one page.

use crate::client::{IntoUrl, Spider};
use crate::ops::{curated_surface, first_page, Call};
use crate::response::{Outcome, Page, Pages};
use crate::transport::route;
use crate::Result;

/// A request for one page.
///
/// Built by [`Spider::scrape`]. This is the operation nearly every run is made
/// of: one address, one page back.
#[derive(Debug)]
pub struct Scrape<'a> {
    call: Call<'a>,
}

impl<'a> Scrape<'a> {
    pub(crate) fn new(spider: &'a Spider, url: impl IntoUrl) -> Scrape<'a> {
        Scrape {
            call: Call::new(spider, url),
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
    ///
    /// A refusal is a result here rather than an error, which is what you want
    /// when the refusal itself is the thing being measured.
    pub async fn send_all(mut self) -> Result<Outcome<Pages>> {
        self.call.run(route::SCRAPE, |params| params.clone()).await
    }
}

curated_surface!(Scrape);
