//! The commands that read pages.

use std::time::Instant;

use spider_cloud_agent::ops::transform::Document;
use spider_cloud_agent::params::ReturnFormat;
use spider_cloud_agent::Spider;

use crate::cli::{
    CrawlArgs, ExtractArgs, FetchArgs, Format, Global, Goal, LinksArgs, ScrapeArgs, ScreenshotArgs,
    TransformArgs,
};
use crate::commands::{absorb, close_caps, finish, record, Caps};
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;
use crate::records::{self, Report};
use crate::setup::{self, pin};

/// Read one page or a list of them.
pub async fn scrape(global: &Global, args: &ScrapeArgs, log: Log) -> Run<Code> {
    let need = setup::need(args.goal, args.selectors.as_deref(), None)?;
    let urls = setup::addresses(&args.targets)?;
    if args.goal == Goal::Screenshot {
        setup::refuse_bytes_on_a_terminal(global)?;
    }
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    let mut report = Report {
        targets: urls.len(),
        ..Report::default()
    };
    let started = Instant::now();
    let caps = Caps::new(global, started);
    let mut worst = None;
    let mut stopped = None;

    for url in &urls {
        if let Some(reason) = caps.spent(&report) {
            stopped = Some(reason);
            break;
        }
        log.note(format!("reading {url}"));
        let call = pin!(
            spider
                .scrape(url.clone())
                .need(need.clone())
                .budget(caps.budget(global, &report)),
            global,
            links
        );
        match call.send_all().await {
            Ok(outcome) => {
                if let Some(route) = &outcome.route {
                    log.note(format!(
                        "{url}: {} over {}, {} call(s)",
                        route.mode().as_str(),
                        route.proxy().as_str(),
                        outcome.attempts.len()
                    ));
                }
                absorb(&mut emitter, &mut report, outcome)?;
            }
            Err(error) => {
                if record(
                    &mut emitter,
                    &mut report,
                    &mut worst,
                    Some(url),
                    Failure::from(error),
                )? {
                    break;
                }
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    close_caps(&mut report, stopped, &mut worst);
    finish(&mut emitter, &report, &log, worst)
}

/// Read one path under the config the service holds for it.
///
/// A different endpoint from [`scrape`], not a different spelling of it: the
/// target is the address rather than a field in the request, and the service
/// answers with a configuration it already holds for that path.
pub async fn fetch(global: &Global, args: &FetchArgs, log: Log) -> Run<Code> {
    let need = setup::need(args.goal, None, None)?;
    if args.goal == Goal::Screenshot {
        setup::refuse_bytes_on_a_terminal(global)?;
    }
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    let mut report = Report {
        targets: 1,
        ..Report::default()
    };
    let started = Instant::now();
    let mut worst = None;

    // The address the service will read, built the way it builds it, so a
    // failure names the page rather than the two halves it came from.
    let target = setup::parse_all(&[format!(
        "https://{}/{}",
        args.domain,
        args.path.trim_start_matches('/')
    )])?;
    let Some(target) = target.first() else {
        return Err(Failure::usage("that is not a host and a path"));
    };
    log.note(format!("reading {target} under its stored config"));
    let call = pin!(
        spider
            .fetch(args.domain.clone(), args.path.clone())
            .need(need),
        global,
        links
    );
    match call.send_all().await {
        Ok(outcome) => absorb(&mut emitter, &mut report, outcome)?,
        Err(error) => {
            record(
                &mut emitter,
                &mut report,
                &mut worst,
                Some(target),
                Failure::from(error),
            )?;
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    finish(&mut emitter, &report, &log, worst)
}

/// Read a site, following its links.
pub async fn crawl(global: &Global, args: &CrawlArgs, log: Log) -> Run<Code> {
    let need = setup::need(args.goal, args.selectors.as_deref(), None)?;
    let urls = setup::addresses(&args.targets)?;
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Ndjson)?;
    let mut report = Report {
        targets: urls.len(),
        ..Report::default()
    };
    let started = Instant::now();
    let caps = Caps::new(global, started);
    let mut worst = None;
    let mut stopped = None;

    for url in &urls {
        if let Some(reason) = caps.spent(&report) {
            stopped = Some(reason);
            break;
        }
        log.note(format!("crawling {url}"));
        let mut call = pin!(
            spider
                .crawl(url.clone())
                .need(need.clone())
                .budget(caps.budget(global, &report)),
            global,
            links
        );
        if let Some(limit) = args.limit {
            call = call.limit(limit);
        }
        if let Some(depth) = args.depth {
            call = call.depth(depth);
        }
        match call.send_all().await {
            Ok(outcome) => absorb(&mut emitter, &mut report, outcome)?,
            Err(error) => {
                if record(
                    &mut emitter,
                    &mut report,
                    &mut worst,
                    Some(url),
                    Failure::from(error),
                )? {
                    break;
                }
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    close_caps(&mut report, stopped, &mut worst);
    finish(&mut emitter, &report, &log, worst)
}

/// Pull named fields out of pages.
pub async fn extract(global: &Global, args: &ExtractArgs, log: Log) -> Run<Code> {
    let need = setup::fields(&args.selectors, args.at.as_deref())?;
    let urls = setup::addresses(&args.targets)?;
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Ndjson)?;
    let mut report = Report {
        targets: urls.len(),
        ..Report::default()
    };
    let started = Instant::now();
    let caps = Caps::new(global, started);
    let mut worst = None;
    let mut stopped = None;

    for url in &urls {
        if let Some(reason) = caps.spent(&report) {
            stopped = Some(reason);
            break;
        }
        log.note(format!("extracting from {url}"));
        let call = pin!(
            spider
                .scrape(url.clone())
                .need(need.clone())
                .budget(caps.budget(global, &report)),
            global,
            links
        );
        match call.send_all().await {
            Ok(outcome) => absorb(&mut emitter, &mut report, outcome)?,
            Err(error) => {
                if record(
                    &mut emitter,
                    &mut report,
                    &mut worst,
                    Some(url),
                    Failure::from(error),
                )? {
                    break;
                }
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    close_caps(&mut report, stopped, &mut worst);
    finish(&mut emitter, &report, &log, worst)
}

/// Collect the links on a page.
pub async fn links(global: &Global, args: &LinksArgs, log: Log) -> Run<Code> {
    let urls = setup::addresses(&args.targets)?;
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    let mut report = Report {
        targets: urls.len(),
        ..Report::default()
    };
    let started = Instant::now();
    let caps = Caps::new(global, started);
    let mut worst = None;
    let mut stopped = None;

    for url in &urls {
        if let Some(reason) = caps.spent(&report) {
            stopped = Some(reason);
            break;
        }
        log.note(format!("reading the links on {url}"));
        let call = pin!(
            spider
                .links(url.clone())
                .budget(caps.budget(global, &report)),
            global
        );
        match call.send_all().await {
            Ok(outcome) => {
                report.attempts += outcome.attempts.len();
                report.cost += outcome.cost;
                report.add_thrift(&outcome.thrift);
                for result in outcome.value.0 {
                    match result {
                        spider_cloud_agent::response::PageResult::Ok(page) => {
                            report.served += 1;
                            for found in page.links.iter().flatten() {
                                emitter.write(records::link(found, &page.url))?;
                            }
                        }
                        spider_cloud_agent::response::PageResult::Failed(failed) => {
                            report.refused += 1;
                            emitter.write(records::failed(&failed))?;
                        }
                    }
                }
            }
            Err(error) => {
                if record(
                    &mut emitter,
                    &mut report,
                    &mut worst,
                    Some(url),
                    Failure::from(error),
                )? {
                    break;
                }
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    close_caps(&mut report, stopped, &mut worst);
    finish(&mut emitter, &report, &log, worst)
}

/// Take a picture of a page.
pub async fn screenshot(global: &Global, args: &ScreenshotArgs, log: Log) -> Run<Code> {
    let urls = setup::addresses(&args.targets)?;
    setup::refuse_bytes_on_a_terminal(global)?;
    if urls.len() > 1
        && global.output_dir.is_none()
        && setup::format(global, Format::Text) == Format::Text
    {
        return Err(Failure::usage(
            "several pictures into one file would be one unreadable file. Pass --output-dir.",
        ));
    }
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    let mut report = Report {
        targets: urls.len(),
        ..Report::default()
    };
    let started = Instant::now();
    let caps = Caps::new(global, started);
    let mut worst = None;
    let mut stopped = None;

    for url in &urls {
        if let Some(reason) = caps.spent(&report) {
            stopped = Some(reason);
            break;
        }
        log.note(format!("shooting {url}"));
        let call = pin!(
            spider
                .screenshot(url.clone())
                .budget(caps.budget(global, &report)),
            global,
            links
        );
        match call.send_all().await {
            Ok(outcome) => absorb(&mut emitter, &mut report, outcome)?,
            Err(error) => {
                if record(
                    &mut emitter,
                    &mut report,
                    &mut worst,
                    Some(url),
                    Failure::from(error),
                )? {
                    break;
                }
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    close_caps(&mut report, stopped, &mut worst);
    finish(&mut emitter, &report, &log, worst)
}

/// Convert markup you already hold. Nothing is fetched.
pub async fn transform(global: &Global, args: &TransformArgs, log: Log) -> Run<Code> {
    let markup = setup::source(&args.input)?;
    let mut document = Document::html(markup);
    if let Some(url) = &args.url {
        document = document.from_url(url.clone());
    }
    let spider: Spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    let mut report = Report {
        targets: 1,
        ..Report::default()
    };
    let started = Instant::now();

    let mut call = spider.transform(vec![document]).format(format_of(args.to));
    if args.readability {
        call = call.readability(true);
    }
    let mut worst = None;
    match call.send_all().await {
        Ok(outcome) => absorb(&mut emitter, &mut report, outcome)?,
        Err(error) => {
            record(
                &mut emitter,
                &mut report,
                &mut worst,
                None,
                Failure::from(error),
            )?;
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    finish(&mut emitter, &report, &log, worst)
}

/// The shape a conversion asks for.
fn format_of(goal: Goal) -> ReturnFormat {
    match goal {
        Goal::Text => ReturnFormat::Text,
        // The markup, which the wire calls raw.
        Goal::Html | Goal::Raw => ReturnFormat::Raw,
        _ => ReturnFormat::Markdown,
    }
}
