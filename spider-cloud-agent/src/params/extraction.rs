//! What comes back, and how much of it.

use serde::de::{Error as DeError, SeqAccess, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// The shape the page content is returned in.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ReturnFormat {
    /// The page as fetched, markup and all. The API's own default, and the
    /// most expensive thing to hand to a model.
    #[default]
    Raw,
    /// Markdown. Keeps headings, lists and links, and drops most of the bytes.
    Markdown,
    /// Markdown restricted to the CommonMark spec, for readers that reject the
    /// extensions.
    Commonmark,
    /// Plain text with the markup taken out.
    Text,
    /// XML.
    Xml,
    /// The bytes exactly as received, for files that are not pages.
    Bytes,
    /// No content at all. Pair it with an extraction map or with metadata to
    /// get the few fields you want and none of the page.
    Empty,
}

impl ReturnFormat {
    /// The value sent on the wire.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ReturnFormat::Raw => "raw",
            ReturnFormat::Markdown => "markdown",
            ReturnFormat::Commonmark => "commonmark",
            ReturnFormat::Text => "text",
            ReturnFormat::Xml => "xml",
            ReturnFormat::Bytes => "bytes",
            ReturnFormat::Empty => "empty",
        }
    }
}

impl fmt::Display for ReturnFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One format, or several at once.
///
/// Asking for several means the response carries the page more than once, so
/// the second format doubles what crosses the wire. Ask for two only when both
/// are read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReturnFormatHandling {
    /// Return the page in one format.
    Single(ReturnFormat),
    /// Return the page in each of these formats.
    Multi(Vec<ReturnFormat>),
}

impl Default for ReturnFormatHandling {
    fn default() -> ReturnFormatHandling {
        ReturnFormatHandling::Single(ReturnFormat::Raw)
    }
}

impl From<ReturnFormat> for ReturnFormatHandling {
    fn from(value: ReturnFormat) -> ReturnFormatHandling {
        ReturnFormatHandling::Single(value)
    }
}

impl From<Vec<ReturnFormat>> for ReturnFormatHandling {
    fn from(value: Vec<ReturnFormat>) -> ReturnFormatHandling {
        ReturnFormatHandling::Multi(value)
    }
}

impl ReturnFormatHandling {
    /// Every format asked for, in the order they were given.
    pub fn formats(&self) -> &[ReturnFormat] {
        match self {
            ReturnFormatHandling::Single(format) => std::slice::from_ref(format),
            ReturnFormatHandling::Multi(formats) => formats,
        }
    }

    /// True when this format asks for a format.
    pub fn contains(&self, format: ReturnFormat) -> bool {
        self.formats().contains(&format)
    }
}

impl Serialize for ReturnFormatHandling {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ReturnFormatHandling::Single(format) => format.serialize(serializer),
            ReturnFormatHandling::Multi(formats) => formats.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ReturnFormatHandling {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ReturnFormatHandling, D::Error> {
        struct HandlingVisitor;

        // Written out rather than left to `untagged`, which reports every
        // failure as "did not match any variant" and hides which format was
        // wrong.
        impl<'de> Visitor<'de> for HandlingVisitor {
            type Value = ReturnFormatHandling;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a return format, or an array of them")
            }

            fn visit_str<E: DeError>(self, value: &str) -> Result<ReturnFormatHandling, E> {
                parse_format(value).map(ReturnFormatHandling::Single)
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<ReturnFormatHandling, A::Error> {
                let mut formats = Vec::with_capacity(seq.size_hint().unwrap_or(2));

                while let Some(format) = seq.next_element::<String>()? {
                    formats.push(parse_format::<A::Error>(&format)?);
                }

                Ok(ReturnFormatHandling::Multi(formats))
            }
        }

        deserializer.deserialize_any(HandlingVisitor)
    }
}

fn parse_format<E: DeError>(value: &str) -> Result<ReturnFormat, E> {
    match value.trim().to_ascii_lowercase().as_str() {
        "raw" => Ok(ReturnFormat::Raw),
        "markdown" => Ok(ReturnFormat::Markdown),
        "commonmark" => Ok(ReturnFormat::Commonmark),
        "text" => Ok(ReturnFormat::Text),
        "xml" => Ok(ReturnFormat::Xml),
        "bytes" => Ok(ReturnFormat::Bytes),
        "empty" => Ok(ReturnFormat::Empty),
        _ => Err(E::invalid_value(
            Unexpected::Str(value),
            &"one of raw, markdown, commonmark, text, xml, bytes or empty",
        )),
    }
}

/// An expression that picks elements out of a page.
///
/// Both dialects go on the wire as the expression itself. Which one a string
/// is gets worked out from how it starts, so `//h1` is read back as an XPath
/// and `h1` as CSS.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ExtractSelector {
    /// A CSS selector, such as `div.price > span`.
    Css(String),
    /// An XPath expression, such as `//div[@class="price"]/span`. Reach for it
    /// when the element is identified by its text or by its position rather
    /// than by a class.
    XPath(String),
}

