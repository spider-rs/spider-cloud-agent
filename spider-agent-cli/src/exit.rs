//! What the process says when it leaves.
//!
//! A caller branches on the code rather than on the words. Every failure in
//! this tool lands on one of these, and the mapping is the contract: a new
//! failure shape reuses an existing code or adds one here, and nothing else
//! decides it.

use std::fmt;
use std::process::ExitCode;

use spider_cloud_agent::{AuthCause, Error};

// Declare the variants and their enumeration together so a new variant cannot
// compile without also appearing in the schema and its coverage tests.
macro_rules! codes {
    ($( $(#[$meta:meta])* $variant:ident, )*) => {
        /// Why the process stopped.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Code {
            $( $(#[$meta])* $variant, )*
        }

        impl Code {
            /// Every process outcome, used to enumerate the schema.
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];
        }
    };
}

codes! {
    /// The work finished.
    Ok,
    /// Something failed that none of the other codes describes.
    Failed,
    /// The command line was wrong, or an input file could not be read.
    Usage,
    /// No key, a key the service refused, or a balance of zero.
    Auth,
    /// A cap this run was given stopped it.
    Budget,
    /// The target site never served the page.
    Refused,
    /// The call never reached the service, or the service failed it.
    Transport,
    /// A destination could not be written, or writing it would have clobbered
    /// something.
    Output,
}

impl Code {
    /// The number the shell sees.
    pub const fn number(self) -> u8 {
        match self {
            Code::Ok => 0,
            Code::Failed => 1,
            Code::Usage => 2,
            Code::Auth => 3,
            Code::Budget => 4,
            Code::Refused => 5,
            Code::Transport => 6,
            Code::Output => 7,
        }
    }

    /// The name used in the `stop` field of a report record and in `schema`.
    pub const fn label(self) -> &'static str {
        match self {
            Code::Ok => "ok",
            Code::Failed => "failed",
            Code::Usage => "usage",
            Code::Auth => "auth",
            Code::Budget => "budget",
            Code::Refused => "refused",
            Code::Transport => "transport",
            Code::Output => "output",
        }
    }
}

impl From<Code> for ExitCode {
    fn from(code: Code) -> ExitCode {
        ExitCode::from(code.number())
    }
}

/// A failure with the code it exits under already decided.
#[derive(Debug)]
pub struct Failure {
    /// The code the process will return.
    pub code: Code,
    /// What to print on stderr, in one line.
    pub message: String,
}

impl Failure {
    /// A failure under a code you have already chosen.
    pub fn new(code: Code, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
        }
    }

    /// The command line was wrong, or an input could not be read.
    pub fn usage(message: impl Into<String>) -> Failure {
        Failure::new(Code::Usage, message)
    }

    /// A destination could not be written.
    pub fn output(message: impl Into<String>) -> Failure {
        Failure::new(Code::Output, message)
    }

    /// The reader of the payload has gone.
    ///
    /// `spider-agent ... | head -1` closes the pipe after one line, and that
    /// is the reader's decision rather than a failure of the run. The run
    /// stops where it is, so no page after that is paid for, and the process
    /// leaves with code 0 and nothing on stderr, the way cat and grep leave.
    /// The message is empty, and an empty message is what tells the caller
    /// of a failed run to print nothing.
    pub fn reader_gone() -> Failure {
        Failure::new(Code::Ok, String::new())
    }

    /// Whether there is anything to print. The one silent failure is the
    /// reader going away.
    pub fn is_silent(&self) -> bool {
        self.message.is_empty()
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Failure {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            return Failure::reader_gone();
        }
        Failure::output(error.to_string())
    }
}

