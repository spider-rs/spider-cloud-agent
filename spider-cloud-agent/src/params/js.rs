//! Waiting, scripting and page interaction.
//!
//! Everything here only takes effect when the request runs in
//! [`RequestMode::Smart`](crate::params::RequestMode::Smart) or
//! [`RequestMode::Browser`](crate::params::RequestMode::Browser). Sent with a
//! plain HTTP fetch they are ignored, and the fetch still costs what it costs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// A span of time, split the way the API writes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeout {
    /// Whole seconds. The service caps individual waits at 60.
    pub secs: u64,
    /// The remainder, in nanoseconds.
    pub nanos: u32,
}

impl Timeout {
    /// A timeout of whole seconds.
    pub const fn from_secs(secs: u64) -> Timeout {
        Timeout { secs, nanos: 0 }
    }

    /// A timeout given in milliseconds, which is how page waits are usually
    /// reasoned about.
    pub const fn from_millis(millis: u64) -> Timeout {
        Timeout {
            secs: millis / 1_000,
            nanos: ((millis % 1_000) * 1_000_000) as u32,
        }
    }
}

impl From<Duration> for Timeout {
    fn from(value: Duration) -> Timeout {
        Timeout {
            secs: value.as_secs(),
            nanos: value.subsec_nanos(),
        }
    }
}

impl From<Timeout> for Duration {
    fn from(value: Timeout) -> Duration {
        Duration::new(value.secs, value.nanos)
    }
}

/// How long to keep waiting for network traffic to settle before giving up and
/// reading the page as it stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdleNetwork {
    /// The point at which the wait ends whether or not the page settled.
    pub timeout: Timeout,
}

impl IdleNetwork {
    /// Wait up to this long.
    pub const fn new(timeout: Timeout) -> IdleNetwork {
        IdleNetwork { timeout }
    }
}

/// Wait for an element, with a point at which to stop waiting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectorWait {
    /// The point at which the wait ends whether or not the element appeared.
    pub timeout: Timeout,
    /// The element to wait on.
    pub selector: String,
}

impl SelectorWait {
    /// Wait for `selector` for at most `timeout`.
    pub fn new(selector: impl Into<String>, timeout: Timeout) -> SelectorWait {
        SelectorWait {
            timeout,
            selector: selector.into(),
        }
    }
}

/// Sit still for a fixed span, whatever the page is doing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delay {
    /// How long to sit still.
    pub timeout: Timeout,
}

impl Delay {
    /// Wait exactly this long.
    pub const fn new(timeout: Timeout) -> Delay {
        Delay { timeout }
    }
}

/// What has to happen before the page is read.
///
/// The fields combine: each one that is set has to be satisfied, and the page
/// is read once the last of them is. Set none of them and the page is read as
/// soon as it loads, which is wrong for anything that fills itself in
/// afterwards and is the usual reason a rendered fetch comes back empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitFor {
    /// Wait until the page stops making requests, then read it. The right
    /// first thing to try on a page that returns an empty shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_network: Option<IdleNetwork>,
    /// Wait for no requests at all in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_network0: Option<IdleNetwork>,
    /// Wait for traffic to drop to near nothing, which settles sooner than
    /// [`WaitFor::idle_network0`] on pages that poll in the background and
    /// would otherwise never be quiet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub almost_idle_network0: Option<IdleNetwork>,
    /// Wait for an element to exist. The most precise wait available, because
    /// you name the thing you came for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<SelectorWait>,
    /// Wait for the page structure to stop changing, optionally watching one
    /// element rather than the whole page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dom: Option<SelectorWait>,
    /// Wait a fixed span. Use it when nothing else describes the page, and
    /// expect to pay for the time whether or not it was needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<Delay>,
    /// Follow navigation the page starts on its own, such as an interstitial
    /// that forwards on. Enabled by the service unless this is set to false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_navigations: Option<bool>,
}

impl WaitFor {
    /// Wait for the network to settle, at most this long.
    pub fn idle_network(timeout: Timeout) -> WaitFor {
        WaitFor {
            idle_network: Some(IdleNetwork::new(timeout)),
            ..WaitFor::default()
        }
    }

    /// Wait for an element to exist, at most this long.
    pub fn selector(selector: impl Into<String>, timeout: Timeout) -> WaitFor {
        WaitFor {
            selector: Some(SelectorWait::new(selector, timeout)),
            ..WaitFor::default()
        }
    }

    /// Wait a fixed span.
    pub fn delay(timeout: Timeout) -> WaitFor {
        WaitFor {
            delay: Some(Delay::new(timeout)),
            ..WaitFor::default()
        }
    }

