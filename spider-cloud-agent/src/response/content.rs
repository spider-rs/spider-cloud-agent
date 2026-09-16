//! The body of a fetched page, and the wire shape it is read from.

use bytes::Bytes;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// The content of a page, in whatever form was asked for.
///
/// Which variant arrives follows from the request. Asking for markdown gives
/// [`Body::Markdown`]; asking for several formats at once gives [`Body::Multi`]; asking
/// for named extractions and no page bytes gives [`Body::Fields`].
///
/// New variants can appear as the API grows, so match with a `_` arm.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[non_exhaustive]
pub enum Body {
    /// Plain text.
    Text(String),
    /// Markdown.
    Markdown(String),
    /// HTML, as served or as rewritten.
    Html(String),
    /// XML, which is what feeds and sitemaps come back as.
    Xml(String),
    /// Bytes that are not text, such as a PDF or an image.
    Bytes(Bytes),
    /// An encoded image of the page.
    Screenshot(Bytes),
    /// Named values from a `css_extraction_map` request, keyed by the names you gave.
    Fields(BTreeMap<String, serde_json::Value>),
    /// Named extractions alongside the requested page content.
    WithFields {
        /// Content retaining its requested format.
        content: Box<Body>,
        /// Named extractions.
        fields: BTreeMap<String, serde_json::Value>,
    },
    /// Several formats of the same page in one response.
    Multi(MultiBody),
    /// The request asked for no page bytes, or the site returned none.
    #[default]
    Empty,
}

impl Body {
    /// The content as text, when this form of the body is text.
    ///
    /// [`Body::Multi`] answers from its own preference order. Bytes and fields answer
    /// `None`, because turning either into a string is a decision the caller should
    /// make.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Body::Text(s) | Body::Markdown(s) | Body::Html(s) | Body::Xml(s) => Some(s),
            Body::Multi(m) => m.as_str(),
            Body::WithFields { content, .. } => content.as_str(),
            _ => None,
        }
    }

    /// The content as bytes, when this form of the body is bytes.
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            Body::Bytes(b) | Body::Screenshot(b) => Some(b),
            Body::Multi(m) => m.bytes.as_ref().or(m.screenshot.as_ref()),
            Body::WithFields { content, .. } => content.as_bytes(),
            _ => None,
        }
    }

    /// The named extractions, when the request asked for them.
    pub fn fields(&self) -> Option<&BTreeMap<String, serde_json::Value>> {
        match self {
            Body::Fields(f) | Body::WithFields { fields: f, .. } => Some(f),
            _ => None,
        }
    }

    /// Whether there is nothing here to read.
    ///
    /// A 2xx with an empty body is the most common way a fetch fails without saying so,
    /// which is why this is worth checking even on a success.
    pub fn is_empty(&self) -> bool {
        match self {
            Body::Empty => true,
            Body::Text(s) | Body::Markdown(s) | Body::Html(s) | Body::Xml(s) => s.trim().is_empty(),
            Body::Bytes(b) | Body::Screenshot(b) => b.is_empty(),
            Body::Fields(f) => f.is_empty(),
            Body::Multi(m) => m.is_empty(),
            Body::WithFields { content, fields } => content.is_empty() && fields.is_empty(),
        }
    }

    /// How many bytes the content takes up.
    pub fn len(&self) -> usize {
        match self {
            Body::Empty => 0,
            Body::Text(s) | Body::Markdown(s) | Body::Html(s) | Body::Xml(s) => s.len(),
            Body::Bytes(b) | Body::Screenshot(b) => b.len(),
            Body::Fields(f) => f.values().map(|v| v.to_string().len()).sum(),
            Body::Multi(m) => m.len(),
            Body::WithFields { content, fields } => {
                content.len() + fields.values().map(|v| v.to_string().len()).sum::<usize>()
            }
        }
    }
}

