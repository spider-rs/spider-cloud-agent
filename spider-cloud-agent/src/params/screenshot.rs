//! Parameters only the screenshot endpoint reads.

use serde::{Deserialize, Serialize};

use crate::params::RequestParams;

/// A screenshot request: the ordinary fetch settings plus the ones that shape
/// the picture.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotParams {
    /// The fetch settings for the page being pictured.
    #[serde(default, flatten)]
    pub base: RequestParams,
    /// Capture the whole page rather than the visible window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_page: Option<bool>,
    /// Send the image as bytes instead of base64 text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<bool>,
    /// Keep images from loading before the capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_images: Option<bool>,
    /// Make the default white background transparent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omit_background: Option<bool>,
    /// On by default. Turn it off for a slower render that gets frames, PDFs
    /// and fine detail right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    /// Chrome DevTools `Page.captureScreenshot` options, such as `format`,
    /// `quality` and `clip`, in the protocol's own field names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdp_params: Option<serde_json::Map<String, serde_json::Value>>,
}
