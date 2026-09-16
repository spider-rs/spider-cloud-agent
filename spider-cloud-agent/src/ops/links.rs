//! Collect the links on a page without paying for its content.
//!
//! A caller who wants the content as well does not need this operation. Every
//! page operation takes `page_links`, and the links then arrive in the same
//! answer as the page.

use url::Url;

use crate::client::{IntoUrl, Spider};
use crate::ops::{curated_surface, Call};
use crate::response::{Outcome, Pages};
use crate::thrift::Need;
use crate::transport::route;
use crate::Result;

/// A request for the links on a page.
///
/// Built by [`Spider::links`].
#[derive(Debug)]
pub struct Links<'a> {
    call: Call<'a>,
}

impl<'a> Links<'a> {
    pub(crate) fn new(spider: &'a Spider, url: impl IntoUrl) -> Links<'a> {
        Links {
            call: Call::new(spider, url),
        }
    }

    /// The links, in the order they were found, with repeats removed.
    ///
    /// Pages the site refused contribute nothing and do not fail the call, so an
    /// empty list means no page was read rather than no links existed.
    pub async fn send(self) -> Result<Outcome<Vec<Url>>> {
        Ok(self.send_all().await?.map(|pages| pages.links()))
    }

    /// Every page the request touched, links and refusals alike.
    pub async fn send_all(mut self) -> Result<Outcome<Pages>> {
        self.call.params.return_page_links = Some(true);
        // This endpoint answers with addresses and no page, so the request has
        // to say that a body is not wanted. Without it the send loop reads the
        // first answer as a blank page and climbs the whole ladder against a
        // call that had already succeeded. Measured on 2026-09-15: five
        // attempts, 164 s and 0.4446 credits, ending in an error, for an answer
        // the first attempt returned in 820 ms for 0.0087.
        //
        // A need the caller stated is theirs and is left alone.
        self.call.need.get_or_insert(Need::Links);
        self.call.run(route::LINKS, |params| params.clone()).await
    }
}

curated_surface!(Links);
