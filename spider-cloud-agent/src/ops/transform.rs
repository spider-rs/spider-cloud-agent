//! Convert markup you already hold, without fetching anything.

use serde::{Deserialize, Serialize};

use crate::client::Spider;
use crate::ops::{curated_surface, first_page, Call};
use crate::params::ReturnFormat;
use crate::response::{Outcome, Page, Pages};
use crate::transport::route;
use crate::Result;

/// One piece of markup to convert.
///
/// The address is optional and worth giving: stripping a page down to its
/// article works better when links can be resolved, and that needs to know where
/// the markup came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// The markup to convert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Content that is already text, when there is no markup to convert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Where the markup came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The language of the content, as a code such as `en`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

impl Document {
    /// A document from markup alone.
    pub fn html(html: impl Into<String>) -> Document {
        Document {
            html: Some(html.into()),
            ..Document::default()
        }
    }

    /// A document from text that needs no markup conversion.
    pub fn content(content: impl Into<String>) -> Document {
        Document {
            content: Some(content.into()),
            ..Document::default()
        }
    }

    /// Say where the markup came from, so links resolve.
    pub fn from_url(mut self, url: impl Into<String>) -> Document {
        self.url = Some(url.into());
        self
    }

    /// Say what language the content is in.
    pub fn in_language(mut self, lang: impl Into<String>) -> Document {
        self.lang = Some(lang.into());
        self
    }
}

/// The request body this endpoint reads.
#[derive(Debug, Clone, Serialize)]
struct TransformBody {
    data: Vec<Document>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_format: Option<ReturnFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    readability: Option<bool>,
}

/// A conversion request.
///
/// Built by [`Spider::transform`]. Nothing is fetched, so the request settings
/// that describe how to reach a page have no effect here.
#[derive(Debug)]
pub struct Transform<'a> {
    call: Call<'a>,
    docs: Vec<Document>,
    format: Option<ReturnFormat>,
}

impl<'a> Transform<'a> {
    pub(crate) fn new(spider: &'a Spider, docs: Vec<Document>) -> Transform<'a> {
        Transform {
            call: Call::bare(spider),
            docs,
            format: None,
        }
    }

    /// The shape to convert into.
    pub fn format(mut self, format: ReturnFormat) -> Transform<'a> {
        self.format = Some(format);
        self
    }

    /// Strip navigation, adverts and the rest of the furniture first.
    pub fn readability(mut self, on: bool) -> Transform<'a> {
        self.call.params.readability = Some(on);
        self
    }

    /// Convert the documents and take the first result.
    pub async fn send(self) -> Result<Outcome<Page>> {
        first_page(self.send_all().await?)
    }

    /// Convert the documents and keep every result.
    pub async fn send_all(mut self) -> Result<Outcome<Pages>> {
        let docs = std::mem::take(&mut self.docs);
        let format = self.format;
        self.call.params.return_format = format.map(Into::into);
        self.call
            .run(route::TRANSFORM, |params| TransformBody {
                data: docs.clone(),
                return_format: format,
                readability: params.readability,
            })
            .await
    }
}

curated_surface!(Transform);
