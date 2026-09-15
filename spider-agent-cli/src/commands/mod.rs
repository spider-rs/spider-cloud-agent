//! One module per group of commands, and the parts they share.

pub mod account;
pub mod local;
pub mod pages;
pub mod query;
pub mod run;

use serde_json::json;
use url::Url;

use spider_cloud_agent::response::{Outcome, PageResult, Pages};

use crate::cli::Format;
use crate::emit::{Emitter, Item};
use crate::exit::{Code, Failure, Run};
use crate::records::{self, Report};

/// Take one operation's result apart into records and totals.
pub fn absorb(emitter: &mut Emitter, report: &mut Report, outcome: Outcome<Pages>) -> Run<()> {
    report.attempts += outcome.attempts.len();
    report.cost += outcome.cost;
    report.add_thrift(&outcome.thrift);
    for result in outcome.value.0 {
        match result {
            PageResult::Ok(page) => {
                report.served += 1;
                emitter.write(records::page(&page))?;
            }
            PageResult::Failed(failed) => {
                report.refused += 1;
                emitter.write(records::failed(&failed))?;
            }
        }
    }
    Ok(())
}

/// A failure against one address, as a record.
pub fn error_item(url: Option<&Url>, failure: &Failure) -> Item {
    let value = json!({
        "type": "error",
        "url": url.map(Url::as_str),
        "code": failure.code.label(),
        "exit": failure.code.number(),
        "message": failure.message,
    });
    match url {
        Some(url) => Item::structured(value).with_url(url.clone()),
        None => Item::structured(value),
    }
}

/// Whether a failure against one address should stop the whole run.
///
/// A key the service refused and a cap that ran out both mean every address
/// after this one fails the same way, and some of those attempts still cost
/// money. A site that refused says nothing about the next site.
pub const fn is_fatal(code: Code) -> bool {
    matches!(code, Code::Auth | Code::Budget | Code::Output)
}

/// Close the run: write the report where it belongs, and say how to leave.
///
/// Under text output the report is a stderr line, because stdout carries the
/// payload. Under JSON and NDJSON it is the last record, so a caller that
/// parses lines never has to read prose to learn what a run cost.
pub fn finish(
    emitter: &mut Emitter,
    report: &Report,
    log: &crate::progress::Log,
    worst: Option<Failure>,
) -> Run<Code> {
    match emitter.format() {
        Format::Text => log.say(report.line()),
        Format::Json | Format::Ndjson => emitter.write_report(report.item())?,
    }
    emitter.finish()?;
    match worst {
        Some(failure) => {
            log.failed(&failure.message);
            Ok(failure.code)
        }
        None => Ok(Code::Ok),
    }
}

/// Keep the most severe failure of a run.
///
/// Severity is the order the codes are written in, which puts a refused key
/// above a refused page. A run that hit both is reported as the one a caller
/// has to act on first.
pub fn worsen(worst: &mut Option<Failure>, failure: Failure) {
    let rank = |code: Code| match code {
        Code::Ok => 0,
        Code::Failed => 1,
        Code::Refused => 2,
        Code::Transport => 3,
        Code::Usage => 4,
        Code::Output => 5,
        Code::Budget => 6,
        Code::Auth => 7,
    };
    match worst {
        Some(held) if rank(held.code) >= rank(failure.code) => {}
        _ => *worst = Some(failure),
    }
}
