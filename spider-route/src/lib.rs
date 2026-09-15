//! Picks the cheapest Spider Cloud request settings that are likely to work.
//!
//! This crate makes one decision and makes it locally: given the shape of a
//! request, which settings should the first attempt use. It does no network
//! work, runs no async runtime, and holds no API client, so depending on it
//! costs a consumer nothing but the vocabulary.
//!
//! A model is optional. With no weights compiled in, [`HeuristicRouter`]
//! answers from rules alone, and that path is tested on its own so it cannot
//! rot.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod action;
pub mod decision;
pub mod domain;
pub mod features;
pub mod heuristic;
pub mod router;

pub use action::{Country, InvalidCountry, ProxyPool, RequestMode, PROXY_POOLS};
pub use decision::{Action, RouteDecision, RouteSource, Wait};
pub use domain::{host_shape, registrable_domain, HostShape, IpKind, TldSlot};
pub use features::{
    featurize, CallerPins, DeclaredNeed, ExtClass, FeatureVector, KeyClass, RouteInput,
    SegmentClass, SiteMemory, StatusClass, FEATURES_USED, FEATURE_DIM,
};
pub use heuristic::{HeuristicRouter, Rule};
pub use router::{AttemptOutcome, Router, RouterVersion};
