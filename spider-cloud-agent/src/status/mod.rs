//! The two status planes, kept apart on purpose.
//!
//! Every call to the API carries two numbers that look alike and mean nothing like
//! each other:
//!
//! - [`ApiStatus`] is the HTTP status of your call to spider.cloud. A 403 here means
//!   the key is rejected.
//! - [`PageStatus`] is what the target site returned. It arrives inside the response
//!   body. A 403 here means the site refused the fetch, and a heavier request usually
//!   gets through.
//!
//! Reading one as the other is the easiest bug to write against this API, so the two
//! are separate types in separate modules. Neither converts into the other, neither has
//! a public `From<u16>`, and only the transport can mint one. Their [`std::fmt::Display`]
//! output carries different prefixes so a log line says which plane it came from.
//!
//! The call plane never reaches a success value. A [`crate::response::Page`] exists only
//! for a 2xx target status, so a page you are holding has already passed both planes.
//!
//! ```compile_fail
//! # use spider_cloud_agent::status::{ApiStatus, PageStatus};
//! fn takes_page(_: PageStatus) {}
//! fn give(api: ApiStatus) {
//!     takes_page(api); // the two planes are different types
//! }
//! ```
//!
//! ```compile_fail
//! # use spider_cloud_agent::status::PageStatus;
//! // Only the transport mints a status.
//! let _ = PageStatus::from(403u16);
//! ```

pub mod api;
pub mod page;

pub use api::{ApiClass, ApiStatus};
pub use page::{PageClass, PageStatus};
