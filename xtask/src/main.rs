//! Repo tasks for spider-agent. Run them with `cargo run -p xtask -- <command>`.
//!
//! This crate never ships. It exists so the checks that keep the published crates
//! clean are ordinary rust code with tests, rather than a shell script nobody reads.

mod fixtures;
mod leak_words;
mod leakcheck;
mod redact;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((c, rest)) => (c.as_str(), rest),
        None => {
            usage();
            return ExitCode::FAILURE;
        }
    };
    let result = match command {
        "leakcheck" => leakcheck::run(rest),
        "redact" => redact::run(rest),
        "help" | "--help" | "-h" => {
            usage();
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("unknown command {other}.");
            usage();
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("xtask {command}: {message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "\
xtask, repo tasks for spider-agent.

  cargo run -p xtask -- leakcheck [--tree] [--explain] [--require-private]
      Check what publishing would expose. The default checks the packaged file set,
      the list cargo package would ship. --tree checks the working tree instead,
      which is faster and does not need the workspace to compile. --explain prints
      the denylist categories. Exits non-zero on any finding.

  cargo run -p xtask -- redact <file> [-o <out>] [--in-place]
      Rewrite a recorded api response into a publishable fixture: hosts become
      example.com, authorization headers and keys are dropped, output is pretty
      printed. Writes to stdout unless -o or --in-place is given.

Environment:
  {}   path to the private denylist, one term per line. Required for releases from the
  private checkout. The list compiled in here holds only public terms.",
        leak_words::EXTRA_LIST_ENV
    );
}
