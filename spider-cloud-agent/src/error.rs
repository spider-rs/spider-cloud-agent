//! What can go wrong, and how to tell the cases apart.

use crate::credits::Credits;
use crate::policy::StopReason;
use crate::response::{Attempt, FailedPage};
use crate::status::ApiStatus;
use std::time::Duration;

/// An error from a call to Spider Cloud.
///
/// Running out of credits is its own variant rather than a status code, because
/// it is the one failure that must never be retried: every attempt after the
/// balance hits zero fails the same way, and some of them still cost money.
///
/// A rate limit is not its own variant either. A 429 arrives as [`Error::Api`]
/// with its status and its `retry_after`, and [`Error::recovery`] is what an
/// agent reads to learn that the answer is to wait.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A call error together with its operation trail and optional run totals.
    #[error("{source}")]
    Accounted {
        /// Original error, preserving its variant and recovery advice.
        source: Box<Error>,
        /// Every attempted call in this operation.
        attempts: Vec<Attempt>,
        /// Totals across the shared run, when configured.
        run: Option<crate::client::RunSpend>,
    },
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

    /// Pages came back, none of them usable, and the walk then stopped.
    ///
    /// A walk that never got a page back keeps the call error in [`Error::Accounted`],
    /// [`Error::Api`], [`Error::Auth`] or [`Error::Transport`], because what
    /// stopped it is a fact about the call rather than about a page.
    #[error(
        "no usable page after {} attempts: {reason}{}",
        .attempts.len(),
        .source.as_deref().map(|s| format!(", {s}")).unwrap_or_default()
    )]
    Exhausted {
        /// What was tried, in order.
        attempts: Vec<Attempt>,
        /// The last failure, when there was one to keep.
        last: Option<Box<FailedPage>>,
        /// Why the policy stopped the walk.
        reason: StopReason,
        /// The call error that stopped the walk, when a call rather than a
        /// page was what stopped it.
        source: Option<Box<Error>>,
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

    /// No usable API key, the key was rejected, or signing in did not produce
    /// one.
    #[error("authentication: {message}")]
    Auth {
        /// Which kind of trouble it was, so a caller can tell a key the service
        /// refused from a key that was never there.
        cause: AuthCause,
        /// What went wrong, naming places and never values.
        message: String,
    },

    /// Sending the request or reading its response failed at the HTTP layer.
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),

    /// The response did not have the shape this version of the crate expects.
    #[error("could not read the response: {0}")]
    Decode(#[source] serde_json::Error),

    /// The answer ran past the size the client was built to read.
    ///
    /// Only a client built with `max_response_bytes` can see this. Inside a
    /// page operation it arrives as the source of an [`Error::Exhausted`]
    /// whose attempts include the cut-off call, so nothing spent before it is
    /// lost.
    #[error("the answer ran past {limit} bytes")]
    ResponseTooLarge {
        /// The most the client was built to read.
        limit: usize,
        /// The status the answer arrived with. The body behind it was not read.
        status: ApiStatus,
    },

    /// The client was built with settings that cannot work.
    #[error("configuration: {0}")]
    Config(String),
}

/// Why authentication failed.
///
/// The split that matters is whether signing in again could help. It can when
/// the service refused a key or the sign in itself failed. It cannot create a
/// key that was never configured, or fix a path on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthCause {
    /// Nothing is configured: no environment variable, no keychain entry and
    /// no credentials file.
    NoKey,
    /// A key was supplied and it was blank.
    EmptyKey,
    /// The service answered 401.
    Refused,
    /// The sign in flow itself failed: the browser step, the token exchange or
    /// a bad state.
    SignInFailed,
    /// A problem on this machine: no home directory, a file that could not be
    /// written or narrowed, or a configured address that is not one.
    Local,
}