    /// True when nothing is set, so the page would be read as soon as it loads.
    pub fn is_empty(&self) -> bool {
        self == &WaitFor::default()
    }
}

/// A step to run on the page before it is read.
///
/// Steps run in order, per page, and a step that cannot find its target stops
/// that page's chain rather than failing the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WebAutomation {
    /// Run a script and carry on.
    Evaluate(String),
    /// Click the first element matching the selector.
    Click(String),
    /// Click every element matching the selector.
    ClickAll(String),
    /// Click everything on the page that can be clicked. Blunt, and mostly
    /// useful for dismissing whatever is covering the content.
    ClickAllClickable(),
    /// Click at a point, for targets that have no selector worth writing.
    ClickPoint {
        /// Distance from the left edge, in pixels.
        x: f64,
        /// Distance from the top edge, in pixels.
        y: f64,
    },
    /// Press an element and hold it down.
    ClickHold {
        /// The element to press.
        selector: String,
        /// How long to hold, in milliseconds.
        hold_for_ms: u64,
    },
    /// Press a point and hold it down.
    ClickHoldPoint {
        /// Distance from the left edge, in pixels.
        x: f64,
        /// Distance from the top edge, in pixels.
        y: f64,
        /// How long to hold, in milliseconds.
        hold_for_ms: u64,
    },
    /// Drag one element onto another.
    ClickDrag {
        /// The element to drag.
        from: String,
        /// The element to drop it on.
        to: String,
        /// A modifier key to hold during the drag.
        modifier: Option<i64>,
    },
    /// Drag from one point to another.
    ClickDragPoint {
        /// Starting distance from the left edge.
        from_x: f64,
        /// Starting distance from the top edge.
        from_y: f64,
        /// Ending distance from the left edge.
        to_x: f64,
        /// Ending distance from the top edge.
        to_y: f64,
        /// A modifier key to hold during the drag.
        modifier: Option<i64>,
    },
    /// Type into whatever currently holds the cursor.
    Type {
        /// The text to type.
        value: String,
        /// A modifier key to hold while typing.
        modifier: Option<i64>,
    },
    /// Pause for this many milliseconds.
    Wait(u64),
    /// Pause until the page navigates.
    WaitForNavigation,
    /// Pause until the page structure stops changing.
    WaitForDom {
        /// Watch one element instead of the whole page.
        selector: Option<String>,
        /// Give up after this many milliseconds.
        timeout: u32,
    },
    /// Pause until an element exists.
    WaitFor(String),
    /// Pause until an element exists, giving up after a limit.
    WaitForWithTimeout {
        /// The element to wait on.
        selector: String,
        /// Give up after this many milliseconds.
        timeout: u64,
    },
    /// Pause until an element exists, then click it. One step rather than two,
    /// so nothing can move in between.
    WaitForAndClick(String),
    /// Scroll sideways by this many pixels.
    ScrollX(i32),
    /// Scroll down by this many pixels.
    ScrollY(i32),
    /// Put a value into an input.
    Fill {
        /// The input to fill.
        selector: String,
        /// The value to put in it.
        value: String,
    },
    /// Keep scrolling while the page keeps adding content, up to this many
    /// rounds. This is how a feed that loads as you scroll gets collected.
    InfiniteScroll(u32),
    /// Take a picture of the page at this point in the chain.
    Screenshot {
        /// Capture the whole page rather than the visible window.
        full_page: bool,
        /// Leave the page background out of the image.
        omit_background: bool,
        /// Where the image is written.
        output: String,
    },
    /// Stop the chain here unless the previous step did what it said. Put it
    /// after a step whose failure makes the rest pointless.
    ValidateChain,
}

/// Steps to run, keyed by the path they apply to.
///
/// The key is a URL path, so one crawl can drive a search page and a product
/// page differently. `"/"` applies to the site root.
pub type WebAutomationMap = BTreeMap<String, Vec<WebAutomation>>;

/// Scripts to run, keyed by the path they apply to.
///
/// The value is the script source. It runs after the page loads, and what it
/// leaves behind in the page is what gets read.
pub type ExecutionScriptsMap = BTreeMap<String, String>;

/// Which parts of the page's own traffic to report back.
///
/// Turning these on adds to the response body, so ask for them when you are
/// working out why a page came back wrong, not by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventTracker {
    /// Report the responses the page received, with their byte counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses: Option<bool>,
    /// Report the requests the page sent, with the time each went out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<bool>,
    /// Report what each automation step did, including its screenshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<bool>,
}
