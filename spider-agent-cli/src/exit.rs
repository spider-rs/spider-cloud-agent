//! What the process says when it leaves.
//!
//! A caller branches on the code rather than on the words. Every failure in
//! this tool lands on one of these, and the mapping is the contract: a new
//! failure shape reuses an existing code or adds one here, and nothing else
//! decides it.

use std::fmt;
use std::process::ExitCode;

use spider_cloud_agent::Error;

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
        let codes = Code::ALL;
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
