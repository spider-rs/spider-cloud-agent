//! The mode that works a goal rather than a request.
//!
//! What makes it agentic is not a model, because this tool calls none. It is
//! four decisions the run makes without being told:
//!
//! - which transport the first attempt uses, decided locally from the shape of
//!   the address before anything is sent
//! - what to change when a site refuses, decided from the status the site
//!   returned rather than from a fixed retry count
//! - what the run has learned about a host, carried across addresses, so the
//!   second page on a host that already refused starts further up the ladder
//!   instead of paying for the cheap attempt again
//! - when to stop, from the cap the caller set, checked against what is left
//!   before every page rather than counted afterwards
//!
//! The frontier is the fifth. A run asked to expand reads the links off the
//! pages it already has and works them too, same host only, so one address and
//! a goal turn into a walk the caller did not have to spell out.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde_json::Value;
use url::Url;

use spider_cloud_agent::{Budget, Credits, Need};

use crate::cli::{Format, Global, Goal, RunArgs};
use crate::commands::{close_caps, finish, record};
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;
use crate::records::{self, Report};
use crate::setup::{self, pin};

/// What the run was asked to do, from the command line and the plan file
/// together.
struct Plan {
    need: Need,
    urls: Vec<Url>,
    expand: usize,
    max_pages: Option<usize>,
    credits: Option<f64>,
}