/// What a caller can do about an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Recovery {
    /// Send the same call again after this wait, or after a wait of the
    /// caller's choosing when the service named none.
    Wait(Option<Duration>),
    /// Sign in again, or supply a different key.
    Reauthenticate,
    /// Nothing about sending again will change the answer.
    Permanent,
    /// The error does not say.
    Unknown,
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
    /// Original error without the accounting wrapper.
    pub fn cause(&self) -> &Error {
        match self {
            Self::Accounted { source, .. } => source.cause(),
            _ => self,
        }
    }

    /// Consume the accounting wrapper after recording its trail.
    pub fn into_cause(self) -> Error {
        match self {
            Self::Accounted { source, .. } => source.into_cause(),
            _ => self,
        }
    }

    /// Run totals on an error from a client with shared credit admission.
    pub fn run_spend(&self) -> Option<crate::RunSpend> {
        match self {
            Self::Accounted { run, .. } => *run,
            _ => None,
        }
    }

    pub(crate) fn with_run(mut self, snapshot: crate::RunSpend) -> Self {
        if let Self::Accounted { run, .. } = &mut self {
            *run = Some(snapshot);
            self
        } else {
            let attempts = self.attempts().to_vec();
            self.accounted(attempts, Some(snapshot))
        }
    }

    /// Calls made before this error, including calls with unknown charges.
    pub fn attempts(&self) -> &[Attempt] {
        match self {
            Self::Accounted { attempts, .. }
            | Self::Exhausted { attempts, .. }
            | Self::BudgetExceeded { attempts, .. } => attempts,
            _ => &[],
        }
    }

    pub(crate) fn accounted(
        self,
        attempts: Vec<Attempt>,
        run: Option<crate::client::RunSpend>,
    ) -> Self {
        Self::Accounted {
            source: Box::new(self),
            attempts,
            run,
        }
    }

    /// Whether trying the same call again could plausibly work.
    ///
    /// False for anything caused by the request itself, and false for running
    /// out of credits, which only gets worse with repetition. On the call plane it
    /// is a rate limit or a status [`ApiStatus::is_transient`] names, the same set
    /// the send loop retries.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Accounted { source, .. } => source.is_retryable(),
            Error::Api { status, .. } => status.code() == 429 || status.is_transient(),
            Error::Transport(e) => e.is_timeout() || e.is_connect(),
            _ => false,
        }
    }

    /// What to do about this error.
    ///
    /// A rate limit and a transient service failure say to wait, with the
    /// service's own figure when it sent one. A refused key or a failed sign in
    /// say to sign in again. A key that was never there, a local path problem,
    /// an empty balance, a cap and a malformed request or response are
    /// permanent, because sending again does not create a key, fix a path or
    /// refill an account. An exhausted walk answers from the call error that
    /// stopped it, when there was one, and from the policy's reason otherwise.
    pub fn recovery(&self) -> Recovery {
        match self {
            Error::Accounted { source, .. } => source.recovery(),
            Error::Api {
                status,
                retry_after,
                ..
            } => {
                if status.code() == 429 || status.is_transient() {
                    Recovery::Wait(*retry_after)
                } else {
                    Recovery::Permanent
                }
            }
            Error::Transport(e) if e.is_timeout() || e.is_connect() => Recovery::Wait(None),
            Error::Transport(_) => Recovery::Permanent,
            Error::Auth { cause, .. } => match cause {
                AuthCause::Refused | AuthCause::SignInFailed => Recovery::Reauthenticate,
                AuthCause::NoKey | AuthCause::EmptyKey | AuthCause::Local => Recovery::Permanent,
            },
            Error::InsufficientCredits | Error::BudgetExceeded { .. } => Recovery::Permanent,
            Error::Exhausted { reason, source, .. } => match source.as_deref() {
                Some(Error::Api {
                    status,
                    retry_after,
                    ..
                }) if status.code() == 429 || status.is_transient() => Recovery::Wait(*retry_after),
                _ if matches!(reason, StopReason::Rejected { .. }) => Recovery::Permanent,
                _ => Recovery::Unknown,
            },
            Error::Decode(_) | Error::ResponseTooLarge { .. } | Error::Config(_) => {
                Recovery::Permanent
            }
        }
    }

    /// The spend that has already happened, when the error knows about any.
    pub fn spent(&self) -> Credits {
        match self {
            Error::Accounted { attempts, .. }
            | Error::Exhausted { attempts, .. }
            | Error::BudgetExceeded { attempts, .. } => attempts.iter().map(|a| a.cost).sum(),
            _ => Credits::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::response::Hint;

    fn api(code: u16, retry_after: Option<Duration>) -> Error {
        Error::Api {
            status: ApiStatus::new(code),
            message: None,
            retry_after,
        }
    }

    fn auth(cause: AuthCause) -> Error {
        Error::Auth {
            cause,
            message: "a message naming no value".to_string(),
        }
    }

    fn exhausted(reason: StopReason, source: Option<Error>) -> Error {
        Error::Exhausted {
            attempts: Vec::new(),
            last: None,
            reason,
            source: source.map(Box::new),
        }
    }

    /// Every stop reason there is, so a test over them cannot quietly miss one
    /// that a later change adds.
    const REASONS: [StopReason; 6] = [
        StopReason::Rejected {
            hint: Hint::Permanent,
        },
        StopReason::OutOfCredits,
        StopReason::Budget(BudgetKind::Credits),
        StopReason::RetriesExhausted,
        StopReason::LadderExhausted,
        StopReason::Unhandled,
    ];

    /// A real timeout: a listener that accepts and never answers, against a
    /// client that gives up first.
    async fn timed_out() -> reqwest::Error {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 1), 0))
            .expect("a port");
        let address = listener.local_addr().expect("an address");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("a client");
        let error = client
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect_err("a timeout");
        drop(listener);
        error
    }

    /// A real refused connection: a port that was open a moment ago and is not
    /// any more.
    async fn refused_connection() -> reqwest::Error {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 1), 0))
            .expect("a port");
        let address = listener.local_addr().expect("an address");
        drop(listener);
        reqwest::get(format!("http://{address}/"))
            .await
            .expect_err("a refused connection")
    }

    #[test]
    fn a_rate_limit_says_to_wait_for_what_the_service_asked() {
        let wait = Some(Duration::from_secs(7));
        assert_eq!(api(429, wait).recovery(), Recovery::Wait(wait));
        assert_eq!(api(429, None).recovery(), Recovery::Wait(None));
    }

    #[test]
    fn a_transient_service_failure_says_to_wait() {
        for code in [408, 500, 502, 503, 504] {
            let wait = Some(Duration::from_secs(2));
            assert_eq!(api(code, wait).recovery(), Recovery::Wait(wait), "{code}");
        }
    }

    #[test]
    fn a_call_the_service_refused_for_good_is_permanent() {
        for code in [400, 403, 404, 413, 422] {
            assert_eq!(api(code, None).recovery(), Recovery::Permanent, "{code}");
        }
    }

    #[tokio::test]
    async fn a_timeout_on_the_wire_says_to_wait_with_no_figure() {
        let error = Error::Transport(timed_out().await);
        assert!(error.is_retryable(), "{error}");
        assert_eq!(error.recovery(), Recovery::Wait(None));
    }

    #[tokio::test]
    async fn a_refused_connection_says_to_wait_with_no_figure() {
        let error = Error::Transport(refused_connection().await);
        assert!(error.is_retryable(), "{error}");
        assert_eq!(error.recovery(), Recovery::Wait(None));
    }

    #[test]
    fn any_other_transport_failure_is_permanent() {
        let error = reqwest::Client::new()
            .get("not a url")
            .build()
            .expect_err("a request that cannot be built");
        assert!(!error.is_timeout() && !error.is_connect());
        assert_eq!(Error::Transport(error).recovery(), Recovery::Permanent);
    }

    #[test]
    fn a_refused_key_or_a_failed_sign_in_says_to_sign_in_again() {
        assert_eq!(
            auth(AuthCause::Refused).recovery(),
            Recovery::Reauthenticate
        );
        assert_eq!(
            auth(AuthCause::SignInFailed).recovery(),
            Recovery::Reauthenticate
        );
    }

    #[test]
    fn a_key_that_was_never_there_or_a_local_problem_is_permanent() {
        for cause in [AuthCause::NoKey, AuthCause::EmptyKey, AuthCause::Local] {
            assert_eq!(auth(cause).recovery(), Recovery::Permanent, "{cause:?}");
        }
    }

    #[test]
    fn an_auth_error_still_reads_as_one() {
        let shown = auth(AuthCause::Refused).to_string();
        assert!(shown.starts_with("authentication: "), "{shown}");
    }

    #[test]
    fn an_empty_balance_is_permanent() {
        assert_eq!(Error::InsufficientCredits.recovery(), Recovery::Permanent);
    }

    #[test]
    fn a_cap_is_permanent() {
        for kind in [BudgetKind::Credits, BudgetKind::Time, BudgetKind::Attempts] {
            let error = Error::BudgetExceeded {
                kind,
                attempts: Vec::new(),
            };
            assert_eq!(error.recovery(), Recovery::Permanent, "{kind}");
        }
    }

    #[test]
    fn a_walk_a_rate_limit_stopped_says_to_wait_for_the_figure_it_carried() {
        let wait = Some(Duration::from_secs(9));
        for reason in REASONS {
            let error = exhausted(reason, Some(api(429, wait)));
            assert_eq!(error.recovery(), Recovery::Wait(wait), "{reason:?}");
        }
    }

    #[test]
    fn a_walk_a_transient_failure_stopped_says_to_wait() {
        for code in [408, 500, 502, 503, 504] {
            let error = exhausted(StopReason::RetriesExhausted, Some(api(code, None)));
            assert_eq!(error.recovery(), Recovery::Wait(None), "{code}");
        }
    }

    #[test]
    fn a_walk_the_site_settled_is_permanent_whatever_stopped_the_last_call() {
        let rejected = StopReason::Rejected {
            hint: Hint::TryBrowser,
        };
        assert_eq!(exhausted(rejected, None).recovery(), Recovery::Permanent);
        let error = exhausted(rejected, Some(api(403, None)));
        assert_eq!(error.recovery(), Recovery::Permanent);
    }

    #[test]
    fn every_other_exhausted_walk_does_not_say() {
        for reason in REASONS {
            if matches!(reason, StopReason::Rejected { .. }) {
                continue;
            }
            assert_eq!(
                exhausted(reason, None).recovery(),
                Recovery::Unknown,
                "{reason:?}"
            );
            let error = exhausted(reason, Some(api(403, None)));
            assert_eq!(error.recovery(), Recovery::Unknown, "{reason:?}");
        }
    }

    #[tokio::test]
    async fn a_walk_a_timeout_stopped_does_not_say() {
        let error = exhausted(
            StopReason::RetriesExhausted,
            Some(Error::Transport(timed_out().await)),
        );
        assert_eq!(error.recovery(), Recovery::Unknown);
    }

    #[test]
    fn a_body_that_cannot_be_read_is_permanent() {
        let error =
            serde_json::from_str::<serde_json::Value>("not json").expect_err("a decode error");
        assert_eq!(Error::Decode(error).recovery(), Recovery::Permanent);
    }

    #[test]
    fn a_configuration_that_cannot_work_is_permanent() {
        let error = Error::Config("an address that is not one".to_string());
        assert_eq!(error.recovery(), Recovery::Permanent);
    }

    #[test]
    fn an_exhausted_walk_says_why_and_what_stopped_it() {
        let alone = exhausted(StopReason::LadderExhausted, None).to_string();
        assert_eq!(
            alone,
            "no usable page after 0 attempts: out of escalation steps"
        );

        let stopped = exhausted(
            StopReason::RetriesExhausted,
            Some(Error::Api {
                status: ApiStatus::new(429),
                message: Some("slow down".to_string()),
                retry_after: None,
            }),
        );
        assert_eq!(
            stopped.to_string(),
            "no usable page after 0 attempts: out of retries, api status 429: slow down"
        );
        let source = std::error::Error::source(&stopped).expect("the call error");
        assert_eq!(source.to_string(), "api status 429: slow down");
    }
}
