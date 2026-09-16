//! Search.

use std::time::Instant;

use crate::cli::{Format, Global, SearchArgs};
use crate::commands::{absorb, error_item, finish, worsen};
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;
use crate::records::{self, Report};
use crate::setup::{self, pin};

/// Run a query, and read the pages behind the results when asked to.
pub async fn search(global: &Global, args: &SearchArgs, log: Log) -> Run<Code> {
    let query = args.query.join(" ");
    if query.trim().is_empty() {
        return Err(Failure::usage("the query is empty."));
    }
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Ndjson)?;
    let mut report = Report {
        targets: 1,
        ..Report::default()
    };
    let started = Instant::now();
    let mut worst = None;

    if args.fetch_pages {
        log.note(format!("searching for {query} and reading every result"));
        let call = pin!(spider.search(&query), global, links);
        let mut call = call;
        if let Some(limit) = args.limit {
            call = call.limit(limit);
        }
        match call.send_all().await {
            Ok(outcome) => absorb(&mut emitter, &mut report, outcome)?,
            Err(error) => {
                let failure = super::failure(&mut report, error);
                emitter.write(error_item(None, &failure))?;
                worsen(&mut worst, failure);
            }
        }
    } else {
        log.note(format!("searching for {query}"));
        let mut call = spider.search(&query);
        if let Some(limit) = args.limit {
            call = call.limit(limit);
        }
        match call.send().await {
            Ok(outcome) => {
                report.attempts += outcome.attempts.len();
                report.cost += outcome.cost;
                for entry in outcome.value.entries() {
                    report.served += 1;
                    emitter.write(records::result(entry))?;
                }
            }
            Err(error) => {
                let failure = super::failure(&mut report, error);
                emitter.write(error_item(None, &failure))?;
                worsen(&mut worst, failure);
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    finish(&mut emitter, &report, &log, worst)
}
