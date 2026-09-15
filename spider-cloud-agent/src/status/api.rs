//! The status of your call to spider.cloud.
//!
//! This plane is about the account and the request envelope: whether the key is
//! accepted, whether there are credits left, whether the service is shedding load. It
//! says nothing about the site you asked for.

use std::fmt;
use std::time::Duration;

/// The HTTP status of the call to spider.cloud.
///
/// Only the transport constructs one, which is why there is no public `From<u16>` and
/// no `Deserialize`. A number read out of a response body belongs to the other plane,
/// [`crate::status::PageStatus`], and the type system refuses to mix them up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct ApiStatus(u16);

impl ApiStatus {
    /// Record a status seen on the wire. Internal to the transport.
    pub(crate) fn new(code: u16) -> Self {
        Self(code)
    }

    /// The numeric status code.
    pub fn code(self) -> u16 {
        self.0
    }

    /// What the code means for the caller.
    ///
    /// Rate limiting and load shedding can carry a wait time in a response header. This
    /// method has no access to headers, so it reports `None` for both. Use
    /// [`ApiStatus::class_with_retry_after`] when the header was read.
    pub fn class(self) -> ApiClass {
        self.class_with_retry_after(None)
    }

    /// The same mapping as [`ApiStatus::class`], carrying a wait time taken from the
    /// response headers.
    pub fn class_with_retry_after(self, retry_after: Option<Duration>) -> ApiClass {
        match self.0 {
            200 => ApiClass::Ok,
            204 => ApiClass::NoContent,
            400 => ApiClass::BadRequest,
            401 => ApiClass::Unauthorized,
            402 => ApiClass::InsufficientCredits,
            413 => ApiClass::PayloadTooLarge,
            429 => ApiClass::RateLimited { retry_after },
            500 => ApiClass::ServerError,
            503 => ApiClass::Draining { retry_after },
            _ => ApiClass::Other,
        }
    }

    /// Whether the call returned a body worth reading.
    pub fn is_success(self) -> bool {
        (200..300).contains(&self.0)
    }
}

impl fmt::Display for ApiStatus {
    /// Prints with an `api` prefix so a log line cannot be mistaken for the target
    /// plane.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "api status {}", self.0)
    }
}

/// What an [`ApiStatus`] means for the caller.
///
/// New variants can appear as the API grows, so match with a `_` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApiClass {
    /// The call succeeded and the body holds a result.
    Ok,
    /// The call succeeded and there is nothing to return.
    NoContent,
    /// The request was malformed. Sending it again unchanged gets the same answer.
    BadRequest,
    /// The key was missing or rejected.
    Unauthorized,
    /// The account is out of credits. Never retry this one.
    InsufficientCredits,
    /// The request body was larger than the endpoint accepts.
    PayloadTooLarge,
    /// Too many calls. Wait before sending the next one.
    RateLimited {
        /// How long to wait, when a response header said so.
        retry_after: Option<Duration>,
    },
    /// The service failed on its side. The same request can succeed later.
    ServerError,
    /// The service is shedding load. The same request can succeed later.
    Draining {
        /// How long to wait, when a response header said so.
        retry_after: Option<Duration>,
    },
    /// A status this version does not map.
    Other,
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
    fn maps_every_documented_code() {
        let cases = [
            (200u16, ApiClass::Ok),
            (204, ApiClass::NoContent),
            (400, ApiClass::BadRequest),
            (401, ApiClass::Unauthorized),
            (402, ApiClass::InsufficientCredits),
            (413, ApiClass::PayloadTooLarge),
            (429, ApiClass::RateLimited { retry_after: None }),
            (500, ApiClass::ServerError),
            (503, ApiClass::Draining { retry_after: None }),
        ];
        for (code, class) in cases {
            assert_eq!(ApiStatus::new(code).class(), class, "code {code}");
        }
        assert_eq!(ApiStatus::new(418).class(), ApiClass::Other);
    }

    #[test]
    fn retry_after_rides_along_when_a_header_supplied_it() {
        let wait = Some(Duration::from_secs(7));
        assert_eq!(
            ApiStatus::new(429).class_with_retry_after(wait),
            ApiClass::RateLimited { retry_after: wait }
        );
        assert_eq!(
            ApiStatus::new(503).class_with_retry_after(wait),
            ApiClass::Draining { retry_after: wait }
        );
    }

    #[test]
    fn display_names_the_call_plane() {
        assert_eq!(ApiStatus::new(403).to_string(), "api status 403");
    }

    #[test]
    fn code_round_trips() {
        assert_eq!(ApiStatus::new(204).code(), 204);
        assert!(ApiStatus::new(204).is_success());
        assert!(!ApiStatus::new(500).is_success());
    }
}
