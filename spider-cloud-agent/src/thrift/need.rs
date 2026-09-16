//! What the caller actually wants back.
//!
//! A [`Need`] is a statement of purpose, not a set of parameters. The
//! parameters follow from it in [`crate::thrift::plan`], which is where the
//! saving is made.

use crate::params::{CssExtractionMap, ExtractSelector, SelectorGroup};

/// The path an extraction map applies to when the caller names no other.
///
/// The service keys extractions by path so a crawl can pull different fields
/// off a listing page than off a product page. One address needs one key, and
/// this is it.
pub const ROOT_PATH: &str = "/";

/// What the caller wants back.
///
/// Each variant answers to a set of request parameters. Everything the variant
/// did not ask for is switched off, which inverts the service's own defaults:
/// the API returns everything unless told otherwise, and this returns nothing
/// unless asked.
///
/// New variants can appear as the API grows, so match with a `_` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Need {
    /// The page as plain text, with the furniture stripped.
    Text,
    /// The page as markdown, with the furniture stripped. Headings and lists
    /// survive, most of the bytes do not.
    Markdown,
    /// The markup, cleaned of scripts, styles and comments.
    Html,
    /// The links on the page and none of its content.
    Links,
    /// The title, description and the rest of the page's declared metadata.
    Metadata,
    /// Named values pulled out of the page, and no page bytes at all. The
    /// cheapest thing this crate can ask for.
    Fields(FieldSpec),
    /// A picture of the page.
    Screenshot(ShotSpec),
    /// Whatever the service would return on its own. The caller has opted out
    /// of this layer and nothing is switched off on their behalf.
    Raw,
}

impl Need {
    /// Ask for named fields, given pairs of name and selector.
    ///
    /// A selector starting with a slash is read as XPath and anything else as
    /// CSS, the same way the service reads it.
    ///
    /// ```
    /// use spider_cloud_agent::Need;
    ///
    /// let need = Need::fields([("price", ".price"), ("title", "h1")]);
    /// ```
    pub fn fields<N: Into<String>, S: AsRef<str>>(pairs: impl IntoIterator<Item = (N, S)>) -> Need {
        Need::Fields(FieldSpec::new(pairs))
    }

    /// Ask for a picture of the page.
    pub fn screenshot() -> Need {
        Need::Screenshot(ShotSpec::default())
    }

    /// Whether this need leaves the service's own defaults alone.
    pub fn is_raw(&self) -> bool {
        matches!(self, Need::Raw)
    }

    /// A short name for the need, safe to log.
    pub fn label(&self) -> &'static str {
        match self {
            Need::Text => "text",
            Need::Markdown => "markdown",
            Need::Html => "html",
            Need::Links => "links",
            Need::Metadata => "metadata",
            Need::Fields(_) => "fields",
            Need::Screenshot(_) => "screenshot",
            Need::Raw => "raw",
        }
    }
}

/// The fields to pull out of a page, and the path they apply to.
///
/// Each field is one name and the expressions that fill it. Several
/// expressions on one name are tried in order, so the precise one goes first
/// and the loose one after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldSpec {
    path: String,
    groups: Vec<SelectorGroup>,
}

impl FieldSpec {
    /// Build a spec from pairs of name and selector, applying to the site root.
    pub fn new<N: Into<String>, S: AsRef<str>>(
        pairs: impl IntoIterator<Item = (N, S)>,
    ) -> FieldSpec {
        let mut spec = FieldSpec {
            path: ROOT_PATH.to_string(),
            groups: Vec::new(),
        };
        for (name, selector) in pairs {
            spec.push(name, selector.as_ref());
        }
        spec
    }

    /// Apply these fields to one path rather than to the site root.
    ///
    /// Use it on a crawl, where the fields worth pulling off `/products/*`
    /// differ from the ones on the front page.
    pub fn at(mut self, path: impl Into<String>) -> FieldSpec {
        self.path = path.into();
        self
    }

