//! Take a picture of a page.

use crate::client::{IntoUrl, Spider};
use crate::ops::{curated_surface, first_page, Call};
use crate::params::{RequestParams, ScreenshotParams};
use crate::response::{Body, Outcome, Page, PageResult, Pages};
use crate::thrift::Need;
use crate::transport::route;
use crate::Result;

/// A request for a picture of a page.
///
/// Built by [`Spider::screenshot`]. The body of a served page is
/// [`Body::Screenshot`], so reading it as text gives nothing on purpose.
#[derive(Debug)]
pub struct Screenshot<'a> {
    call: Call<'a>,
    shot: ScreenshotParams,
}

impl<'a> Screenshot<'a> {
    pub(crate) fn new(spider: &'a Spider, url: impl IntoUrl) -> Screenshot<'a> {
        Screenshot {
            call: Call::new(spider, url),
            shot: ScreenshotParams::default(),
        }
    }

    /// The screenshot parameters, to change directly.
    ///
    /// The fetch settings live in [`Screenshot::params_mut`], and anything set
    /// on `base` here is replaced by them when the request goes out.
    pub fn screenshot_mut(&mut self) -> &mut ScreenshotParams {
        &mut self.shot
    }

    /// Take the picture.
    pub async fn send(self) -> Result<Outcome<Page>> {
        first_page(self.send_all().await?)
    }

    /// Take the picture and keep the answer whatever it was.
    pub async fn send_all(mut self) -> Result<Outcome<Pages>> {
        // Asked for nothing in particular, this endpoint answers with the image
        // encoded as base64 text, which lands in the body as a string and never
        // reads as a picture. Asking for bytes is what makes it arrive as
        // bytes: measured on 2026-09-15, a request with no format named came
        // back as 22,960 characters of text, and the same page asked for in
        // bytes came back as a PNG starting 137, 80, 78, 71.
        //
        // A need the caller stated is theirs and is left alone.
        self.call.need.get_or_insert(Need::screenshot());
        let shot = self.shot.clone();
        let outcome = self
            .call
            .run(route::SCREENSHOT, |params| body(&shot, params))
            .await?;
        Ok(outcome.map(as_pictures))
    }
}

/// The request body: the picture settings, plus the fetch settings for the page.
fn body(shot: &ScreenshotParams, params: &RequestParams) -> ScreenshotParams {
    let mut body = shot.clone();
    body.base = params.clone();
    body
}

/// Bytes from this endpoint are a picture, which the wire does not say and the
/// caller should not have to guess.
fn as_pictures(pages: Pages) -> Pages {
    Pages(
        pages
            .0
            .into_iter()
            .map(|result| match result {
                PageResult::Ok(mut page) => {
                    if let Body::Bytes(bytes) = page.body {
                        page.body = Body::Screenshot(bytes);
                    }
                    PageResult::Ok(page)
                }
                failed => failed,
            })
            .collect(),
    )
}

curated_surface!(Screenshot, page_links);
