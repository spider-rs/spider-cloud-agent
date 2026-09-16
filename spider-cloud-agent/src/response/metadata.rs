//! Page metadata the API returns when it is asked for.

use serde::{Deserialize, Serialize};

/// What the API knows about a page beyond its content.
///
/// The crate turns metadata off by default, so this is `None` on most responses. Every
/// field is optional because which ones arrive depends on the endpoint and on the page.
///
/// A request for several formats at once gets a different shape: the block is
/// keyed by format name, and each entry is a whole metadata block for that
/// rendering. Reading that into one struct would leave every named field empty
/// and merge nothing, so the per-format blocks land in [`Metadata::extra`]
/// under their format names, and [`Metadata::for_format`] reads one out.
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
    /// The address that was asked for, before any redirect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_url: Option<String>,
    /// The address the page was served from, when a redirect moved it. The
    /// service writes null when nothing moved, so this is `None` unless the
    /// two differ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    /// The crawl this page belongs to, when the service assigned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crawl_id: Option<String>,
    /// A vector for the title and description, when the request set
    /// `return_embeddings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f64>>,
    /// Everything else the service sent, under the key it used.
    ///
    /// This is where a video transcript (`yt_transcript`), a maps place
    /// (`maps_place`) and the per-format blocks of a multi-format reply
    /// arrive, and where a field the service adds tomorrow lands rather than
    /// being dropped. The shapes belong to the service.
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Metadata {
    /// Whether the API sent nothing at all.
    pub fn is_empty(&self) -> bool {
        *self == Metadata::default()
    }

    /// The metadata block for one format of a multi-format reply, by the
    /// format name the service keys it under, such as `markdown` or `raw`.
    ///
    /// `None` when the reply was not multi-format or did not carry that
    /// format. A single-format reply has its fields on this struct directly.
    pub fn for_format(&self, format: &str) -> Option<Metadata> {
        let block = self.extra.get(format)?;
        if !block.is_object() {
            return None;
        }
        serde_json::from_value(block.clone()).ok()
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
    fn what_the_service_adds_is_kept_under_its_own_key() {
        let meta: Metadata = serde_json::from_str(
            r#"{"title":"Example","original_url":"https://example.com/a",
                "final_url":null,"crawl_id":"3f1c","embedding":[0.25,0.5],
                "yt_transcript":[{"text":"hello"}],"next_year":true}"#,
        )
        .expect("metadata");
        assert_eq!(meta.original_url.as_deref(), Some("https://example.com/a"));
        assert!(meta.final_url.is_none());
        assert_eq!(meta.crawl_id.as_deref(), Some("3f1c"));
        assert_eq!(meta.embedding.as_deref(), Some(&[0.25, 0.5][..]));
        assert_eq!(meta.extra["yt_transcript"][0]["text"], "hello");
        assert_eq!(meta.extra["next_year"], true);
        assert!(meta.for_format("markdown").is_none());
    }

    #[test]
    fn a_multi_format_block_keeps_each_format_apart() {
        let meta: Metadata = serde_json::from_str(
            r#"{"markdown":{"title":"As markdown","embedding":[0.1]},
                "raw":{"title":"As html"},"yt_transcript":[]}"#,
        )
        .expect("metadata");
        assert!(meta.title.is_none());
        let markdown = meta.for_format("markdown").expect("markdown block");
        assert_eq!(markdown.title.as_deref(), Some("As markdown"));
        assert_eq!(markdown.embedding.as_deref(), Some(&[0.1][..]));
        assert_eq!(
            meta.for_format("raw").and_then(|m| m.title).as_deref(),
            Some("As html")
        );
        assert!(meta.for_format("yt_transcript").is_none());
        assert!(meta.for_format("text").is_none());
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
