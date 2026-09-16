//! Turning a command line into a client, a need and a list of addresses.

use std::io::{IsTerminal, Read};
use std::time::Duration;

use serde_json::Value;
use url::Url;

use spider_cloud_agent::params::{Country, ProxyPool, RequestMode};
use spider_cloud_agent::DeclaredNeed;
use spider_cloud_agent::{Budget, Credits, Need, Spider};

use crate::cli::{Format, Global, Goal, Mode, Pool, Targets};
use crate::emit::{Emitter, FileRules};
use crate::exit::{Code, Failure, Run};

/// Read a file, or stdin when the name is a single dash.
///
/// A byte order mark at the start is dropped. An editor on Windows writes one
/// in front of a list of addresses or a selector map, and it is part of
/// neither: left in, the first address fails to parse and a JSON file is
/// refused at its first byte.
pub fn source(spec: &str) -> Run<String> {
    let text = if spec == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| Failure::usage(format!("could not read stdin: {e}")))?;
        text
    } else {
        std::fs::read_to_string(spec)
            .map_err(|e| Failure::usage(format!("could not read {spec}: {e}")))?
    };
    Ok(without_bom(text))
}

/// The text with a leading byte order mark removed.
fn without_bom(text: String) -> String {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// The addresses to work on.
///
/// Named on the command line, read from a file, or read from stdin when
/// nothing was named and stdin is a pipe. That last case is what makes
/// `cat urls.txt | spider-agent extract ...` work without a shell loop.
pub fn addresses(targets: &Targets) -> Run<Vec<Url>> {
    let mut raw: Vec<String> = targets.urls.clone();
    if let Some(spec) = &targets.urls_from {
        raw.extend(lines(&source(spec)?));
    }
    if raw.is_empty() && !std::io::stdin().is_terminal() {
        raw.extend(lines(&source("-")?));
    }
    if raw.is_empty() {
        return Err(Failure::usage(
            "no address. Name one, pass --urls-from, or pipe a list on stdin.",
        ));
    }
    parse_all(&raw)
}

/// Non-empty lines, with comments and surrounding space removed.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Parse every address, naming the one that is not an address.
///
/// A bare word gets the command list pointed at it, because typing a command
/// this version does not have is the ordinary way to land here: anything that
/// is not a known command is read as an address to fetch.
pub fn parse_all(raw: &[String]) -> Run<Vec<Url>> {
    let mut out = Vec::with_capacity(raw.len());
    for one in raw {
        out.push(Url::parse(one).map_err(|e| {
            let hint = if one.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                ". If you meant a command, spider-agent --help lists them."
            } else {
                ""
            };
            Failure::usage(format!("{one} is not an address: {e}{hint}"))
        })?);
    }
    Ok(out)
}

/// What the run asks the service for.
///
/// Passing selectors means fields, whatever the goal said, because a caller
/// who wrote a selector map wanted the fields and not the page.
pub fn need(goal: Goal, selectors: Option<&str>, at: Option<&str>) -> Run<Need> {
    if let Some(spec) = selectors {
        return fields(spec, at);
    }
    Ok(match goal {
        Goal::Text => Need::Text,
        Goal::Markdown => Need::Markdown,
        Goal::Html => Need::Html,
        Goal::Links => Need::Links,
        Goal::Metadata => Need::Metadata,
        Goal::Screenshot => Need::screenshot(),
        Goal::Raw => Need::Raw,
        Goal::Fields => {
            return Err(Failure::usage(
                "--goal fields needs --selectors, a JSON object of field name to selector.",
            ))
        }
    })
}

/// What the router is told the request wants, for the local `route` command.
pub fn declared(goal: Goal) -> DeclaredNeed {
    match goal {
        Goal::Text => DeclaredNeed::Text,
        Goal::Markdown => DeclaredNeed::Markdown,
        Goal::Html => DeclaredNeed::Html,
        Goal::Links => DeclaredNeed::Links,
        Goal::Metadata => DeclaredNeed::Metadata,
        Goal::Fields => DeclaredNeed::Fields,
        Goal::Screenshot => DeclaredNeed::Screenshot,
        Goal::Raw => DeclaredNeed::Raw,
    }
}

