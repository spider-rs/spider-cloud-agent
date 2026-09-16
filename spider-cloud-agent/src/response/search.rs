//! Results from the search endpoint.

use serde::{Deserialize, Serialize};
use url::Url;

/// What a search returned.
///
/// The wire wraps the list in a `content` field, which is why this is a struct rather
/// than a bare vector.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SearchResults {
    /// The results, in the order the engine ranked them.
    #[serde(default)]
    pub content: Vec<SearchEntry>,
}

impl SearchResults {
    /// How many results came back.
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Whether the search returned nothing.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// The results, borrowed.
    pub fn entries(&self) -> &[SearchEntry] {
        &self.content
    }

    /// The results, owned.
    pub fn into_entries(self) -> Vec<SearchEntry> {
        self.content
    }

    /// Every result URL that parses, in rank order.
    ///
    /// A result whose URL does not parse is dropped rather than failing the search.
    pub fn urls(&self) -> Vec<Url> {
        self.content.iter().filter_map(|e| e.url()).collect()
    }
}

impl IntoIterator for SearchResults {
    type Item = SearchEntry;
    type IntoIter = std::vec::IntoIter<SearchEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.content.into_iter()
    }
}

/// One search result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SearchEntry {
    /// The result title.
    #[serde(default)]
    pub title: Option<String>,
    /// The snippet shown under the title.
    #[serde(default)]
    pub description: Option<String>,
    /// The result address, as the engine wrote it.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub url: String,
}

/// A missing or null address reads as an empty one, so one entry the engine
/// could not name does not lose the whole list.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

impl SearchEntry {
    /// The address parsed, or `None` when the engine returned something that is not a
    /// URL.
    pub fn url(&self) -> Option<Url> {
        Url::parse(&self.url).ok()
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
    fn reads_the_wire_shape() {
        let json = r#"{"content":[
            {"title":"Example","description":"A page","url":"https://example.com/a"},
            {"url":"not a url"}
        ]}"#;
        let results: SearchResults = serde_json::from_str(json).expect("search results");
        assert_eq!(results.len(), 2);
        assert_eq!(results.entries()[0].title.as_deref(), Some("Example"));
        assert!(results.entries()[1].title.is_none());
        assert_eq!(results.urls().len(), 1);
    }

    #[test]
    fn a_null_field_on_one_entry_does_not_lose_the_whole_list() {
        let json = r#"{"content":[
            {"title":null,"description":null,"url":null},
            {"title":"Example","url":"https://example.com/a"}
        ]}"#;
        let results: SearchResults = serde_json::from_str(json).expect("search results");
        assert_eq!(results.len(), 2);
        assert_eq!(results.entries()[0].url, "");
        assert!(results.entries()[0].url().is_none());
        assert_eq!(results.urls().len(), 1);
    }

    #[test]
    fn an_absent_list_reads_as_no_results() {
        let results: SearchResults = serde_json::from_str("{}").expect("search results");
        assert!(results.is_empty());
        assert_eq!(results.into_entries().len(), 0);
    }
}