/// Several formats of one page, as the `content` field sends them.
///
/// The wire puts this field in three shapes: a bare string, an array of bytes, or an
/// object with one key per format. All three read into this type. A bare string lands in
/// `raw` and an array of bytes lands in `bytes`, so a caller that only ever reads one
/// field does not have to know which shape arrived.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[non_exhaustive]
pub struct MultiBody {
    /// The page as served, before any conversion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// The page as bytes, for content that is not text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Bytes>,
    /// The page as plain text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The page as markdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// The page's HTML flattened to text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html2text: Option<String>,
    /// An encoded image of the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<Bytes>,
}

/// The object keys `content` can carry, in the order [`MultiBody::as_str`] prefers them.
const MULTI_BODY_KEYS: &[&str] = &[
    "raw",
    "bytes",
    "text",
    "markdown",
    "html2text",
    "screenshot",
];

impl MultiBody {
    /// The first text format present, preferring the most processed one.
    ///
    /// Order is text, then markdown, then the flattened HTML, then the page as served.
    pub fn as_str(&self) -> Option<&str> {
        self.text
            .as_deref()
            .or(self.markdown.as_deref())
            .or(self.html2text.as_deref())
            .or(self.raw.as_deref())
    }

    /// Whether every format is absent or blank.
    pub fn is_empty(&self) -> bool {
        let blank = |s: &Option<String>| s.as_ref().is_none_or(|s| s.trim().is_empty());
        let no_bytes = |b: &Option<Bytes>| b.as_ref().is_none_or(|b| b.is_empty());
        blank(&self.raw)
            && blank(&self.text)
            && blank(&self.markdown)
            && blank(&self.html2text)
            && no_bytes(&self.bytes)
            && no_bytes(&self.screenshot)
    }

    /// The total size of every format present.
    pub fn len(&self) -> usize {
        let text = |s: &Option<String>| s.as_ref().map_or(0, |s| s.len());
        let raw = |b: &Option<Bytes>| b.as_ref().map_or(0, |b| b.len());
        text(&self.raw)
            + text(&self.text)
            + text(&self.markdown)
            + text(&self.html2text)
            + raw(&self.bytes)
            + raw(&self.screenshot)
    }

    /// Which formats the response carried.
    pub fn present(&self) -> Vec<&'static str> {
        let flags = [
            self.raw.is_some(),
            self.bytes.is_some(),
            self.text.is_some(),
            self.markdown.is_some(),
            self.html2text.is_some(),
            self.screenshot.is_some(),
        ];
        MULTI_BODY_KEYS
            .iter()
            .zip(flags)
            .filter(|(_, present)| *present)
            .map(|(key, _)| *key)
            .collect()
    }
}

impl<'de> Deserialize<'de> for MultiBody {
    /// Reads the three shapes the `content` field takes.
    ///
    /// This is written out rather than derived with `untagged` on purpose. An untagged
    /// enum tries each variant and reports only that none matched, so a change to the
    /// object's keys reads as a type error about the whole field and the actual mismatch
    /// is never named. A visitor says which shape arrived and which ones were expected.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(MultiBodyVisitor)
    }
}

struct MultiBodyVisitor;

