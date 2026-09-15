//! `spider-agent`, the command line tool.
//!
//! Results go to stdout and diagnostics go to stderr, so a pipe carries the
//! payload and nothing else. Nothing here writes an escape code, prompts for
//! anything, or draws a spinner, because the caller is as likely to be another
//! program as a person.
//!
//! The agency is local and deterministic. Transport is chosen before a call
//! goes out, escalation answers the status the site returned, and a budget the
//! caller set stops the run. No large model is called, no model key is read,
//! and nothing in here decides anything by asking one.

#![forbid(unsafe_code)]

mod cli;
mod commands;
mod emit;
mod exit;
mod progress;
mod records;
mod setup;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::exit::{Code, Failure};
use crate::progress::Log;

fn main() -> ExitCode {
    let parsed = match Cli::try_parse() {
        Ok(parsed) => parsed,
        Err(error) => return from_clap(error),
    };
    let log = Log::new(parsed.global.quiet, parsed.global.verbose);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            log.failed(format!("could not start the async runtime: {error}"));
            return Code::Failed.into();
        }
    };

    match runtime.block_on(dispatch(&parsed, log)) {
        Ok(code) => code.into(),
        Err(failure) => {
            log.failed(&failure.message);
            failure.code.into()
        }
    }
}

/// Run the command the caller named, or scrape when they named none.
async fn dispatch(parsed: &Cli, log: Log) -> Result<Code, Failure> {
    let global = &parsed.global;
    match &parsed.command {
        None => commands::pages::scrape(global, &parsed.default, log).await,
        Some(Command::Scrape(args)) => commands::pages::scrape(global, args, log).await,
        Some(Command::Fetch(args)) => commands::pages::fetch(global, args, log).await,
        Some(Command::Crawl(args)) => commands::pages::crawl(global, args, log).await,
        Some(Command::Extract(args)) => commands::pages::extract(global, args, log).await,
        Some(Command::Links(args)) => commands::pages::links(global, args, log).await,
        Some(Command::Search(args)) => commands::query::search(global, args, log).await,
        Some(Command::Screenshot(args)) => commands::pages::screenshot(global, args, log).await,
        Some(Command::Transform(args)) => commands::pages::transform(global, args, log).await,
        Some(Command::Run(args)) => commands::run::run(global, args, log).await,
        Some(Command::Credits) => commands::account::credits(global, log).await,
        Some(Command::Logs(args)) => commands::account::logs(global, args, log).await,
        Some(Command::Sites(args)) => commands::account::sites(global, args, log).await,
        Some(Command::Keys(args)) => commands::account::keys(global, args, log).await,
        Some(Command::Profile) => commands::account::profile(global, log).await,
        Some(Command::Login(args)) => commands::account::login(global, args, log).await,
        Some(Command::Route(args)) => commands::local::route(global, args, log),
        Some(Command::Schema) => commands::local::schema(global),
    }
}

/// Turn a clap outcome into an exit code.
///
/// Help and version are a successful run that happens to print to stdout.
/// Everything else is a usage failure and goes to stderr, so a caller reading
/// the pipe never finds a help page where a payload should be.
fn from_clap(error: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;
    match error.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let text = error.render();
            let mut out = std::io::stdout();
            let _ = out.write_all(text.to_string().as_bytes());
            let _ = out.flush();
            Code::Ok.into()
        }
        _ => {
            let text = error.render();
            let mut err = std::io::stderr();
            let _ = err.write_all(text.to_string().as_bytes());
            let _ = err.flush();
            Code::Usage.into()
        }
    }
}
