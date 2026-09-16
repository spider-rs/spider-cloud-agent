//! Turning a [`Need`] into request parameters.
//!
//! This is the table that earns the saving. Nearly all of it happens here
//! rather than in the trimming that follows, because a byte the service never
//! sends is a byte nobody pays for.
//!
//! | need | what it sends |
//! |---|---|
//! | [`Need::Text`] | `return_format=text`, `readability`, `clean_html` |
//! | [`Need::Markdown`] | the same with `return_format=markdown` |
//! | [`Need::Html`] | `return_format=raw`, `clean_html` |
//! | [`Need::Links`] | the links endpoint, `return_format=empty` |
//! | [`Need::Metadata`] | `return_format=empty`, `metadata` |
//! | [`Need::Fields`] | `css_extraction_map`, `return_format=empty` |
//! | [`Need::Screenshot`] | the screenshot endpoint, `return_format=bytes` |
//! | [`Need::Raw`] | nothing |
//!
//! Every need but [`Need::Raw`] also switches off metadata, headers, cookies,
//! page links, structured data and embeddings, unless it is the need asking for
//! them. The API returns those by default and a caller who did not name them
//! is paying for bytes they will not read.

use crate::params::{CssExtractionMap, RequestParams, ReturnFormat};
use crate::thrift::need::Need;

/// Which endpoint a need is answered by.
///
/// A need can move a request to a different path, which no request parameter
/// can express. The builder reads this and sends the call elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Endpoint {
    /// The page endpoint, which is where most needs are answered.
    Scrape,
    /// The links endpoint, which returns addresses and no content.
    Links,
    /// The picture endpoint.
    Screenshot,
}

/// The optional return fields the API is generous with.
///
/// Named here so the rule that switches them off is one list rather than six
/// scattered assignments, and so a test can walk it.
pub const OPTIONAL_RETURNS: &[&str] = &[
    "metadata",
    "return_headers",
    "return_cookies",
    "return_page_links",
    "return_json_data",
    "return_embeddings",
];

/// The parameters a [`Need`] asks for.
///
/// Every field is optional and an unset one means the plan has no opinion, so
/// a plan is something you can read and print rather than a closure over a
/// request.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Plan {
    /// The endpoint this need is answered by, when it is not the page one.
    pub endpoint: Option<Endpoint>,
    /// The shape the content comes back in.
    pub return_format: Option<ReturnFormat>,
    /// Whether the service strips navigation, adverts and the rest.
    pub readability: Option<bool>,
    /// Whether the service drops scripts, styles and comments.
    pub clean_html: Option<bool>,
    /// Whether the page's declared metadata comes back.
    pub metadata: Option<bool>,
    /// Whether the response headers come back.
    pub return_headers: Option<bool>,
    /// Whether the response cookies come back.
    pub return_cookies: Option<bool>,
    /// Whether the links found on each page come back.
    pub return_page_links: Option<bool>,
    /// Whether the page's structured data comes back.
    pub return_json_data: Option<bool>,
    /// Whether vectors for the title and description come back.
    pub return_embeddings: Option<bool>,
    /// The named extractions to pull out of the page.
    pub css_extraction_map: Option<CssExtractionMap>,
}

/// Write the planned value onto the request, unless the caller set that field
/// themselves.
macro_rules! settle {
    ($plan:expr, $params:expr, $caller:expr, $($field:ident),+ $(,)?) => {
        $(
            if $caller.$field.is_none() {
                if let Some(value) = $plan.$field.clone() {
                    $params.$field = Some(value);
                }
            }
        )+
    };
}

impl Plan {
    /// A plan with no opinion about anything, which is what [`Need::Raw`]
    /// produces.
    pub fn none() -> Plan {
        Plan::default()
    }

    /// A plan that switches off every optional return field.
    ///
    /// The starting point for every need but [`Need::Raw`]. A need that wants
    /// one of these turns it back on, and the rest stay off.
    pub fn frugal() -> Plan {
        Plan {
            metadata: Some(false),
            return_headers: Some(false),
            return_cookies: Some(false),
            return_page_links: Some(false),
            return_json_data: Some(false),
            return_embeddings: Some(false),
            ..Plan::default()
        }
    }

