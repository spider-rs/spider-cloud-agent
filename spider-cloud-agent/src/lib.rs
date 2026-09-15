//! Agentic client for the Spider Cloud API.
//!
//! Three things this adds over a plain API binding:
//!
//! - it picks the cheapest request settings likely to work, before spending a call
//! - it reads both status planes and escalates on its own when a fetch is blocked
//! - it returns only what the caller asked for, so an LLM reading the result pays for less
//!
//! The two status planes are the thing to understand first. A call to Spider
//! Cloud has an HTTP status, and the page it fetched has its own status inside
//! the body. A 403 in the first means your key is wrong. A 403 in the second
//! means the site blocked the fetch. See [`status`].

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
pub mod client;
pub mod credits;
pub mod error;
pub mod memory;
pub mod ops;
pub mod params;
pub mod policy;
pub mod record;
pub mod response;
pub mod routing;
pub mod status;
pub mod thrift;
mod transport;

pub use client::{Spider, SpiderBuilder, Transport};
pub use credits::{Credits, Usd, WholeCredits, CREDITS_PER_USD};
pub use error::{BudgetKind, Error};
pub use memory::SiteMemoryStore;
pub use params::{
    Country, ProxyPool, RequestMode, RequestParams, ReturnFormat, SearchParams, WaitFor,
};
pub use policy::{Budget, Ladder, Policy, Rung, Step};
pub use record::{JsonlRecorder, Recorder};
pub use response::{Attempt, Body, FailedPage, Outcome, Page, PageResult, Pages};
pub use routing::Explorer;
pub use spider_route::{
    featurize, Action, AttemptOutcome, CallerPins, DeclaredNeed, HeuristicRouter, RouteDecision,
    RouteInput, RouteSource, Router, RouterVersion, SiteMemory, StatusClass, Wait,
};
pub use status::{ApiClass, ApiStatus, PageClass, PageStatus};
pub use thrift::{Need, ThriftReport, TokenBudget};

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;
