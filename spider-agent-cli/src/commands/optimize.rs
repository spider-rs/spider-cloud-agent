//! The stored optimizer settings: set, show and clear.
//!
//! None of these makes a call. The settings live in `~/.spider/optimize.json`,
//! written owner only, and hold a mode and the path to a weights file. No key
//! is read here and none is stored.

use crate::cli::{Format, Global, OptimizeArgs, OptimizeCommand, OptimizeSetArgs};
use crate::exit::{Code, Failure, Run};
use crate::optimize::{self, Mode, OPTIMIZE_PATH};
use crate::progress::Log;
use crate::records;
use crate::setup;

/// Run the `optimize` subcommand the caller named.
pub fn command(global: &Global, args: &OptimizeArgs, log: Log) -> Run<Code> {
    match &args.command {
        OptimizeCommand::Set(set_args) => set(set_args, log),
        OptimizeCommand::Show => show(global),
        OptimizeCommand::Clear => clear(log),
    }
}

/// Merge the flags into the stored settings and write them back.
fn set(args: &OptimizeSetArgs, log: Log) -> Run<Code> {
    if args.mode.is_none() && args.model.is_none() && !args.no_model {
        return Err(Failure::usage(
            "nothing to set. Name --mode, --model or --no-model.",
        ));
    }
    let mut stored = optimize::load()?.unwrap_or_default();
    if let Some(mode) = optimize::chosen(args.mode) {
        stored.mode = Some(mode.as_str().to_string());
    }
    if args.no_model {
        stored.model = None;
    } else if let Some(model) = &args.model {
        stored.model = Some(model.clone());
    }

    let path = optimize::store_settings(&stored)?;
    #[cfg(feature = "optimize")]
    if stored.mode() != Mode::Off && stored.model.is_none() {
        log.say(
            "no weights are stored, so the optimizer abstains and every request goes out as it is",
        );
    }
    // A binary built without the optimizer stores the settings and refuses to
    // run under them, so it says which it is before the run does.
    #[cfg(not(feature = "optimize"))]
    if stored.mode() != Mode::Off {
        log.say("this binary was built without the optimizer, so a run under these settings fails until it is rebuilt with the optimize feature");
    }
    log.say(format!(
        "stored the optimizer settings in {}",
        path.display()
    ));
    Ok(Code::Ok)
}

/// Write the stored settings as a record.
fn show(global: &Global) -> Run<Code> {
    let stored = optimize::load()?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    emitter.write_sole(records::optimize(stored.as_ref()))?;
    emitter.finish()?;
    Ok(Code::Ok)
}

/// Delete the settings, and say whether there were any.
fn clear(log: Log) -> Run<Code> {
    if optimize::clear()? {
        log.say(format!("removed ~/{OPTIMIZE_PATH}"));
    } else {
        log.say("there were no stored optimizer settings");
    }
    Ok(Code::Ok)
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::cli::Mode as CliMode;
    use crate::optimize::StoredOptimize;

    #[test]
    fn a_set_that_sets_nothing_is_a_usage_failure() {
        let args = OptimizeSetArgs {
            mode: None,
            model: None,
            no_model: false,
        };
        let failed = set(&args, Log::new(true, false)).expect_err("a usage failure");
        assert_eq!(failed.code, Code::Usage);
        assert!(failed.message.contains("--mode"), "{failed}");
    }

    #[test]
    fn the_stored_settings_are_what_the_flags_said() {
        // What `set` writes, without touching this machine's file.
        let mut stored = StoredOptimize {
            mode: optimize::chosen(Some(CliMode::Apply)).map(|mode| mode.as_str().to_string()),
            model: Some("weights.bin".to_string()),
        };
        assert_eq!(stored.mode(), Mode::Apply);
        assert!(stored.validate().is_ok());
        // --no-model forgets the weights and leaves the mode as it is.
        stored.model = None;
        assert_eq!(stored.mode(), Mode::Apply);
    }
}