impl ExtractSelector {
    /// Read a CSS selector.
    pub fn css(expression: impl Into<String>) -> ExtractSelector {
        ExtractSelector::Css(expression.into())
    }

    /// Read an XPath expression.
    pub fn xpath(expression: impl Into<String>) -> ExtractSelector {
        ExtractSelector::XPath(expression.into())
    }

    /// The expression itself, which is what is sent.
    pub fn as_str(&self) -> &str {
        match self {
            ExtractSelector::Css(expression) | ExtractSelector::XPath(expression) => expression,
        }
    }

    /// Classify an expression the way the service does, by its opening
    /// characters.
    pub fn parse(expression: &str) -> ExtractSelector {
        let head = expression.trim_start();

        if head.starts_with('/') || head.starts_with("./") || head.starts_with("(/") {
            ExtractSelector::XPath(expression.to_string())
        } else {
            ExtractSelector::Css(expression.to_string())
        }
    }
}

impl fmt::Display for ExtractSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ExtractSelector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ExtractSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<ExtractSelector, D::Error> {
        let expression = String::deserialize(deserializer)?;

        Ok(ExtractSelector::parse(&expression))
    }
}

/// One named field and the expressions that fill it.
///
/// The expressions are tried in order, so put the precise one first and a
/// looser fallback after it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectorGroup {
    /// The name this field takes in the response.
    pub name: String,
    /// The expressions that find it.
    pub selectors: Vec<ExtractSelector>,
}

impl SelectorGroup {
    /// A field named `name`, found by these expressions.
    pub fn new(
        name: impl Into<String>,
        selectors: impl IntoIterator<Item = ExtractSelector>,
    ) -> SelectorGroup {
        SelectorGroup {
            name: name.into(),
            selectors: selectors.into_iter().collect(),
        }
    }

    /// A field found by CSS selectors.
    pub fn css<S: Into<String>>(
        name: impl Into<String>,
        selectors: impl IntoIterator<Item = S>,
    ) -> SelectorGroup {
        SelectorGroup::new(name, selectors.into_iter().map(ExtractSelector::css))
    }

    /// A field found by XPath expressions.
    pub fn xpath<S: Into<String>>(
        name: impl Into<String>,
        selectors: impl IntoIterator<Item = S>,
    ) -> SelectorGroup {
        SelectorGroup::new(name, selectors.into_iter().map(ExtractSelector::xpath))
    }
}

/// Fields to pull out, keyed by the path they apply to.
///
/// The key is a URL path, so a crawl can pull different fields from a listing
/// page than from a product page. `"/"` applies to the site root.
pub type CssExtractionMap = BTreeMap<String, Vec<SelectorGroup>>;

/// What a chunk is measured in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ChunkingKind {
    /// Count words.
    #[default]
    ByWords,
    /// Count lines.
    ByLines,
    /// Count characters.
    ByCharacterLength,
    /// Break on sentence ends, which keeps a chunk readable on its own.
    BySentence,
}

/// Split the content into pieces before returning it.
///
/// Set this when the content is going straight into something with a context
/// limit, so the split happens once on the service rather than again in every
/// caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkingAlg {
    /// What is being counted.
    pub r#type: ChunkingKind,
    /// How many of them go in a chunk.
    pub value: i32,
}

impl ChunkingAlg {
    /// Split every `value` units of `kind`.
    pub const fn new(kind: ChunkingKind, value: i32) -> ChunkingAlg {
        ChunkingAlg {
            r#type: kind,
            value,
        }
    }
}