    /// Add another expression for a field, or add the field if it is new.
    pub fn push(&mut self, name: impl Into<String>, selector: &str) {
        let name = name.into();
        let parsed = ExtractSelector::parse(selector);
        match self.groups.iter_mut().find(|group| group.name == name) {
            Some(group) => group.selectors.push(parsed),
            None => self.groups.push(SelectorGroup::new(name, [parsed])),
        }
    }

    /// The path these fields apply to.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The fields, in the order they were given.
    pub fn groups(&self) -> &[SelectorGroup] {
        &self.groups
    }

    /// The names the response will be keyed by.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.groups.iter().map(|group| group.name.as_str())
    }

    /// Whether there is nothing to extract, which would leave a request asking
    /// for no content and no fields.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The extraction map to send.
    pub fn to_map(&self) -> CssExtractionMap {
        let mut map = CssExtractionMap::new();
        map.insert(self.path.clone(), self.groups.clone());
        map
    }
}

/// How a picture is asked for.
///
/// The one choice here is whether the page's declared metadata comes back
/// alongside the image. It does not by default, because a picture and a title
/// are rarely wanted by the same caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShotSpec {
    with_metadata: bool,
}

impl ShotSpec {
    /// A picture on its own.
    pub const fn new() -> ShotSpec {
        ShotSpec {
            with_metadata: false,
        }
    }

    /// A picture, plus the page's declared metadata.
    pub const fn with_metadata() -> ShotSpec {
        ShotSpec {
            with_metadata: true,
        }
    }

    /// Whether metadata was asked for alongside the picture.
    pub const fn wants_metadata(&self) -> bool {
        self.with_metadata
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
    fn the_headline_constructor_reads_the_way_the_readme_writes_it() {
        let need = Need::fields([("price", ".price"), ("title", "h1")]);
        let Need::Fields(spec) = &need else {
            panic!("expected fields, got {need:?}");
        };
        assert_eq!(spec.names().collect::<Vec<_>>(), vec!["price", "title"]);
        assert_eq!(spec.path(), ROOT_PATH);
        assert_eq!(need.label(), "fields");
    }

    #[test]
    fn a_selector_is_classified_the_way_the_service_classifies_it() {
        let spec = FieldSpec::new([("price", ".price"), ("heading", "//h1")]);
        let map = spec.to_map();
        let groups = &map[ROOT_PATH];
        assert_eq!(
            groups[0].selectors[0],
            ExtractSelector::Css(".price".into())
        );
        assert_eq!(
            groups[1].selectors[0],
            ExtractSelector::XPath("//h1".into())
        );
    }

    #[test]
    fn two_selectors_on_one_name_become_one_field_with_a_fallback() {
        let mut spec = FieldSpec::new([("price", ".price")]);
        spec.push("price", "[itemprop=price]");
        assert_eq!(spec.groups().len(), 1);
        assert_eq!(spec.groups()[0].selectors.len(), 2);
    }

    #[test]
    fn a_spec_can_be_pinned_to_a_path() {
        let spec = FieldSpec::new([("price", ".price")]).at("/products");
        assert!(spec.to_map().contains_key("/products"));
        assert!(!spec.to_map().contains_key(ROOT_PATH));
    }

    #[test]
    fn an_empty_spec_says_so() {
        assert!(FieldSpec::new(Vec::<(String, String)>::new()).is_empty());
        assert!(!FieldSpec::new([("a", "b")]).is_empty());
    }

    #[test]
    fn a_picture_carries_no_metadata_unless_it_was_asked_for() {
        assert!(!ShotSpec::new().wants_metadata());
        assert!(!ShotSpec::default().wants_metadata());
        assert!(ShotSpec::with_metadata().wants_metadata());
        assert!(Need::screenshot().label() == "screenshot");
    }

    #[test]
    fn only_raw_opts_out() {
        assert!(Need::Raw.is_raw());
        for need in [
            Need::Text,
            Need::Markdown,
            Need::Html,
            Need::Links,
            Need::Metadata,
            Need::fields([("a", "b")]),
            Need::screenshot(),
        ] {
            assert!(!need.is_raw(), "{need:?} should not read as raw");
        }
    }
}
