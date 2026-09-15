//! Paying for less of what comes back.
//!
//! Two tiers, and the first one is where nearly all of the saving is.
//!
//! The first tier picks request parameters from what the caller said they
//! need, so the bytes never leave the service. It is mostly not clever. The
//! API returns everything unless it is told otherwise, and this crate returns
//! nothing that was not asked for. [`Need::Fields`] is the extreme case: the
//! response carries the named extractions and none of the page.
//!
//! The second tier trims what still arrives. Repeated navigation, inlined
//! images, blank runs, and a ceiling on how much of a page reaches a context
//! window. See [`trim`].
//!
//! ```
//! use spider_cloud_agent::Need;
//! use spider_cloud_agent::thrift::Plan;
//!
//! // Three fields off a product page, and no page bytes at all.
//! let plan = Plan::for_need(&Need::fields([("price", ".price"), ("title", "h1")]));
//! assert!(plan.returns_no_page());
//! ```

pub mod need;
pub mod plan;
pub mod report;
pub mod tokens;
pub mod trim;

pub use need::{FieldSpec, Need, ShotSpec, ROOT_PATH};
pub use plan::{Endpoint, Plan, OPTIONAL_RETURNS};
pub use report::ThriftReport;
pub use tokens::{approx_tokens, max_tokens, TokenBudget};
pub use trim::{TrimReason, TrimSettings, Trimmed, Trimmer};
