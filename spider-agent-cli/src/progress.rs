//! Diagnostics, on stderr, always.
//!
//! Nothing here reaches stdout. There is no spinner and no escape code, on a
//! terminal or off it, so a line that appears in a terminal is the same line
//! that appears in a log file.

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
            eprintln!("{}", message.as_ref());
        }
    }

    /// A line worth printing unless the caller asked for silence.
    pub fn say(&self, message: impl AsRef<str>) {
        if !self.quiet {
            eprintln!("{}", message.as_ref());
        }
    }

    /// A failure. Printed whatever the flags say, because a caller who
    /// silenced progress did not silence what went wrong.
    pub fn failed(&self, message: impl AsRef<str>) {
        eprintln!("{}", message.as_ref());
    }
}
