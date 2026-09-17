//! One module per group of commands, and the parts they share.

pub mod account;
pub mod local;
pub mod pages;
pub mod query;
pub mod router;
pub mod run;

use std::time::{Duration, Instant};

use serde_json::json;
use url::Url;

use spider_cloud_agent::response::{Outcome, PageResult, Pages};
use spider_cloud_agent::{Budget, RunBudget, Spider};

use crate::cli::{Format, Global};
use crate::emit::{Emitter, Item};
use crate::exit::{Code, Failure, Run};
use crate::records::{self, Report};
use crate::setup;

/// The caps a run over several addresses shares.
///
/// `--budget` and `--wall` are documented as the most the whole run may
/// spend. The library owns credit admission and settlement. This helper keeps
/// the run's wall and checks the shared credit balance between pages.
pub struct Caps {
    credits: Option<RunBudget>,
    /// In seconds, as the flag reads it.
    wall: Option<u64>,
    started: Instant,
}

impl Caps {
    /// The caps the caller set, counting from now.
    pub fn new(global: &Global, started: Instant, spider: &Spider) -> Caps {
        Caps {
            credits: spider.run_budget().cloned(),
            wall: global.wall,
            started,
        }
    }

    /// Which cap is spent before the next page goes out, if one is.
    ///
    /// Credit admission also checks the estimate immediately before each send.
    pub fn spent(&self, _report: &Report) -> Option<&'static str> {
        if let Some(cap) = &self.credits {
            if cap.remaining() < spider_cloud_agent::policy::budget::ASSUMED_MINIMUM_COST {
                return Some("budget");
            }
        }
        if let Some(seconds) = self.wall {
            if self.started.elapsed() >= Duration::from_secs(seconds) {
                return Some("time");
            }
        }
        None
    }

    /// What the next operation may spend, out of what the run has left.
    pub fn budget(&self, global: &Global, _report: &Report) -> Budget {
        let mut budget = setup::budget(global);
        if let Some(seconds) = self.wall {
            let left = Duration::from_secs(seconds).saturating_sub(self.started.elapsed());
            budget = budget.with_wall(left);
        }
        budget
    }
}

/// Close a run that a cap stopped.
///
/// A spend cap or a time cap that stopped the run leaves under the budget
/// code, so a caller can tell a run that finished from one that ran out
/// without reading the report. A page limit is not that: it is the run doing
/// what it was told, so it leaves with zero.
pub fn close_caps(report: &mut Report, stopped: Option<&'static str>, worst: &mut Option<Failure>) {
    // A stop already on the report came from a budget failure inside one
    // operation, and nothing here unsays it.
    if stopped.is_some() {
        report.stopped = stopped;
    }
    if let Some(reason @ ("budget" | "time")) = stopped {
        worsen(
            worst,
            Failure::new(
                Code::Budget,
                format!(
                    "stopped on {reason}, after {} page(s) and {:.3} credits",
                    report.served + report.refused,
                    report.cost.get()
                ),
            ),
        );
    }
}

/// Write a failure against one address, keep the worst, and say whether the
/// run should stop here.
///
/// A budget failure from inside one operation is the same stop as a cap
/// spent between two, and the report says so either way.
pub fn record(
    emitter: &mut Emitter,
    report: &mut Report,
    worst: &mut Option<Failure>,
    url: Option<&Url>,
    failure: Failure,
) -> Run<bool> {
    if failure.code == Code::Budget {
        report.stopped = Some("budget");
    }
    emitter.write(error_item(url, &failure))?;
    let fatal = is_fatal(failure.code);
    worsen(worst, failure);
    Ok(fatal)
}

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

/// Keep the paid trail before converting the underlying error to the CLI code.
pub fn failure(report: &mut Report, error: spider_cloud_agent::Error) -> Failure {
    report.attempts += error.attempts().len();
    report.cost += error.spent();
    Failure::from(error.into_cause())
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
            if !failure.is_silent() {
                log.failed(&failure.message);
            }
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

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn a_stop_from_inside_one_operation_survives_the_close() {
        let mut report = Report {
            stopped: Some("budget"),
            ..Report::default()
        };
        let mut worst = Some(Failure::new(Code::Budget, "the operation said so"));
        close_caps(&mut report, None, &mut worst);
        assert_eq!(report.stopped, Some("budget"));
        assert_eq!(
            worst.as_ref().map(|f| f.message.as_str()),
            Some("the operation said so")
        );
    }

    #[test]
    fn a_cap_spent_between_two_pages_leaves_under_the_budget_code() {
        let mut report = Report::default();
        let mut worst = None;
        close_caps(&mut report, Some("time"), &mut worst);
        assert_eq!(report.stopped, Some("time"));
        assert_eq!(worst.map(|f| f.code), Some(Code::Budget));

        let mut worst = None;
        close_caps(&mut report, Some("pages"), &mut worst);
        assert!(
            worst.is_none(),
            "a page limit is the run doing what it was told"
        );
    }
}