    /// The parameters this need asks for.
    pub fn for_need(need: &Need) -> Plan {
        match need {
            Need::Raw => Plan::none(),
            Need::Text => Plan {
                return_format: Some(ReturnFormat::Text),
                readability: Some(true),
                clean_html: Some(true),
                ..Plan::frugal()
            },
            Need::Markdown => Plan {
                return_format: Some(ReturnFormat::Markdown),
                readability: Some(true),
                clean_html: Some(true),
                ..Plan::frugal()
            },
            Need::Html => Plan {
                return_format: Some(ReturnFormat::Raw),
                clean_html: Some(true),
                ..Plan::frugal()
            },
            Need::Links => Plan {
                endpoint: Some(Endpoint::Links),
                return_format: Some(ReturnFormat::Empty),
                return_page_links: Some(true),
                ..Plan::frugal()
            },
            Need::Metadata => Plan {
                return_format: Some(ReturnFormat::Empty),
                metadata: Some(true),
                ..Plan::frugal()
            },
            Need::Fields(spec) => {
                // An empty spec would ask for no content and no fields, which
                // is a response with nothing in it. Leave the format alone and
                // let the service answer as it would.
                if spec.is_empty() {
                    Plan::frugal()
                } else {
                    Plan {
                        return_format: Some(ReturnFormat::Empty),
                        css_extraction_map: Some(spec.to_map()),
                        ..Plan::frugal()
                    }
                }
            }
            Need::Screenshot(shot) => Plan {
                endpoint: Some(Endpoint::Screenshot),
                return_format: Some(ReturnFormat::Bytes),
                metadata: Some(shot.wants_metadata()),
                ..Plan::frugal()
            },
        }
    }

    /// Write this plan onto a request, leaving the caller's own settings alone.
    ///
    /// `caller` is the request as it stood before any plan touched it. A field
    /// the caller set is theirs, and the plan does not argue. A field the
    /// caller left alone is the plan's, and it is written every time this is
    /// called, so an escalation step cannot leave a fat return format behind
    /// it.
    pub fn apply_over(&self, params: &mut RequestParams, caller: &RequestParams) {
        // The request carries a format handling, which can hold several
        // formats. A plan names one, so this pair does not go through the
        // macro.
        if caller.return_format.is_none() {
            if let Some(format) = self.return_format {
                set_format(params, format);
            }
        }
        if caller.css_extraction_map.is_none() {
            if let Some(map) = &self.css_extraction_map {
                params.css_extraction_map = Some(map.clone());
            }
        }
        settle!(
            self,
            params,
            caller,
            readability,
            clean_html,
            metadata,
            return_headers,
            return_cookies,
            return_page_links,
            return_json_data,
            return_embeddings,
        );
    }

    /// Whether this plan asks for no page bytes.
    pub fn returns_no_page(&self) -> bool {
        self.return_format == Some(ReturnFormat::Empty)
    }

    /// Whether this plan has any opinion at all.
    pub fn is_empty(&self) -> bool {
        self == &Plan::none()
    }

    /// The format this plan asks for, if any.
    pub fn format(&self) -> Option<ReturnFormat> {
        self.return_format
    }
}

