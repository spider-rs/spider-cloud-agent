//! How this crate reads and writes a request.
//!
//! The request body is `RequestParams` in the client crate, and the client
//! depends on this crate, so this crate cannot name that type. It asks for
//! the handful of reads and writes an edit needs instead, and the client
//! implements them for its own type. Every rule about what may be written,
//! the caller's precedence above all, lives in [`crate::EditSet::apply`] and
//! [`crate::validate()`], not in an implementation of this trait.
//!
//! An implementation is plain field access. It must not allocate on the read
//! methods, because candidates are validated and featurized on every request.

use crate::schema::Key;
use spider_route::{ProxyPool, RequestMode};

/// Field access on a request body, keyed by [`Key`].
///
/// Object safe, so a [`crate::Context`] can hold the request as it is about to
/// go out and the caller's own snapshot side by side without a type parameter.
pub trait Params {
    /// Whether the field this key names holds a value. For
    /// [`Key::WaitIdleMillis`] that is whether `wait_for` holds anything.
    fn is_set(&self, key: Key) -> bool;

    /// The fetch mode, when one is set.
    fn request(&self) -> Option<RequestMode>;

    /// The proxy pool, when one is set.
    fn proxy(&self) -> Option<ProxyPool>;

    /// The idle network timeout in milliseconds, when `wait_for` holds one.
    /// Any other kind of wait answers `None`.
    fn idle_wait_millis(&self) -> Option<u32>;

    /// The value of a field whose [`crate::Kind`] is `Bool`, when it is set.
    /// Any other key answers `None`.
    fn flag(&self, key: Key) -> Option<bool>;

    /// The entries of a field whose [`crate::Kind`] is `StringList`, when it
    /// is set. Any other key answers `None`.
    fn list(&self, key: Key) -> Option<&[String]>;

    /// Set the fetch mode.
    fn set_request(&mut self, mode: RequestMode);

    /// Set the proxy pool.
    fn set_proxy(&mut self, pool: ProxyPool);

    /// Replace `wait_for` with an idle network wait of this many milliseconds.
    fn set_idle_wait(&mut self, millis: u32);

    /// Set a `Bool` field. Only learnable keys are ever passed, and any other
    /// key may be ignored.
    fn set_flag(&mut self, key: Key, value: bool);

    /// Replace a `StringList` field. Only [`Key::NetworkBlacklist`] is ever
    /// passed, and any other key may be ignored.
    fn set_list(&mut self, key: Key, list: Option<Vec<String>>);
}
