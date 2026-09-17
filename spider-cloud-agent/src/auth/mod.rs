//! Where the API key comes from, and how to get one.
//!
//! Three pieces. [`store`] finds a key that is already on the machine and writes
//! one back, sharing both places with the command line tools so signing in with
//! either tool signs you in for all of them. [`router`] keeps a provider
//! fallback beside it, with the provider keys in it. [`oauth`], behind the
//! `oauth` feature, opens a browser and comes back with a freshly provisioned
//! key.
//!
//! Nothing here prints a key. Every type that holds one writes its own `Debug`
//! and puts a placeholder where the value would be, and the tests assert that
//! rather than trusting it.

pub mod router;
pub mod store;

#[cfg(feature = "oauth")]
#[cfg_attr(docsrs, doc(cfg(feature = "oauth")))]
pub mod oauth;

pub use store::{
    Credentials, Source, Stored, API_KEY_ENV, API_KEY_ENV_ALT, CREDENTIALS_PATH, KEYRING_SERVICE,
    KEYRING_USER,
};

#[cfg(feature = "oauth")]
#[cfg_attr(docsrs, doc(cfg(feature = "oauth")))]
pub use oauth::{login, MCP_SERVER_ENV};