impl<'de> Visitor<'de> for MultiBodyVisitor {
    type Value = MultiBody;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "page content as a string, as an array of bytes, or as an object with any of the keys {}",
            MULTI_BODY_KEYS.join(", ")
        )
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<MultiBody, E> {
        Ok(MultiBody {
            raw: Some(value.to_owned()),
            ..MultiBody::default()
        })
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<MultiBody, E> {
        Ok(MultiBody {
            raw: Some(value),
            ..MultiBody::default()
        })
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<MultiBody, E> {
        Ok(MultiBody {
            bytes: Some(Bytes::copy_from_slice(value)),
            ..MultiBody::default()
        })
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<MultiBody, E> {
        Ok(MultiBody {
            bytes: Some(Bytes::from(value)),
            ..MultiBody::default()
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<MultiBody, A::Error> {
        let mut buf: Vec<u8> = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(byte) = seq.next_element::<u8>()? {
            buf.push(byte);
        }
        Ok(MultiBody {
            bytes: Some(Bytes::from(buf)),
            ..MultiBody::default()
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<MultiBody, A::Error> {
        let mut out = MultiBody::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "raw" => out.raw = map.next_value()?,
                "bytes" => out.bytes = map.next_value()?,
                "text" => out.text = map.next_value()?,
                "markdown" => out.markdown = map.next_value()?,
                "html2text" => out.html2text = map.next_value()?,
                "screenshot" => out.screenshot = map.next_value()?,
                // A key this version does not know is skipped rather than rejected, so
                // a new format added server side does not break an old client. The log
                // line is there to make the drift visible.
                other => {
                    log::debug!("content object carried an unknown key: {other}");
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
        }
        Ok(out)
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
    fn reads_a_bare_string() {
        let body: MultiBody = serde_json::from_str(r#""hello""#).expect("string content");
        assert_eq!(body.raw.as_deref(), Some("hello"));
        assert_eq!(body.as_str(), Some("hello"));
        assert_eq!(body.present(), vec!["raw"]);
    }

    #[test]
    fn reads_an_array_of_bytes() {
        let body: MultiBody = serde_json::from_str("[104,105]").expect("byte array content");
        assert_eq!(body.bytes.as_deref(), Some(&b"hi"[..]));
        assert_eq!(body.as_str(), None);
        assert_eq!(body.present(), vec!["bytes"]);
    }

    #[test]
    fn reads_an_object() {
        let body: MultiBody =
            serde_json::from_str(r##"{"markdown":"# hi","raw":"<h1>hi</h1>"}"##).expect("object");
        assert_eq!(body.markdown.as_deref(), Some("# hi"));
        assert_eq!(body.raw.as_deref(), Some("<h1>hi</h1>"));
        assert_eq!(body.as_str(), Some("# hi"));
        assert_eq!(body.present(), vec!["raw", "markdown"]);
    }

    #[test]
    fn an_unexpected_shape_says_what_it_got_and_what_it_wanted() {
        let err = serde_json::from_str::<MultiBody>("42").expect_err("a number is not content");
        let message = err.to_string();
        assert!(message.contains("invalid type"), "{message}");
        assert!(message.contains("42"), "{message}");
        assert!(message.contains("array of bytes"), "{message}");
        assert!(message.contains("html2text"), "{message}");

        let err = serde_json::from_str::<MultiBody>("true").expect_err("a bool is not content");
        assert!(err.to_string().contains("boolean"), "{err}");
    }

    #[test]
    fn an_unknown_object_key_is_skipped_rather_than_fatal() {
        let body: MultiBody =
            serde_json::from_str(r#"{"text":"hi","some_new_format":{"a":1}}"#).expect("object");
        assert_eq!(body.text.as_deref(), Some("hi"));
    }

    #[test]
    fn empty_and_length_agree_with_what_is_there() {
        assert!(MultiBody::default().is_empty());
        assert!(serde_json::from_str::<MultiBody>(r#""   ""#)
            .expect("blank")
            .is_empty());
        let body: MultiBody = serde_json::from_str(r#"{"text":"hi"}"#).expect("object");
        assert!(!body.is_empty());
        assert_eq!(body.len(), 2);
    }

    #[test]
    fn body_reads_through_to_its_content() {
        assert_eq!(Body::Markdown("# hi".into()).as_str(), Some("# hi"));
        assert!(Body::Empty.is_empty());
        assert!(Body::Text("  ".into()).is_empty());
        assert_eq!(Body::Bytes(Bytes::from_static(b"abc")).len(), 3);
        assert_eq!(Body::default(), Body::Empty);
        assert_eq!(Body::Bytes(Bytes::from_static(b"abc")).as_str(), None);

        let mut fields = BTreeMap::new();
        fields.insert("price".to_string(), serde_json::json!("19.99"));
        let body = Body::Fields(fields);
        assert!(body.fields().is_some_and(|f| f.contains_key("price")));
        assert!(!body.is_empty());
    }
}