/// Work the goal until it is met, the frontier empties, or a cap stops it.
pub async fn run(global: &Global, args: &RunArgs, log: Log) -> Run<Code> {
    let plan = settle(global, args)?;
    if plan.need == Need::screenshot() {
        setup::refuse_bytes_on_a_terminal(global)?;
    }
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Ndjson)?;
    let mut report = Report {
        targets: plan.urls.len(),
        ..Report::default()
    };
    let started = Instant::now();
    let mut worst = None;

    let mut frontier: VecDeque<Url> = plan.urls.iter().cloned().collect();
    let mut seen: Vec<String> = plan.urls.iter().map(|u| u.as_str().to_string()).collect();
    let mut followed = 0usize;
    let mut pages = 0usize;
    let mut spent = Credits::ZERO;
    let mut stopped = None;

    while let Some(url) = frontier.pop_front() {
        if let Some(cap) = plan.max_pages {
            if pages >= cap {
                stopped = Some("pages");
                break;
            }
        }
        if let Some(cap) = plan.credits {
            if spent.get() >= cap {
                stopped = Some("budget");
                break;
            }
        }
        if let Some(seconds) = global.wall {
            if started.elapsed() >= Duration::from_secs(seconds) {
                stopped = Some("time");
                break;
            }
        }

        let budget = remaining(global, plan.credits, spent, started);
        log.note(format!("working {url}"));

        let mut call = pin!(
            spider
                .scrape(url.clone())
                .need(plan.need.clone())
                .budget(budget),
            global
        );
        if plan.expand > 0 {
            // A caller pin, so the need's own plan cannot switch it back off.
            // The frontier is built from these.
            call.params_mut().return_page_links = Some(true);
        }

        pages += 1;
        match call.send_all().await {
            Ok(outcome) => {
                report.attempts += outcome.attempts.len();
                report.cost += outcome.cost;
                spent += outcome.cost;
                report.add_thrift(&outcome.thrift);
                if outcome.attempts.len() > 1 {
                    log.note(format!(
                        "{url}: took {} calls, the ladder answered the status",
                        outcome.attempts.len()
                    ));
                }
                for result in outcome.value.0 {
                    match result {
                        spider_cloud_agent::response::PageResult::Ok(page) => {
                            report.served += 1;
                            if plan.expand > 0 && followed < plan.expand {
                                followed +=
                                    queue(&mut frontier, &mut seen, &page, plan.expand - followed);
                            }
                            emitter.write(records::page(&page))?;
                        }
                        spider_cloud_agent::response::PageResult::Failed(failed) => {
                            report.refused += 1;
                            log.note(format!(
                                "{}: {} refused it, next move would be {}",
                                failed.url,
                                failed.status,
                                records::hint(failed.hint)
                            ));
                            emitter.write(records::failed(&failed))?;
                        }
                    }
                }
            }
            Err(error) => {
                let failure = Failure::from(error);
                if failure.code == Code::Budget {
                    stopped = Some("budget");
                }
                if record(&mut emitter, &mut report, &mut worst, Some(&url), failure)? {
                    break;
                }
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    close_caps(&mut report, stopped, &mut worst);
    finish(&mut emitter, &report, &log, worst)
}

/// What one page may spend, out of what the run has left.
fn remaining(global: &Global, credits: Option<f64>, spent: Credits, started: Instant) -> Budget {
    let mut budget = setup::budget(global);
    if let Some(cap) = credits {
        budget = budget.with_credits(Credits::new((cap - spent.get()).max(0.0)));
    }
    if let Some(seconds) = global.wall {
        let left = Duration::from_secs(seconds).saturating_sub(started.elapsed());
        budget = budget.with_wall(left);
    }
    budget
}

/// Add the same-host links off one page to the frontier.
///
/// Same host only. A run that followed every outbound link would walk the web,
/// and the caller asked about a site.
fn queue(
    frontier: &mut VecDeque<Url>,
    seen: &mut Vec<String>,
    page: &spider_cloud_agent::response::Page,
    room: usize,
) -> usize {
    let mut added = 0usize;
    let host = page.url.host_str().map(str::to_string);
    for link in page.links.iter().flatten() {
        if added >= room {
            break;
        }
        if link.host_str().map(str::to_string) != host {
            continue;
        }
        let key = link.as_str().to_string();
        if seen.iter().any(|held| held == &key) {
            continue;
        }
        seen.push(key);
        frontier.push_back(link.clone());
        added += 1;
    }
    added
}

/// The command line over the plan file.
///
/// A plan file is how an agent hands over a goal and a long list of addresses
/// without fighting argv quoting. Anything named on the command line wins,
/// because that is the thing the caller typed last.
fn settle(global: &Global, args: &RunArgs) -> Run<Plan> {
    let mut goal = args.goal;
    let mut urls: Vec<String> = args.targets.urls.clone();
    let mut expand = args.expand;
    let mut max_pages = args.max_pages;
    let mut credits = global.budget;
    let mut selectors: Option<Value> = None;

    if let Some(spec) = &args.plan {
        let text = setup::source(spec)?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| Failure::usage(format!("the plan in {spec} is not JSON: {e}")))?;
        let Value::Object(plan) = value else {
            return Err(Failure::usage(format!(
                "the plan in {spec} has to be a JSON object."
            )));
        };
        if let Some(Value::String(named)) = plan.get("goal") {
            goal = parse_goal(named)?;
        }
        if let Some(Value::Array(listed)) = plan.get("urls") {
            for one in listed {
                match one {
                    Value::String(url) => urls.push(url.clone()),
                    other => {
                        return Err(Failure::usage(format!(
                            "an address in the plan is not a string: {other}"
                        )))
                    }
                }
            }
        }
        if let Some(value) = plan.get("expand").and_then(Value::as_u64) {
            if args.expand == 0 {
                expand = value as usize;
            }
        }
        if max_pages.is_none() {
            max_pages = plan
                .get("max_pages")
                .and_then(Value::as_u64)
                .map(|n| n as usize);
        }
        if credits.is_none() {
            credits = plan.get("budget").and_then(Value::as_f64);
        }
        if let Some(map) = plan.get("selectors") {
            selectors = Some(map.clone());
        }
    }

    let need = match (&args.selectors, selectors) {
        (Some(spec), _) => setup::fields(spec, None)?,
        (None, Some(map)) => setup::fields_from_value(&map, None)?,
        (None, None) => setup::need(goal, None, None)?,
    };

    if urls.is_empty() {
        let listed = setup::addresses(&args.targets)?;
        return Ok(Plan {
            need,
            urls: listed,
            expand,
            max_pages,
            credits,
        });
    }

    Ok(Plan {
        need,
        urls: setup::parse_all(&urls)?,
        expand,
        max_pages,
        credits,
    })
}

/// A goal named in a plan file.
fn parse_goal(named: &str) -> Run<Goal> {
    Ok(match named {
        "text" => Goal::Text,
        "markdown" => Goal::Markdown,
        "html" => Goal::Html,
        "links" => Goal::Links,
        "metadata" => Goal::Metadata,
        "fields" => Goal::Fields,
        "screenshot" => Goal::Screenshot,
        "raw" => Goal::Raw,
        other => {
            return Err(Failure::usage(format!(
                "{other} is not a goal. The goals are text, markdown, html, links, metadata, fields, screenshot and raw."
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn a_goal_that_is_not_one_names_the_goals() {
        let failed = parse_goal("summarise").expect_err("a usage failure");
        assert_eq!(failed.code, Code::Usage);
        assert!(failed.message.contains("markdown"), "{failed}");
    }

    #[test]
    fn every_goal_the_command_line_takes_is_readable_in_a_plan() {
        for named in [
            "text",
            "markdown",
            "html",
            "links",
            "metadata",
            "fields",
            "screenshot",
            "raw",
        ] {
            assert!(parse_goal(named).is_ok(), "{named}");
        }
    }
}
