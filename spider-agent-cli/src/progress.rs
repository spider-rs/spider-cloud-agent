//! Diagnostics, on stderr, always.
//!
//! Nothing here reaches stdout. There is no spinner and no escape code, on a
//! terminal or off it, so a line that appears in a terminal is the same line
//! that appears in a log file.
//!
//! Every line is written on a best effort basis. `eprintln!` panics when
//! stderr cannot be written, and stderr is a pipe as often as stdout is, so a
//! reader that closed it would have turned a progress note into a panic and,
//! under the release profile, into an abort. A note nobody is reading is
//! dropped instead.

use std::io::Write;

/// What gets printed while a run is going.
#[derive(Debug, Clone, Copy)]
pub struct Log {
    quiet: bool,
    verbose: bool,
}

impl Log {
    /// Build from the two flags.
    pub const fn new(quiet: bool, verbose: bool) -> Log {
        Log { quiet, verbose }
    }

    /// A line only a caller who asked for detail wants.
    pub fn note(&self, message: impl AsRef<str>) {
        if self.verbose && !self.quiet {
            line(message.as_ref());
        }
    }

    /// A line worth printing unless the caller asked for silence.
    pub fn say(&self, message: impl AsRef<str>) {
        if !self.quiet {
            line(message.as_ref());
        }
    }

    /// A failure. Printed whatever the flags say, because a caller who
    /// silenced progress did not silence what went wrong.
    pub fn failed(&self, message: impl AsRef<str>) {
        line(message.as_ref());
    }
}

/// One line on stderr, and nothing said if it cannot be written.
fn line(message: &str) {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(message.as_bytes());
    let _ = err.write_all(b"\n");
}