/// Read a selector map.
///
/// `{"price": ".price", "title": ["h1", ".product-title"]}`. Several
/// expressions on one name are tried in order, so the precise one goes first.
pub fn fields(spec: &str, at: Option<&str>) -> Run<Need> {
    let text = source(spec)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| Failure::usage(format!("the selector map in {spec} is not JSON: {e}")))?;
    fields_from_value(&value, at)
}

/// The same map, already parsed.
///
/// A plan file carries its selectors inline, and writing them back out to a
/// file so they could be read again would be a round trip for nothing.
pub fn fields_from_value(value: &Value, at: Option<&str>) -> Run<Need> {
    let Value::Object(map) = value.clone() else {
        return Err(Failure::usage(
            "a selector map has to be a JSON object of field name to selector.",
        ));
    };
    if map.is_empty() {
        return Err(Failure::usage(
            "the selector map is empty, so the request would ask for nothing.",
        ));
    }

    let mut pairs: Vec<(String, String)> = Vec::new();
    for (name, selector) in map {
        match selector {
            Value::String(one) => pairs.push((name, one)),
            Value::Array(many) => {
                for one in many {
                    match one {
                        Value::String(one) => pairs.push((name.clone(), one)),
                        other => {
                            return Err(Failure::usage(format!(
                                "the selectors for {name} have to be strings, and one is {other}."
                            )))
                        }
                    }
                }
            }
            other => {
                return Err(Failure::usage(format!(
                    "the selector for {name} has to be a string or a list of strings, and it is {other}."
                )))
            }
        }
    }

    let mut spec = spider_cloud_agent::thrift::FieldSpec::new(Vec::<(String, String)>::new());
    for (name, selector) in pairs {
        spec.push(name, &selector);
    }
    if let Some(path) = at {
        spec = spec.at(path);
    }
    Ok(Need::Fields(spec))
}

/// The caps one operation runs under.
pub fn budget(global: &Global) -> Budget {
    let mut budget = Budget::default();
    if let Some(credits) = global.budget {
        budget = budget.with_credits(Credits::new(credits));
    }
    if let Some(credits) = global.max_per_page {
        budget = budget.with_per_page_credits(Credits::new(credits));
    }
    if let Some(attempts) = global.attempts {
        budget = budget.with_attempts(attempts);
    }
    if let Some(seconds) = global.wall {
        budget = budget.with_wall(Duration::from_secs(seconds));
    }
    budget
}

/// A client carrying the caps, ready to be asked for an operation.
pub fn client(global: &Global) -> Run<Spider> {
    client_with_credits(global, global.budget)
}

/// Build the shared run cap, including a cap supplied by a plan.
pub fn client_with_credits(global: &Global, credits: Option<f64>) -> Run<Spider> {
    let mut builder = Spider::builder().budget(budget(global));
    if let Some(cap) = credits {
        builder = builder.run_budget(spider_cloud_agent::RunBudget::new(Credits::new(cap)));
    }
    builder.build().map_err(Failure::from)
}

/// The fetch mode the caller fixed, when they fixed one.
pub fn mode(global: &Global) -> Option<RequestMode> {
    global.mode.map(|mode| match mode {
        Mode::Http => RequestMode::Http,
        Mode::Smart => RequestMode::Smart,
        Mode::Browser => RequestMode::Browser,
    })
}

/// The pool the caller fixed, when they fixed one.
pub fn pool(global: &Global) -> Option<ProxyPool> {
    global.proxy.map(|pool| match pool {
        Pool::Isp => ProxyPool::Isp,
        Pool::Residential => ProxyPool::Residential,
    })
}

/// The country the caller asked to appear to be in.
pub fn country(global: &Global) -> Run<Option<Country>> {
    match &global.country {
        None => Ok(None),
        Some(code) => match Country::new(code) {
            Some(country) => Ok(Some(country)),
            None => Err(Failure::usage(format!(
                "{code} is not a two letter country code."
            ))),
        },
    }
}