/// Apply a plan's format to a request, which takes a handling rather than a
/// bare format.
pub(crate) fn set_format(params: &mut RequestParams, format: ReturnFormat) {
    params.return_format = Some(format.into());
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
    use crate::params::ReturnFormatHandling;
    use crate::thrift::need::{FieldSpec, ShotSpec};

    /// Read the plan's own fields as a list, so a test can walk them without
    /// naming each one again.
    fn optional_returns(plan: &Plan) -> Vec<(&'static str, Option<bool>)> {
        vec![
            ("metadata", plan.metadata),
            ("return_headers", plan.return_headers),
            ("return_cookies", plan.return_cookies),
            ("return_page_links", plan.return_page_links),
            ("return_json_data", plan.return_json_data),
            ("return_embeddings", plan.return_embeddings),
        ]
    }

    #[test]
    fn the_optional_return_list_matches_the_fields_a_plan_carries() {
        let named: Vec<&str> = optional_returns(&Plan::frugal())
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(named, OPTIONAL_RETURNS);
    }

    #[test]
    fn raw_has_no_opinion_and_changes_nothing() {
        let plan = Plan::for_need(&Need::Raw);
        assert!(plan.is_empty());

        let caller = RequestParams::url("https://example.com");
        let mut params = caller.clone();
        plan.apply_over(&mut params, &caller);
        assert_eq!(params, caller);
    }

    #[test]
    fn every_need_but_raw_turns_the_optional_returns_off() {
        let needs = [
            (Need::Text, None),
            (Need::Markdown, None),
            (Need::Html, None),
            (Need::Links, Some("return_page_links")),
            (Need::Metadata, Some("metadata")),
            (Need::Fields(FieldSpec::new([("price", ".price")])), None),
            (Need::Screenshot(ShotSpec::new()), None),
            (
                Need::Screenshot(ShotSpec::with_metadata()),
                Some("metadata"),
            ),
        ];
        for (need, asked_for) in needs {
            let plan = Plan::for_need(&need);
            for (name, value) in optional_returns(&plan) {
                let expected = Some(Some(name) == asked_for);
                assert_eq!(value, expected, "{} left {name} at {value:?}", need.label());
            }
        }
    }

    #[test]
    fn fields_ask_for_the_extractions_and_none_of_the_page() {
        let plan = Plan::for_need(&Need::fields([("price", ".price"), ("title", "h1")]));
        assert_eq!(plan.return_format, Some(ReturnFormat::Empty));
        assert!(plan.returns_no_page());
        let map = plan.css_extraction_map.expect("an extraction map");
        assert_eq!(map["/"].len(), 2);
    }

    #[test]
    fn an_empty_field_spec_does_not_ask_for_a_response_with_nothing_in_it() {
        let plan = Plan::for_need(&Need::Fields(FieldSpec::default()));
        assert_eq!(plan.return_format, None);
        assert!(plan.css_extraction_map.is_none());
    }

    #[test]
    fn a_setting_the_caller_made_survives_the_plan() {
        let mut caller = RequestParams::url("https://example.com");
        caller.return_format = Some(ReturnFormatHandling::Single(ReturnFormat::Raw));
        caller.metadata = Some(true);

        let mut params = caller.clone();
        let plan = Plan::for_need(&Need::Markdown);
        plan.apply_over(&mut params, &caller);

        assert_eq!(
            params.return_format,
            Some(ReturnFormatHandling::Single(ReturnFormat::Raw))
        );
        assert_eq!(params.metadata, Some(true));
        // The fields the caller said nothing about are still the plan's.
        assert_eq!(params.readability, Some(true));
        assert_eq!(params.return_cookies, Some(false));
    }

    #[test]
    fn applying_twice_lands_in_the_same_place() {
        let caller = RequestParams::url("https://example.com");
        let plan = Plan::for_need(&Need::Text);
        let mut once = caller.clone();
        plan.apply_over(&mut once, &caller);
        let mut twice = once.clone();
        plan.apply_over(&mut twice, &caller);
        assert_eq!(once, twice);
    }

    #[test]
    fn a_plan_reapplied_over_a_changed_request_puts_the_format_back() {
        let caller = RequestParams::url("https://example.com");
        let mut params = caller.clone();
        let plan = Plan::for_need(&Need::Markdown);
        plan.apply_over(&mut params, &caller);

        // Something later in the send loop asked for the whole page again.
        params.return_format = Some(ReturnFormatHandling::Single(ReturnFormat::Raw));
        plan.apply_over(&mut params, &caller);

        assert_eq!(
            params.return_format,
            Some(ReturnFormatHandling::Single(ReturnFormat::Markdown))
        );
    }

    #[test]
    fn set_format_writes_one_format_not_a_list() {
        let mut params = RequestParams::default();
        set_format(&mut params, ReturnFormat::Text);
        assert_eq!(
            params.return_format,
            Some(ReturnFormatHandling::Single(ReturnFormat::Text))
        );
    }

    #[test]
    fn endpoints_follow_the_need() {
        assert_eq!(Plan::for_need(&Need::Links).endpoint, Some(Endpoint::Links));
        assert_eq!(
            Plan::for_need(&Need::screenshot()).endpoint,
            Some(Endpoint::Screenshot)
        );
        assert_eq!(Plan::for_need(&Need::Markdown).endpoint, None);
        assert_eq!(Plan::for_need(&Need::Raw).endpoint, None);
    }
}
