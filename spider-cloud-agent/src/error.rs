//! What can go wrong, and how to tell the cases apart.

use crate::credits::Credits;
use crate::response::{Attempt, FailedPage};
use crate::status::ApiStatus;
use std::time::Duration;

/// An error from a call to Spider Cloud.
///
/// Running out of credits is its own variant rather than a status code, because
/// it is the one failure that must never be retried: every attempt after the
/// balance hits zero fails the same way, and some of them still cost money.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The service rejected the call itself. This is never a target site's
    /// status, only the status of your request to Spider Cloud.
    #[error("{status}{}", .message.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
    Api {
        /// The status the service returned.
        status: ApiStatus,
        /// What the service said, when it said anything.
        message: Option<String>,
        /// How long to wait before trying again, when the service said.
        retry_after: Option<Duration>,
    },

    /// The account has no credits left. Do not retry this.
    #[error("no credits left on the account")]
    InsufficientCredits,

    /// Every attempt was spent and none produced a usable page.
    #[error("no usable page after {} attempts", .attempts.len())]
    Exhausted {
        /// What was tried, in order.
        attempts: Vec<Attempt>,
        /// The last failure, when there was one to keep.
        last: Option<Box<FailedPage>>,
    },

    /// The call would have cost more than the budget allowed.
    #[error("budget exceeded: {kind}")]
    BudgetExceeded {
        /// Which cap stopped it.
        kind: BudgetKind,
        /// What was tried before the cap stopped it, in order. Empty when the
        /// cap was reached before anything was sent.
        attempts: Vec<Attempt>,
    },

    /// No usable API key, or the key was rejected.
    #[error("authentication: {0}")]
    Auth(String),

    /// The request never reached the service.
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),

    /// The response did not have the shape this version of the crate expects.
    #[error("could not read the response: {0}")]
    Decode(#[source] serde_json::Error),

    /// The client was built with settings that cannot work.
    #[error("configuration: {0}")]
    Config(String),
}

/// Which budget ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BudgetKind {
    /// The credit cap.
    Credits,
    /// The wall clock cap.
    Time,
    /// The attempt count.
    Attempts,
}

impl std::fmt::Display for BudgetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetKind::Credits => write!(f, "credits"),
            BudgetKind::Time => write!(f, "time"),
            BudgetKind::Attempts => write!(f, "attempts"),
        }
    }
}

impl Error {
    /// Whether trying the same call again could plausibly work.
    ///
    /// False for anything caused by the request itself, and false for running
    /// out of credits, which only gets worse with repetition. On the call plane it
    /// is a rate limit or a status [`ApiStatus::is_transient`] names, the same set
    /// the send loop retries.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Api { status, .. } => status.code() == 429 || status.is_transient(),
            Error::Transport(e) => e.is_timeout() || e.is_connect(),
            _ => false,
        }
    }

    /// The spend that has already happened, when the error knows about any.
    pub fn spent(&self) -> Credits {
        match self {
            Error::Exhausted { attempts, .. } | Error::BudgetExceeded { attempts, .. } => {
                attempts.iter().map(|a| a.cost).sum()
            }
            _ => Credits::ZERO,
        }
    }
}