/// Apply the settings the caller fixed, and nothing else.
///
/// One macro rather than one function per builder, because the curated surface
/// is the same shape on every operation without being a trait.
macro_rules! pin {
    ($builder:expr, $global:expr) => {{
        let mut builder = $builder;
        if let Some(mode) = $crate::setup::mode($global) {
            builder = builder.mode(mode);
        }
        if let Some(pool) = $crate::setup::pool($global) {
            builder = builder.proxy(pool);
        }
        if let Some(country) = $crate::setup::country($global)? {
            builder = builder.country(country);
        }
        if let Some(seconds) = $global.timeout {
            builder = builder.timeout(std::time::Duration::from_secs(seconds));
        }
        if $global.session {
            builder = builder.session(true);
        }
        if let Some(tokens) = $global.max_tokens {
            builder = builder.max_tokens(tokens);
        }
        builder
    }};
    // The operations that fetch a page and can hand its links back with it.
    // Separate from the common form because the links operation already
    // returns them and the transform operation fetches nothing.
    ($builder:expr, $global:expr, links) => {{
        let mut builder = $crate::setup::pin!($builder, $global);
        if $global.with_links {
            builder = builder.page_links(true);
        }
        builder
    }};
}

pub(crate) use pin;

/// The shape to write, from the flags and the command's own default.
pub fn format(global: &Global, default: Format) -> Format {
    if global.json {
        return Format::Json;
    }
    if global.ndjson {
        return Format::Ndjson;
    }
    global.format.unwrap_or(default)
}

/// Open the destination the caller named.
pub fn emitter(global: &Global, default: Format) -> Run<Emitter> {
    Emitter::open(
        format(global, default),
        global.output.as_deref(),
        global.output_dir.as_deref(),
        FileRules {
            force: global.force,
            mkdir: global.mkdir,
            append: global.append,
        },
    )
}

/// Refuse to write bytes into a terminal.
///
/// A picture printed to a tty is a scrambled prompt and nothing else, so this
/// is a usage failure rather than a surprise.
pub fn refuse_bytes_on_a_terminal(global: &Global) -> Run<()> {
    if global.output.is_none() && global.output_dir.is_none() && std::io::stdout().is_terminal() {
        return Err(Failure::new(
            Code::Usage,
            "a picture needs somewhere to go. Pass --output or --output-dir, or redirect stdout.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn comments_and_blank_lines_are_not_addresses() {
        let read = lines("# a note\n\nhttps://example.com\n  https://example.com/a  \n");
        assert_eq!(
            read,
            vec![
                "https://example.com".to_string(),
                "https://example.com/a".to_string()
            ]
        );
    }

    #[test]
    fn a_byte_order_mark_is_not_part_of_the_text() {
        assert_eq!(
            without_bom("\u{feff}https://example.com\n".to_string()),
            "https://example.com\n"
        );
        assert_eq!(
            without_bom("https://example.com\n".to_string()),
            "https://example.com\n"
        );
        // Only the one at the start. One inside the text is the file's own.
        assert_eq!(without_bom("a\u{feff}b".to_string()), "a\u{feff}b");
    }

    #[test]
    fn an_address_that_is_not_one_names_itself() {
        let failed = parse_all(&["not a url".to_string()]).expect_err("a usage failure");
        assert_eq!(failed.code, Code::Usage);
        assert!(failed.message.contains("not a url"), "{failed}");
    }

    #[test]
    fn a_goal_of_fields_without_selectors_is_a_usage_failure() {
        let failed = need(Goal::Fields, None, None).expect_err("a usage failure");
        assert_eq!(failed.code, Code::Usage);
    }

    #[test]
    fn every_other_goal_needs_nothing_else() {
        for goal in [
            Goal::Text,
            Goal::Markdown,
            Goal::Html,
            Goal::Links,
            Goal::Metadata,
            Goal::Screenshot,
            Goal::Raw,
        ] {
            assert!(need(goal, None, None).is_ok(), "{goal:?}");
        }
    }
}
