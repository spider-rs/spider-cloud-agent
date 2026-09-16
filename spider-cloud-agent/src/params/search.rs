//! Parameters for the search endpoint.

use serde::{Deserialize, Serialize};

use crate::params::transport::Country;
use crate::params::RequestParams;

/// How far back results may come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TimeWindow {
    /// The last hour.
    #[serde(rename = "qdr:h")]
    PastHour,
    /// The last day.
    #[serde(rename = "qdr:d")]
    PastDay,
    /// The last week.
    #[serde(rename = "qdr:w")]
    PastWeek,
    /// The last month.
    #[serde(rename = "qdr:m")]
    PastMonth,
    /// The last year.
    #[serde(rename = "qdr:y")]
    PastYear,
}

/// A search, plus the fetch settings used on whatever it finds.
///
/// The fetch settings are the ordinary ones: everything in [`RequestParams`]
/// applies to the pages behind the results, so a search that also reads the
/// pages costs a search plus that many fetches.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SearchParams {
    /// The fetch settings applied to each result.
    #[serde(default, flatten)]
    pub base: RequestParams,
    /// The query.
    pub search: String,
    /// How many results to work through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_limit: Option<u32>,
    /// Fetch each result as well as listing it. Leaving this off returns the
    /// result list alone, which is a great deal cheaper when the titles and
    /// URLs are all you needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_page_content: Option<bool>,
    /// The place to search from, written the way a person would say it, such
    /// as `Austin, Texas`. Local results differ from national ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The country to search from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<Country>,
    /// The language to search in, as a code such as `en`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Search from an exact point. Set it with [`SearchParams::longitude`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    /// Search from an exact point. Set it with [`SearchParams::latitude`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    /// How far around that point to favour, in metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<i64>,
    /// How many results one page of the search holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num: Option<u32>,
    /// Only return results published inside this window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tbs: Option<TimeWindow>,
    /// Which page of results to read, counting from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// Cap how many distinct sites the results may cover, when the query is a
    /// list of URLs or text rather than a phrase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_limit: Option<u32>,
    /// Which search back end answers the query. Leave it unset to let the
    /// service decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<SearchEngine>,
    /// Return sooner with fewer results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_search: Option<bool>,
    /// Keep reading result pages on your behalf, up to a hundred of them. The
    /// bill grows with the pages, so pair it with a credit cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pagination: Option<bool>,
}

impl std::fmt::Debug for SearchParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchParams")
            .field("base", &self.base)
            .field("search", &"<redacted>")
            .field("search_limit", &self.search_limit)
            .field("fetch_page_content", &self.fetch_page_content)
            .field("location", &self.location.as_ref().map(|_| "<redacted>"))
            .field("country", &self.country)
            .field("language", &self.language.as_ref().map(|_| "<redacted>"))
            .field("latitude", &self.latitude)
            .field("longitude", &self.longitude)
            .field("radius", &self.radius)
            .field("num", &self.num)
            .field("tbs", &self.tbs)
            .field("page", &self.page)
            .field("website_limit", &self.website_limit)
            .field("engine", &self.engine)
            .field("quick_search", &self.quick_search)
            .field("auto_pagination", &self.auto_pagination)
            .finish_non_exhaustive()
    }
}

impl SearchParams {
    /// A search for `query` with everything else left to the service.
    pub fn new(query: impl Into<String>) -> SearchParams {
        SearchParams {
            search: query.into(),
            ..SearchParams::default()
        }
    }
}

/// The search back end that answers a query.
///
/// This names a search provider, not anything about how a page is later
/// fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum SearchEngine {
    /// Google results only.
    Google,
    /// Brave results only.
    Brave,
    /// Every back end the service has, merged.
    All,
}