/// Sort a library error into a code.
///
/// The two status planes decide most of this. A refusal by the service is an
/// auth or a transport failure, and a refusal by the site is [`Code::Refused`],
/// which a caller can retry from another address rather than treating as a dead
/// key.
impl From<Error> for Failure {
    fn from(error: Error) -> Failure {
        let code = match &error {
            Error::Auth { .. } => Code::Auth,
            Error::InsufficientCredits => Code::Auth,
            Error::BudgetExceeded { .. } => Code::Budget,
            // Pages came back before the walk stopped, whatever stopped it: a
            // site that refused every attempt, a rate limit that ran out after
            // a page, or a call that failed after one. The run reached the
            // site, so the code says so.
            Error::Exhausted { .. } => Code::Refused,
            Error::Config(_) => Code::Usage,
            Error::Transport(_) | Error::Decode(_) => Code::Transport,
            Error::Api { status, .. } => match status.code() {
                401..=403 => Code::Auth,
                400 | 404 | 422 => Code::Usage,
                _ => Code::Transport,
            },
            // The error list grows with the API. An unsorted failure is still
            // a failure, and calling it something specific would be a guess.
            _ => Code::Failed,
        };
        let message = match &error {
            Error::Auth { cause, .. } => match next_step(*cause) {
                Some(step) => format!("{error}. {step}"),
                None => error.to_string(),
            },
            _ => error.to_string(),
        };
        Failure::new(code, message)
    }
}

/// The one thing to do about a key that did not work, so the stderr line
/// names the next step and not only what went wrong.
///
/// A missing key already says where it looked, so it gets nothing added.
fn next_step(cause: AuthCause) -> Option<&'static str> {
    match cause {
        AuthCause::NoKey => None,
        AuthCause::EmptyKey => Some("The key that was found is blank, so set a real one"),
        AuthCause::Refused => Some(
            "The service refused the key. Sign in again with spider-agent login, or set a different key",
        ),
        AuthCause::SignInFailed => Some("Run spider-agent login again"),
        AuthCause::Local => Some(
            "Signing in again will not fix this. Check the home directory and the credentials file",
        ),
        // The causes grow with the crate, and a cause this build does not know
        // still lands on the auth code with the crate's own message.
        _ => None,
    }
}

