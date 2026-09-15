//! The vocabulary a routing decision is made in.
//!
//! These types live here rather than in the client because the client depends
//! on this crate and not the other way round. Keeping one definition means a
//! new routing action cannot exist on one side of the boundary and not the
//! other. See `docs/action-vocabulary.md`.

use serde::de::{Error as DeError, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// How the page is fetched.
///
/// The three values are the whole of the choice. Which software performs a
/// rendered fetch is decided by the service and is not selectable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum RequestMode {
    /// Plain HTTP fetch. Cheapest and fastest, and wrong for pages that build
    /// their content after load.
    Http,
    /// Start with HTTP and switch to a rendered fetch when the first response
    /// looks like it needs one. The default, and the right answer when you do
    /// not know the page.
    #[default]
    Smart,
    /// Always render the page before reading it. Costs more per page and takes
    /// longer, so ask for it when you already know HTTP comes back empty.
    Browser,
}

impl RequestMode {
    /// The value sent on the wire.
    pub const fn as_str(&self) -> &'static str {
        match self {
            RequestMode::Http => "http",
            RequestMode::Smart => "smart",
            RequestMode::Browser => "browser",
        }
    }

    /// Read a mode from an API value. Older spellings still in circulation are
    /// accepted, and matching ignores case.
    pub fn from_wire(value: &str) -> Option<RequestMode> {
        match value.trim().to_ascii_lowercase().as_str() {
            "http" => Some(RequestMode::Http),
            "smart" | "smart_mode" | "smartmode" => Some(RequestMode::Smart),
            "browser" | "chrome" | "headless" => Some(RequestMode::Browser),
            _ => None,
        }
    }
}

impl fmt::Display for RequestMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for RequestMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RequestMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<RequestMode, D::Error> {
        struct ModeVisitor;

        impl<'de> Visitor<'de> for ModeVisitor {
            type Value = RequestMode;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("one of \"http\", \"smart\" or \"browser\"")
            }

            fn visit_str<E: DeError>(self, value: &str) -> Result<RequestMode, E> {
                RequestMode::from_wire(value)
                    .ok_or_else(|| E::invalid_value(Unexpected::Str(value), &self))
            }
        }

        deserializer.deserialize_str(ModeVisitor)
    }
}

/// Which pool the outbound address comes from.
///
/// Two pools are sold and two appear here. A request that keeps failing on
/// `Isp` is the case for `Residential`, and a request that succeeds on `Isp`
/// should stay there, because residential addresses cost more per page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ProxyPool {
    /// Addresses held by internet service providers. Fast, stable, and the
    /// default. `datacenter` is the older name for the same pool.
    #[serde(rename = "isp", alias = "datacenter", alias = "ISP")]
    #[default]
    Isp,
    /// Addresses assigned to home connections. Slower and dearer per page, and
    /// what gets through when a site turns away the ISP pool.
    #[serde(rename = "residential", alias = "RESIDENTIAL")]
    Residential,
}

impl ProxyPool {
    /// The value sent on the wire.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ProxyPool::Isp => "isp",
            ProxyPool::Residential => "residential",
        }
    }
}

impl fmt::Display for ProxyPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every pool a request may be sent through.
pub const PROXY_POOLS: [ProxyPool; 2] = [ProxyPool::Isp, ProxyPool::Residential];

/// A two letter ISO 3166-1 country code, checked when it is built.
///
/// Pinning a country changes which addresses the request leaves from, which
/// changes what the site serves: prices, stock, language, and sometimes
/// whether the fetch is allowed at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Country(String);

impl Country {
    /// Build a country from a code such as `us` or `DE`. Returns `None` unless
    /// the input is exactly two ASCII letters. The stored form is lowercase,
    /// which is what the API expects.
    pub fn new(code: &str) -> Option<Country> {
        let code = code.trim();

        if code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()) {
            Some(Country(code.to_ascii_lowercase()))
        } else {
            None
        }
    }

    /// The lowercase code, ready to send.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Country {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Country {
    type Err = InvalidCountry;

    fn from_str(value: &str) -> Result<Country, InvalidCountry> {
        Country::new(value).ok_or(InvalidCountry)
    }
}

impl TryFrom<&str> for Country {
    type Error = InvalidCountry;

    fn try_from(value: &str) -> Result<Country, InvalidCountry> {
        Country::new(value).ok_or(InvalidCountry)
    }
}

/// Returned when a string is not a two letter country code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCountry;

impl fmt::Display for InvalidCountry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a country code is two ASCII letters, such as us or de")
    }
}

impl std::error::Error for InvalidCountry {}

impl Serialize for Country {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Country {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Country, D::Error> {
        struct CountryVisitor;

        impl<'de> Visitor<'de> for CountryVisitor {
            type Value = Country;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a two letter ISO 3166-1 country code")
            }

            fn visit_str<E: DeError>(self, value: &str) -> Result<Country, E> {
                Country::new(value).ok_or_else(|| E::invalid_value(Unexpected::Str(value), &self))
            }
        }

        deserializer.deserialize_str(CountryVisitor)
    }
}
