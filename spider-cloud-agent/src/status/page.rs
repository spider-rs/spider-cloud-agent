//! The status the target site returned.
//!
//! This plane arrives inside the response body, after the call to spider.cloud already
//! succeeded. It describes the site, not your account.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The HTTP status the target site returned for the fetch.
///
/// This is the number to read when deciding whether to try again with different request
/// parameters. It is deserialized from the response body, which is the one place it can
/// legitimately come from. [`crate::status::ApiStatus`] has no `Deserialize` at all, so
/// a body can never produce one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageStatus(u16);

impl PageStatus {
    /// Record a status read out of a response body. Internal to the transport.
    pub(crate) fn new(code: u16) -> Self {
        Self(code)
    }

    /// The numeric status code, or zero when no target status was supplied.
    pub fn code(self) -> u16 {
        self.0
    }

    /// Whether the response omitted the target status.
    pub fn is_unknown(self) -> bool {
        self.0 == 0
    }

    /// What the code means for the fetch.
    pub fn class(self) -> PageClass {
        match self.0 {
            200..=299 => PageClass::Ok,
            400 => PageClass::BadRequest,
            401 => PageClass::NeedsLogin,
            403 => PageClass::Blocked,
            404 => PageClass::NotFound,
            429 => PageClass::TargetRateLimited,
            500..=599 => PageClass::ServerError,
            _ => PageClass::Other,
        }
    }

    /// Whether the fetch succeeded.
    pub fn is_ok(self) -> bool {
        matches!(self.class(), PageClass::Ok)
    }

    /// Whether this attempt is billed.
    ///
    /// A fetch that reached the site is billed even when the site refused it, because
    /// the work was done either way. The two exceptions are target 500 and 503, which
    /// are free. Everything else, a 403 and a 404 and a 429 included, costs credits.
    pub fn consumes_credits(self) -> bool {
        !matches!(self.0, 500 | 503)
    }
}

impl fmt::Display for PageStatus {
    /// Prints with a `target` prefix so a log line cannot be mistaken for the call
    /// plane.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "target status {}", self.0)
    }
}

/// What a [`PageStatus`] means for the fetch.
///
/// New variants can appear as the API grows, so match with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PageClass {
    /// The site served the page.
    Ok,
    /// The site rejected the request as malformed.
    BadRequest,
    /// The page is behind a login.
    NeedsLogin,
    /// The site refused the fetch. Heavier request parameters often get through.
    Blocked,
    /// The page is not there. Asking again will not change that.
    NotFound,
    /// The site is rate limiting the fetch. Slowing down helps, spending more does not.
    TargetRateLimited,
    /// The site failed on its side. Any target 5xx is worth retrying.
    ServerError,
    /// A status this version does not map.
    Other,
}

impl PageClass {
    /// Whether the same request is worth sending again.
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            PageClass::Blocked | PageClass::TargetRateLimited | PageClass::ServerError
        )
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
    fn maps_the_codes_it_promises() {
        let cases = [
            (200u16, PageClass::Ok),
            (201, PageClass::Ok),
            (299, PageClass::Ok),
            (400, PageClass::BadRequest),
            (401, PageClass::NeedsLogin),
            (403, PageClass::Blocked),
            (404, PageClass::NotFound),
            (429, PageClass::TargetRateLimited),
            (500, PageClass::ServerError),
            (502, PageClass::ServerError),
            (503, PageClass::ServerError),
            (504, PageClass::ServerError),
            (526, PageClass::ServerError),
            (599, PageClass::ServerError),
            (301, PageClass::Other),
            (999, PageClass::Other),
        ];
        for (code, class) in cases {
            assert_eq!(PageStatus::new(code).class(), class, "code {code}");
        }
    }

    #[test]
    fn only_target_500_and_503_are_free() {
        assert!(!PageStatus::new(500).consumes_credits());
        assert!(!PageStatus::new(503).consumes_credits());
        for code in [200u16, 400, 401, 403, 404, 429, 502, 504, 526, 999] {
            assert!(
                PageStatus::new(code).consumes_credits(),
                "code {code} should be billed"
            );
        }
        // Exhaustive sweep, so a later edit cannot widen the free set by accident.
        for code in 100u16..=1000 {
            let free = !PageStatus::new(code).consumes_credits();
            assert_eq!(free, code == 500 || code == 503, "code {code}");
        }
    }

    #[test]
    fn display_names_the_target_plane() {
        assert_eq!(PageStatus::new(403).to_string(), "target status 403");
    }

    #[test]
    fn the_two_planes_print_differently_for_the_same_number() {
        let api = crate::status::ApiStatus::new(403).to_string();
        let page = PageStatus::new(403).to_string();
        assert_ne!(api, page);
    }

    #[test]
    fn the_two_planes_are_distinct_types() {
        use std::any::TypeId;
        assert_ne!(
            TypeId::of::<PageStatus>(),
            TypeId::of::<crate::status::ApiStatus>()
        );
        // And neither is a bare integer that the other could be handed as.
        assert_ne!(TypeId::of::<PageStatus>(), TypeId::of::<u16>());
        assert_ne!(
            TypeId::of::<crate::status::ApiStatus>(),
            TypeId::of::<u16>()
        );
    }

    #[test]
    fn deserializes_transparently_from_a_body() {
        let status: PageStatus = serde_json::from_str("403").expect("a bare number");
        assert_eq!(status.code(), 403);
        assert_eq!(serde_json::to_string(&status).expect("serialize"), "403");
    }

    #[test]
    fn retryable_covers_the_target_five_hundreds() {
        assert!(PageStatus::new(502).class().is_retryable());
        assert!(PageStatus::new(526).class().is_retryable());
        assert!(!PageStatus::new(404).class().is_retryable());
    }
}
