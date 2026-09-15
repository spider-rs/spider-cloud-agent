//! How a request reaches the target: mode, proxy pool, country, presentation.

use serde::{Deserialize, Serialize};

// The routing vocabulary is defined once, in spider-route, because the router
// has to name these values and cannot depend on this crate. Re-exported here so
// callers reach them at the path they would expect.
pub use spider_route::{Country, InvalidCountry, ProxyPool, RequestMode, PROXY_POOLS};

/// The window the page is laid out in.
///
/// Sites serve different markup at different widths, so this decides what the
/// extraction sees as much as it decides what a screenshot looks like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// Layout width in pixels.
    pub width: u32,
    /// Layout height in pixels.
    pub height: u32,
    /// Pixel ratio. Two gives an image at twice the pixel count, and costs the
    /// bytes to match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_scale_factor: Option<f64>,
    /// Present the page as a handheld screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulating_mobile: Option<bool>,
    /// Lay the screen out wider than it is tall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_landscape: Option<bool>,
    /// Report a touch screen, which some sites read before choosing a layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_touch: Option<bool>,
}

impl Default for Viewport {
    fn default() -> Viewport {
        Viewport::desktop()
    }
}

impl Viewport {
    /// A wide screen at a plain pixel ratio.
    pub const fn desktop() -> Viewport {
        Viewport {
            width: 1920,
            height: 1080,
            device_scale_factor: None,
            emulating_mobile: None,
            is_landscape: None,
            has_touch: None,
        }
    }

    /// A tall handheld screen with touch reported.
    pub const fn handheld() -> Viewport {
        Viewport {
            width: 390,
            height: 844,
            device_scale_factor: Some(3.0),
            emulating_mobile: Some(true),
            is_landscape: Some(false),
            has_touch: Some(true),
        }
    }
}

/// A coarse identity for the request to present.
///
/// This is one dial with three settings, not a fingerprint editor. It fills in
/// a user agent and the viewport that belongs with it, and leaves everything
/// else to the service, which tracks what sites accept far more closely than a
/// client can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Profile {
    /// A wide screen reader of ordinary web pages. The default.
    #[default]
    Desktop,
    /// A handheld screen. Worth trying when a site keeps a lighter page for
    /// small screens, which is often smaller to fetch and easier to read.
    MobileDevice,
    /// Say plainly that this is an automated fetch, with a user agent naming
    /// this crate and a link back. Use it where you would rather be turned
    /// away than be mistaken for a person.
    Bot,
}

/// The user agent sent under [`Profile::Bot`].
pub const BOT_USER_AGENT: &str = concat!(
    "spider-cloud-agent/",
    env!("CARGO_PKG_VERSION"),
    " (+https://spider.cloud)"
);

impl Profile {
    /// The user agent this profile sends, if it pins one.
    ///
    /// `Desktop` and `MobileDevice` return `None` on purpose: the service picks
    /// a current, consistent identity for them per request, and a string frozen
    /// into a released client would go stale and start standing out.
    pub const fn user_agent(&self) -> Option<&'static str> {
        match self {
            Profile::Desktop | Profile::MobileDevice => None,
            Profile::Bot => Some(BOT_USER_AGENT),
        }
    }

    /// The window size that goes with this profile.
    pub const fn viewport(&self) -> Viewport {
        match self {
            Profile::Desktop | Profile::Bot => Viewport::desktop(),
            Profile::MobileDevice => Viewport::handheld(),
        }
    }
}

/// What to do when the target answers with a redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
#[non_exhaustive]
pub enum RedirectPolicy {
    /// Follow redirects anywhere, including to another host.
    Loose,
    /// Follow redirects that stay on the host that was asked for. The default,
    /// and what keeps a crawl from wandering off a domain.
    #[default]
    Strict,
}

/// Where to send progress, and which events are worth sending.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookSettings {
    /// The URL that receives the calls.
    pub destination: String,
    /// Call when the account runs out of credits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_credits_depleted: Option<bool>,
    /// Call once half of the credits are gone, which is the point at which a
    /// long crawl can still be stopped in time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_credits_half_depleted: Option<bool>,
    /// Call when a site's crawl status changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_website_status: Option<bool>,
    /// Call for each page found, with its links and byte count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_find: Option<bool>,
    /// Include the page metadata in the find call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_find_metadata: Option<bool>,
}

impl WebhookSettings {
    /// A webhook that sends nothing until it is asked to.
    pub fn new(destination: impl Into<String>) -> WebhookSettings {
        WebhookSettings {
            destination: destination.into(),
            ..WebhookSettings::default()
        }
    }
}
