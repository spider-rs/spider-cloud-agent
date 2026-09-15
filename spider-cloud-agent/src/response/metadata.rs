//! Page metadata the API returns when it is asked for.

use serde::{Deserialize, Serialize};

/// What the API knows about a page beyond its content.
///
/// The crate turns metadata off by default, so this is `None` on most responses. Every
/// field is optional because which ones arrive depends on the endpoint and on the page.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Metadata {
    /// The page title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The meta description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The Open Graph preview image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub og_image: Option<String>,
    /// Keywords declared by the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    /// The host the page was served from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// The path part of the final URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pathname: Option<String>,
    /// How the content was classified, such as a document or an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    /// Size of the resource in bytes, when the API could tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    /// Whatever a structured extraction produced, in its own shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_data: Option<serde_json::Value>,
    /// Whatever an automation step recorded, in its own shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_data: Option<serde_json::Value>,
}

impl Metadata {
    /// Whether the API sent nothing at all.
    pub fn is_empty(&self) -> bool {
        *self == Metadata::default()
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
    fn reads_a_sparse_block() {
        let meta: Metadata =
            serde_json::from_str(r#"{"title":"Example","og_image":null}"#).expect("metadata");
        assert_eq!(meta.title.as_deref(), Some("Example"));
        assert!(meta.og_image.is_none());
        assert!(!meta.is_empty());
    }

    #[test]
    fn reads_an_empty_block() {
        let meta: Metadata = serde_json::from_str("{}").expect("metadata");
        assert!(meta.is_empty());
    }

    #[test]
    fn unset_fields_are_not_written_back() {
        let meta = Metadata {
            title: Some("Example".into()),
            ..Metadata::default()
        };
        let json = serde_json::to_string(&meta).expect("serialize");
        assert_eq!(json, r#"{"title":"Example"}"#);
    }
}
