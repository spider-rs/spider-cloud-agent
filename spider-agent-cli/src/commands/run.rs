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

use spider_cloud_agent::{Budget, Need};

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
    let spider = setup::client_with_credits(global, plan.credits)?;
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
    let mut stopped = None;

    while let Some(url) = frontier.pop_front() {
        if let Some(cap) = plan.max_pages {
            if pages >= cap {
                stopped = Some("pages");
                break;
            }
        }
        if let Some(cap) = spider.run_budget() {
            if cap.remaining() < spider_cloud_agent::policy::budget::ASSUMED_MINIMUM_COST {
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

        let budget = remaining(global, started);
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
                let failure = super::failure(&mut report, error);
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
fn remaining(global: &Global, started: Instant) -> Budget {
    let mut budget = setup::budget(global);
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
    let plan: PlanFile = if let Some(spec) = &args.plan {
        serde_json::from_str(&setup::source(spec)?)
            .map_err(|e| Failure::usage(format!("the plan in {spec} is invalid: {e}")))?
    } else {
        PlanFile::default()
    };
    // Validate even values that a command line flag will replace.
    let planned_goal = plan.goal.as_deref().map(parse_goal).transpose()?;
    let planned_budget = plan
        .budget
        .map(|n| crate::cli::credits(&n.to_string()))
        .transpose()
        .map_err(|e| Failure::usage(format!("plan budget: {e}")))?;
    let planned_fields = plan
        .selectors
        .as_ref()
        .map(|map| setup::fields_from_value(map, None))
        .transpose()?;
    let goal = args.goal.or(planned_goal).unwrap_or(Goal::Markdown);
    let need = if let Some(spec) = &args.selectors {
        setup::fields(spec, None)?
    } else if let Some(fields) = planned_fields {
        fields
    } else {
        setup::need(goal, None, None)?
    };
    let mut urls = args.targets.urls.clone();
    urls.extend(plan.urls);
    let targets = crate::cli::Targets {
        urls,
        urls_from: args.targets.urls_from.clone(),
    };
    Ok(Plan {
        need,
        urls: setup::addresses(&targets)?,
        expand: args.expand.or(plan.expand).unwrap_or(0),
        max_pages: args.max_pages.or(plan.max_pages),
        credits: global.budget.or(planned_budget),
    })
}

/// Values read from JSON before command line overrides are applied.
#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanFile {
    #[serde(default, deserialize_with = "present")]
    goal: Option<String>,
    #[serde(default)]
    urls: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    expand: Option<usize>,
    #[serde(default, deserialize_with = "present")]
    max_pages: Option<usize>,
    #[serde(default, deserialize_with = "present")]
    budget: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    selectors: Option<Value>,
}

/// Missing keys are optional; a supplied null is not a typed value.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// A goal named in a plan file.
fn parse_goal(named: &str) -> Run<Goal> {
    use clap::ValueEnum;
    Goal::from_str(named, false).map_err(|_| {
        let goals: Vec<String> = Goal::value_variants()
            .iter()
            .filter_map(|goal| goal.to_possible_value().map(|v| v.get_name().to_owned()))
            .collect();
        Failure::usage(format!(
            "{named} is not a goal. The goals are {}.",
            goals.join(", ")
        ))
    })
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn typed_plan_values_are_checked_before_overrides() {
        use clap::Parser;
        let path = std::env::temp_dir().join(format!(
            "spider-agent-plan-unit-{}.json",
            std::process::id()
        ));
        let parsed = crate::cli::Cli::try_parse_from([
            "spider-agent",
            "run",
            "https://example.com/",
            "--plan",
            path.to_str().unwrap(),
            "--goal",
            "markdown",
            "--expand",
            "0",
            "--budget",
            "10",
        ])
        .unwrap();
        let Some(crate::cli::Command::Run(ref args)) = parsed.command else {
            panic!("run arguments")
        };
        for body in [
            r#"{"budget":"10"}"#,
            r#"{"budget":-1}"#,
            r#"{"budget":null}"#,
            r#"{"expand":-1}"#,
            r#"{"expand":1.5}"#,
            r#"{"max_pages":"2"}"#,
            r#"{"goal":"typo"}"#,
            r#"{"urls":[1]}"#,
            r#"{"selectors":{}}"#,
        ] {
            std::fs::write(&path, body).unwrap();
            let error = settle(&parsed.global, args).err().expect("invalid plan");
            assert_eq!(error.code, Code::Usage, "{body}");
        }
        std::fs::write(
            &path,
            r#"{"goal":"html","expand":4,"budget":0,"urls":["https://example.com/b"]}"#,
        )
        .unwrap();
        let plan = settle(&parsed.global, args).unwrap();
        assert_eq!(plan.need, Need::Markdown);
        assert_eq!(plan.expand, 0);
        assert_eq!(plan.credits, Some(10.0));
        assert_eq!(plan.urls.len(), 2);
        std::fs::remove_file(path).unwrap();
    }

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
