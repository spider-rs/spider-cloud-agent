//! Run a query, and fetch what it finds when you ask for that.

use crate::client::Spider;
use crate::ops::{curated_surface, Call};
use crate::params::SearchParams;
use crate::response::{Outcome, Pages, SearchResults};
use crate::transport::route;
use crate::Result;

/// A query.
///
/// Built by [`Spider::search`]. Listing results is cheap. Reading the pages
/// behind them costs a search plus one fetch per result, so it is off until
/// [`Search::fetch_pages`] turns it on.
#[derive(Debug)]
pub struct Search<'a> {
    call: Call<'a>,
    search: SearchParams,
}

impl<'a> Search<'a> {
    pub(crate) fn new(spider: &'a Spider, query: impl Into<String>) -> Search<'a> {
        Search {
            call: Call::bare(spider),
            search: SearchParams::new(query),
        }
    }

    /// How many results to work through.
    pub fn limit(mut self, results: u32) -> Search<'a> {
        self.search.search_limit = Some(results);
        self
    }

    /// Read the page behind each result as well as listing it.
    ///
    /// This is the expensive switch on this endpoint: every result becomes a
    /// fetch, and a query returning fifty of them is fifty fetches.
    pub fn fetch_pages(mut self, on: bool) -> Search<'a> {
        self.search.fetch_page_content = Some(on);
        self
    }

    /// The search parameters, to change directly.
    ///
    /// The escape hatch for this endpoint, alongside
    /// [`Search::params_mut`], which reaches the fetch settings applied to each
    /// result.
    pub fn search_mut(&mut self) -> &mut SearchParams {
        &mut self.search
    }

    /// Run the query and list what it found.
    pub async fn send(mut self) -> Result<Outcome<SearchResults>> {
        let search = self.search.clone();
        self.call
            .run_json(route::SEARCH, |params| body(&search, params))
            .await
    }

    /// Run the query and read the pages behind the results.
    ///
    /// Turns [`Search::fetch_pages`] on, because asking for pages and not paying
    /// for them returns nothing and looks like a bug.
    pub async fn send_all(mut self) -> Result<Outcome<Pages>> {
        self.search.fetch_page_content = Some(true);
        let search = self.search.clone();
        self.call
            .run(route::SEARCH, |params| body(&search, params))
            .await
    }
}

/// The request body: the query, plus the fetch settings applied to each result.
fn body(search: &SearchParams, params: &crate::params::RequestParams) -> SearchParams {
    let mut body = search.clone();
    body.base = params.clone();
    body
}

curated_surface!(Search);