/// The result every command returns.
pub type Run<T> = std::result::Result<T, Failure>;

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn every_code_has_its_own_number() {
        let codes = Code::ALL;
        let mut numbers: Vec<u8> = codes.iter().map(|c| c.number()).collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), codes.len(), "two codes share a number");
    }

    use spider_cloud_agent::policy::{Observed, Reached, StopReason};
    use spider_cloud_agent::response::Hint;
    use spider_cloud_agent::{ApiStatus, BudgetKind};
    use std::time::Duration;

    /// A status the way the crate mints one, through the documented replay
    /// hole, since nothing outside the crate can build one from a number.
    fn api_status(code: u16) -> ApiStatus {
        match Observed::seen(code, None).api {
            Reached::Api(status) => status,
            other => panic!("no status for {code}: {other:?}"),
        }
    }

    fn api(code: u16) -> Error {
        Error::Api {
            status: api_status(code),
            message: None,
            retry_after: Some(Duration::from_secs(1)),
        }
    }

    fn auth(cause: AuthCause) -> Error {
        Error::Auth {
            cause,
            message: "the key did not work".to_string(),
        }
    }

    /// A real transport error: a port that was open a moment ago and is not
    /// any more, reached through the crate the way the tool reaches it.
    fn transport_error() -> Error {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 1), 0))
            .expect("a port");
        let address = listener.local_addr().expect("an address");
        drop(listener);
        let spider = spider_cloud_agent::Spider::builder()
            .key("not-a-real-key")
            .base_url(url::Url::parse(&format!("http://{address}")).expect("a url"))
            .build()
            .expect("a client");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(spider.credits())
            .expect_err("a refused connection")
    }

    fn exhausted(reason: StopReason, source: Option<Error>) -> Error {
        Error::Exhausted {
            attempts: Vec::new(),
            last: None,
            reason,
            source: source.map(Box::new),
        }
    }

    /// Every reason a walk can stop with.
    fn every_reason() -> Vec<StopReason> {
        vec![
            StopReason::Rejected {
                hint: Hint::Permanent,
            },
            StopReason::OutOfCredits,
            StopReason::Budget(BudgetKind::Attempts),
            StopReason::RetriesExhausted,
            StopReason::LadderExhausted,
            StopReason::Unhandled,
        ]
    }

    /// Every kind of call error that can stop a walk after a page came back,
    /// and no error at all.
    fn every_source() -> Vec<(&'static str, Option<Error>)> {
        let transport = transport_error();
        assert!(matches!(transport, Error::Transport(_)), "{transport:?}");
        vec![
            ("none", None),
            ("rate limit", Some(api(429))),
            ("transient", Some(api(502))),
            ("refused call", Some(api(403))),
            ("transport", Some(transport)),
            ("auth", Some(auth(AuthCause::Refused))),
            ("credits", Some(Error::InsufficientCredits)),
            (
                "budget",
                Some(Error::BudgetExceeded {
                    kind: BudgetKind::Credits,
                    attempts: Vec::new(),
                }),
            ),
            (
                "decode",
                Some(Error::Decode(
                    serde_json::from_str::<serde_json::Value>("not json").expect_err("an error"),
                )),
            ),
            (
                "config",
                Some(Error::Config("an address that is not one".into())),
            ),
        ]
    }

    #[test]
    fn a_site_refusal_is_not_an_auth_failure() {
        let refused: Failure = exhausted(
            StopReason::Rejected {
                hint: Hint::Permanent,
            },
            None,
        )
        .into();
        assert_eq!(refused.code, Code::Refused);

        let auth: Failure = auth(AuthCause::NoKey).into();
        assert_eq!(auth.code, Code::Auth);
    }

    /// The table does not move. A walk that got a page back and then stopped
    /// leaves with the refused code whatever the reason and whatever call
    /// error stopped it: a refused site, a rate limit that ran out after a
    /// page, a transport failure after a page, all five.
    #[test]
    fn every_exhausted_walk_leaves_with_the_refused_code() {
        let mut checked = 0;
        for reason in every_reason() {
            for (kind, source) in every_source() {
                let failure: Failure = exhausted(reason, source).into();
                assert_eq!(failure.code, Code::Refused, "{reason:?} with {kind}");
                assert_eq!(failure.code.number(), 5);
                assert!(
                    failure.message.contains("no usable page"),
                    "{}",
                    failure.message
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 6 * 10, "a reason or a source kind went missing");
    }

    /// The bare call errors keep their codes too, so the same 429 lands on the
    /// transport code when no page ever came back and on the refused code
    /// when one did.
    #[test]
    fn a_bare_call_error_keeps_its_own_code() {
        let rate_limited: Failure = api(429).into();
        assert_eq!(rate_limited.code, Code::Transport);
        let refused_call: Failure = api(403).into();
        assert_eq!(refused_call.code, Code::Auth);
        let bad_request: Failure = api(400).into();
        assert_eq!(bad_request.code, Code::Usage);
    }

    /// Every cause lands on the one auth code, and the line says what to do.
    #[test]
    fn every_auth_cause_leaves_with_three_and_says_the_next_step() {
        for cause in [
            AuthCause::NoKey,
            AuthCause::EmptyKey,
            AuthCause::Refused,
            AuthCause::SignInFailed,
            AuthCause::Local,
        ] {
            let failure: Failure = auth(cause).into();
            assert_eq!(failure.code, Code::Auth, "{cause:?}");
            assert_eq!(failure.code.number(), 3);
            assert!(
                failure.message.starts_with("authentication: "),
                "{cause:?}: {}",
                failure.message
            );
            if let Some(step) = next_step(cause) {
                assert!(
                    failure.message.ends_with(step),
                    "{cause:?}: {}",
                    failure.message
                );
            }
        }
        let refused: Failure = auth(AuthCause::Refused).into();
        assert!(refused.message.contains("login"), "{}", refused.message);
    }

    #[test]
    fn a_closed_pipe_is_a_silent_stop_and_not_an_output_failure() {
        let gone: Failure = std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
        assert_eq!(gone.code, Code::Ok);
        assert!(gone.is_silent());

        let refused: Failure = std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
        assert_eq!(refused.code, Code::Output);
        assert!(!refused.is_silent());
    }

    #[test]
    fn a_budget_stop_has_its_own_code() {
        let stopped: Failure = Error::BudgetExceeded {
            kind: spider_cloud_agent::BudgetKind::Credits,
            attempts: Vec::new(),
        }
        .into();
        assert_eq!(stopped.code, Code::Budget);
        assert_eq!(stopped.code.number(), 4);
    }
}
