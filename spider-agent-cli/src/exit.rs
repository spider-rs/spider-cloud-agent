//! What the process says when it leaves.
//!
//! A caller branches on the code rather than on the words. Every failure in
//! this tool lands on one of these, and the mapping is the contract: a new
//! failure shape reuses an existing code or adds one here, and nothing else
//! decides it.

use std::fmt;
use std::process::ExitCode;

use spider_cloud_agent::Error;

/// Why the process stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
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
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Failure {
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
        let message = error.to_string();
        let code = match &error {
            Error::Auth(_) => Code::Auth,
            Error::InsufficientCredits => Code::Auth,
            Error::BudgetExceeded { .. } => Code::Budget,
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
        Failure::new(code, message)
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
        let codes = [
            Code::Ok,
            Code::Failed,
            Code::Usage,
            Code::Auth,
            Code::Budget,
            Code::Refused,
            Code::Transport,
            Code::Output,
        ];
        let mut numbers: Vec<u8> = codes.iter().map(|c| c.number()).collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), codes.len(), "two codes share a number");
    }

    #[test]
    fn a_site_refusal_is_not_an_auth_failure() {
        let refused: Failure = Error::Exhausted {
            attempts: Vec::new(),
            last: None,
        }
        .into();
        assert_eq!(refused.code, Code::Refused);

        let auth: Failure = Error::Auth("no api key".to_string()).into();
        assert_eq!(auth.code, Code::Auth);
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
