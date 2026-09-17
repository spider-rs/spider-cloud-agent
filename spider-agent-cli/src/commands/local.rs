//! The commands that make no call.

use clap::CommandFactory;
use serde_json::json;

use spider_cloud_agent::{HeuristicRouter, RouteInput, Router};

use crate::cli::{Format, Global, RouteArgs};
use crate::emit::Item;
use crate::exit::{Code, Run};
use crate::progress::Log;
use crate::records;
use crate::setup;

/// What transport would be chosen for an address.
///
/// The same decision the fetch commands make before they send anything, on its
/// own. No key is read and no call goes out, so this is the command to run
/// when you want to know what a run would do without paying for it.
pub fn route(global: &Global, args: &RouteArgs, log: Log) -> Run<Code> {
    let urls = setup::addresses(&args.targets)?;
    let router = HeuristicRouter::new();
    let need = setup::declared(args.goal);
    let mut emitter = setup::emitter(global, Format::Text)?;
    for url in &urls {
        let decision = router.route(&RouteInput::new(url, need));
        emitter.write(records::route(url, &decision))?;
    }
    emitter.finish()?;
    log.note("decided locally, nothing was sent");
    Ok(Code::Ok)
}

/// The command tree and the record schema, as JSON.
///
/// Here so a calling agent can learn the surface without parsing help text.
/// The parser supplies the command tree; the record contract is kept here.
pub fn schema(global: &Global) -> Run<Code> {
    let mut emitter = setup::emitter(global, Format::Json)?;
    emitter.write_sole(Item::structured(document()))?;
    emitter.finish()?;
    Ok(Code::Ok)
}

/// The document `schema` prints.
fn document() -> serde_json::Value {
    let mut command = crate::cli::Cli::command();
    command.build();
    let tree = command_tree(&command);
    let codes: serde_json::Map<String, serde_json::Value> = Code::ALL
        .iter()
        .map(|code| (code.number().to_string(), json!(code.label())))
        .collect();
    json!({
        "command_tree": tree,
        "commands": tree.get("commands"),
        "plan": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "goal": {"type": "string", "enum": goal_names()},
                "urls": {"type": "array", "items": {"type": "string", "format": "uri"}},
                "expand": {"type": "integer", "minimum": 0, "maximum": usize::MAX},
                "max_pages": {"type": "integer", "minimum": 0, "maximum": usize::MAX},
                "budget": {"type": "number", "minimum": 0},
                "selectors": {"type": "object", "minProperties": 1, "additionalProperties": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]}}
            },
            "notes": "All keys are optional. Explicit flags override plan values; URLs accumulate. Selectors imply fields. Goal defaults to markdown and expand to zero. A leading BOM is accepted."
        },
        "tool": "spider-agent",
        "version": env!("CARGO_PKG_VERSION"),
        "command_notes": {
            "scrape": "one page or a list of them, and what the bare form runs. Default format text.",
            "fetch": "one path under the config the service holds for it. Takes a host and a path, not an address, and is not scrape. Default format text.",
            "crawl": "a site, following its links. Default format ndjson.",
            "extract": "named fields, no page bytes on the wire. Default format ndjson.",
            "links": "the links on a page. Default format text.",
            "search": "a query. Default format ndjson.",
            "screenshot": "a picture. Needs --output or --output-dir.",
            "transform": "convert markup you already hold. Nothing is fetched.",
            "run": "work a goal over addresses until it is met or the budget stops it.",
            "credits": "the balance.",
            "logs": "the record of past crawls. Default format ndjson.",
            "sites": "the sites configured on the account, one row each. Default format ndjson.",
            "keys": "the API keys on the account, as metadata. No key, secret or hash is returned. Default format ndjson.",
            "profile": "the account itself: plan limits, totals and billing caps. One row. Default format ndjson.",
            "login": "sign in through a browser and store the key.",
            "router": "store, show or clear a provider fallback (the router parameter and provider_options) in ~/.spider/router.json. Every command that fetches pages sends it unless the request names a router; --no-router or SPIDER_AGENT_NO_ROUTER skips it for one run. Keys come from stdin or SPIDER_ROUTER_TOKEN, never an argument. show writes a router record with every key replaced. Local, no call.",
            "optimize": "store how the parameter optimizer runs (mode and weights file) in ~/.spider/optimize.json. The optimizer scores the valid edits to a request after the router has picked the mode, the pool and the wait, and writes at most one set on the first attempt: shadow scores and changes nothing, apply writes the pick onto the fields the caller left unset, off sends the request as it is. It never edits a field the caller set, and a request that fixes the mode, the pool or the country is not passed to it. Without weights the scorer abstains, so apply edits nothing. Every command that fetches pages reads the file; --optimize and --optimize-model override it for one run, and --no-optimize or SPIDER_AGENT_NO_OPTIMIZE leaves it out. show writes an optimize record. Local, no call.",
            "route": "what transport would be chosen. Local, no call, no spend.",
            "schema": "this document.",
            "update": "install the newest release now. Reports on stderr. Exit 0 installed or current, 1 refused or failed, 2 turned off, 6 unreachable, 7 not this tool's to replace."
        },
        "goals": ["text", "markdown", "html", "links", "metadata", "fields", "screenshot", "raw"],
        "formats": {
            "text": "the content alone, no wrapper",
            "json": "one document, {items: [...], report: {...}}, buffered until the run ends",
            "ndjson": "one object per line, flushed as each arrives"
        },
        "records": {
            "page": {
                "type": "page",
                "url": "string",
                "status": "integer, the status the site returned",
                "error": "string, present when the service attached a note to a page it still served",
                "content": "text|markdown|html|xml|bytes|screenshot|fields|multi|empty|other",
                "body": "string or null. Null when the content is bytes or fields. Fields asked for beside a page keep the page here and the fields under fields.",
                "fields": "object, present when the request asked for named fields",
                "metadata": "object, present when the request asked for metadata",
                "links": "array of strings, present when the request asked for links",
                "headers": "object, present when the request asked for response headers",
                "cookies": "object, present when the request asked for response cookies",
                "json_data": "object, present when the request asked for the page's structured data",
                "request_map": "object, present when the request asked for the page's own requests",
                "response_map": "object, present when the request asked for the page's own responses",
                "trace": "array, present when the request asked for the fetch trace and the account has it",
                "bytes": "integer",
                "duration_ms": "integer or null, the service's time on this page. Null on a converted document.",
                "call_elapsed_ms": "integer, the whole call, shared by every page in one reply",
                "cost_credits": "number",
                "vendor": "object, present when an outside vendor served the page: provider, route, vendor_cost_credits, billed_credits, byok, attempts. billed_credits is inside cost_credits, not on top of it."
            },
            "failed": {
                "type": "failed",
                "url": "string",
                "status": "integer, the status the site returned",
                "error": "string or null",
                "hint": "try_browser|try_residential_proxy|try_different_country|needs_session|permanent|slow_down|unknown",
                "billed": "boolean",
                "note": "the site's answer travels with the refusal, so every field below is the page record's, with the same presence rules",
                "content": "text|markdown|html|xml|bytes|screenshot|fields|multi|empty|other",
                "body": "string or null",
                "fields": "object, when asked for",
                "metadata": "object, when asked for",
                "links": "array of strings, when asked for",
                "headers": "object, when asked for",
                "cookies": "object, when asked for",
                "json_data": "object, when asked for",
                "request_map": "object, when asked for",
                "response_map": "object, when asked for",
                "trace": "array, when asked for",
                "bytes": "integer",
                "duration_ms": "integer or null",
                "call_elapsed_ms": "integer",
                "cost_credits": "number",
                "vendor": "object, when an outside vendor served the page"
            },
            "link": { "type": "link", "url": "string", "from": "the page it was found on" },
            "result": {
                "type": "result",
                "url": "string",
                "title": "string or null",
                "description": "string or null"
            },
            "row": { "type": "row", "data": "object, the columns belong to the service" },
            "route": {
                "type": "route",
                "url": "string",
                "mode": "http|smart|browser",
                "proxy": "isp|residential",
                "wait_ms": "integer",
                "country": "string or null",
                "start_rung": "integer, where escalation would begin",
                "confidence": "number from zero to one",
                "source": "heuristic|model|caller|memory|explore|unknown"
            },
            "credits": { "type": "credits", "credits": "number", "usd": "number" },
            "key": { "type": "key", "key": "string, written by login --print and nothing else" },
            "router": {
                "type": "router",
                "stored": "boolean. When false no other field is present.",
                "mode": "fallback|first|off or null",
                "provider": "string or null",
                "funding": "any|own or null",
                "token": "<redacted> or null",
                "credentials": "object of credential name to <redacted>, or null",
                "provider_options": "object of provider name to an object of setting name to <redacted>, or null"
            },
            "optimize": {
                "type": "optimize",
                "stored": "boolean. When false no other field is present.",
                "mode": "off|shadow|apply",
                "model": "string or null, the path to the weights file"
            },
            "error": {
                "type": "error",
                "url": "string or null",
                "code": "the exit code name",
                "exit": "integer, the code the process would return for this alone",
                "message": "string"
            },
            "report": {
                "type": "report",
                "note": "always the last record",
                "targets": "integer",
                "served": "integer",
                "refused": "integer",
                "attempts": "integer, calls made including escalations",
                "cost_credits": "number",
                "elapsed_ms": "integer",
                "wire_bytes": "integer, what the service sent",
                "returned_bytes": "integer, what reached the caller",
                "approx_tokens_in": "integer",
                "approx_tokens_out": "integer",
                "stopped": "budget|pages|time|null. budget and time leave with code 4, pages with 0."
            }
        },
        "exit_codes": codes,
        "notes": [
            "results go to stdout, diagnostics to stderr, and no escape codes are written",
            "a reader that closes stdout early, such as head, ends the run quietly with code 0 and no further page is fetched",
            "addresses can be piped in on stdin, one per line",
            crate::cli::KEY_RESOLUTION,
            "There is no flag for the key, because an argument is readable in the process list.",
            "Environment: SPIDER_API_KEY is the first environment source for the API key; empty values after trimming are skipped. SPIDER_CLOUD_API_KEY is the fallback when SPIDER_API_KEY is empty or unset. SPIDER_API_URL overrides the API base (normally https://api.spider.cloud) for every key-bearing API request; the API base must use https. SPIDER_MCP_SERVER overrides the sign-in discovery server (normally https://mcp.spider.cloud/mcp) when OAuth is enabled. Set either server override only to a server you trust: one receives API requests with your key, and the other directs sign-in.",
            format!(
                "Provider fallback: {} set to any value, or --no-router, skips the stored router for one run. {} is read by router set as the provider token.",
                crate::setup::NO_ROUTER_ENV,
                crate::commands::router::TOKEN_ENV
            ),
            format!(
                "Optimizer: {} set to any value, or --no-optimize, leaves the optimizer out of one run. --optimize off|shadow|apply and --optimize-model FILE override the settings stored in ~/{}.",
                crate::optimize::NO_OPTIMIZE_ENV,
                crate::optimize::OPTIMIZE_PATH
            )
        ]
    })
}

fn goal_names() -> Vec<String> {
    use clap::ValueEnum;
    crate::cli::Goal::value_variants()
        .iter()
        .filter_map(|goal| goal.to_possible_value().map(|v| v.get_name().to_owned()))
        .collect()
}

/// Walk the built parser so inherited global flags and inferred arity are present.
fn command_tree(command: &clap::Command) -> serde_json::Value {
    let arguments: serde_json::Map<String, serde_json::Value> = command
        .get_arguments()
        .map(|arg| (arg.get_id().to_string(), argument_schema(command, arg)))
        .collect();
    let commands: serde_json::Map<String, serde_json::Value> = command
        .get_subcommands()
        .map(|sub| (sub.get_name().to_string(), command_tree(sub)))
        .collect();
    json!({"name": command.get_name(), "arguments": arguments, "commands": commands})
}

fn argument_schema(command: &clap::Command, arg: &clap::Arg) -> serde_json::Value {
    let arity = arg.get_num_args().unwrap_or_default();
    let maximum = (arity.max_values() != usize::MAX).then_some(arity.max_values());
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .map(|value| value.get_name().to_string())
        .collect();
    let defaults: Vec<_> = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy())
        .collect();
    let conflicts: Vec<&str> = command
        .get_arguments()
        .filter(|other| {
            command
                .get_arg_conflicts_with(arg)
                .iter()
                .any(|a| a.get_id() == other.get_id())
                || command
                    .get_arg_conflicts_with(other)
                    .iter()
                    .any(|a| a.get_id() == arg.get_id())
        })
        .map(|a| a.get_id().as_str())
        .collect();
    json!({
        "long": arg.get_long(),
        "short": arg.get_short(),
        "position": arg.get_index(),
        "arity": {"min": arity.min_values(), "max": maximum},
        "value_type": value_type(arg),
        "default": defaults,
        "possible_values": possible,
        "required": arg.is_required_set(),
        "global": arg.is_global_set(),
        "action": format!("{:?}", arg.get_action()),
        "conflicts": conflicts,
        "requires": requirements(command, arg),
    })
}

fn value_type(arg: &clap::Arg) -> &'static str {
    use std::any::TypeId;

    let ty = arg.get_value_parser().type_id();
    if !arg.get_possible_values().is_empty() {
        "enum"
    } else if ty == TypeId::of::<bool>() {
        "boolean"
    } else if ty == TypeId::of::<f64>() {
        "number"
    } else if [
        TypeId::of::<usize>(),
        TypeId::of::<u64>(),
        TypeId::of::<u32>(),
        TypeId::of::<u8>(),
    ]
    .iter()
    .any(|candidate| ty == *candidate)
    {
        "integer"
    } else {
        "string"
    }
}

/// Clap has no public requires getter. Probe its validator with required flags
/// cleared and string values, then map its missing-argument context back to IDs.
/// This reflects the unconditional requirements used by this CLI.
fn requirements(command: &clap::Command, arg: &clap::Arg) -> Vec<String> {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    if matches!(
        arg.get_action(),
        clap::ArgAction::Help | clap::ArgAction::Version
    ) {
        return Vec::new();
    }
    let mut probe = clap::Command::new("probe")
        .version("probe")
        .disable_help_flag(true)
        .disable_version_flag(true);
    for candidate in command.get_arguments() {
        let mut candidate = candidate.clone().required(false).global(false);
        if candidate.get_action().takes_values() {
            candidate = candidate.value_parser(clap::builder::ValueParser::string());
        }
        probe = probe.arg(candidate);
    }
    let mut argv = vec!["probe".to_string()];
    if let Some(long) = arg.get_long() {
        argv.push(format!("--{long}"));
    } else if let Some(short) = arg.get_short() {
        argv.push(format!("-{short}"));
    }
    for _ in 0..arg.get_num_args().unwrap_or_default().min_values() {
        argv.push("value".to_string());
    }
    if let Err(error) = probe.try_get_matches_from(argv) {
        if error.kind() == ErrorKind::MissingRequiredArgument {
            if let Some(ContextValue::Strings(missing)) = error.get(ContextKind::InvalidArg) {
                return command
                    .get_arguments()
                    .filter(|a| missing.contains(&a.to_string()))
                    .map(|a| a.get_id().to_string())
                    .collect();
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;

    #[test]
    fn the_schema_names_every_record_the_tool_writes() {
        let document = document();
        let records = document["records"]
            .as_object()
            .expect("records is an object");
        for name in [
            "page", "failed", "link", "result", "row", "route", "router", "optimize", "credits",
            "error", "report",
        ] {
            assert!(records.contains_key(name), "{name} is missing");
        }
    }

    /// Every command the binary answers to is in the document, because a
    /// caller reads this instead of the help text.
    #[test]
    fn the_schema_names_every_command() {
        let document = document();
        let commands = document["commands"]
            .as_object()
            .expect("commands is an object");
        for name in [
            "scrape",
            "fetch",
            "crawl",
            "extract",
            "links",
            "search",
            "screenshot",
            "transform",
            "run",
            "credits",
            "logs",
            "sites",
            "keys",
            "profile",
            "login",
            "router",
            "optimize",
            "route",
            "schema",
            "update",
        ] {
            assert!(commands.contains_key(name), "{name} is missing");
        }
        assert_eq!(
            commands.len(),
            20,
            "a command was added or removed without a line here"
        );
    }

    /// `table` took any name and read it, which made the tool a way to ask for
    /// whatever sits under `/data`. The named reads replaced it in 0.3.0, and
    /// a caller reads this document rather than the help text, so its absence
    /// belongs here too.
    #[test]
    fn the_schema_no_longer_names_a_command_that_reads_any_table() {
        let document = document();
        let commands = document["commands"]
            .as_object()
            .expect("commands is an object");
        assert!(
            !commands.contains_key("table"),
            "table is back in the schema"
        );
    }

    #[test]
    fn the_schema_names_every_exit_code() {
        let document = document();
        let expected: serde_json::Map<String, serde_json::Value> = Code::ALL
            .iter()
            .map(|c| (c.number().to_string(), json!(c.label())))
            .collect();
        assert_eq!(document["exit_codes"], json!(expected));
    }

    #[test]
    fn invocations_built_from_the_tree_round_trip_through_clap() {
        use clap::Parser;
        let doc = document();
        let tree = &doc["commands"];
        for (command, id, value) in [
            ("transform", "input", "page.html"),
            ("run", "expand", "0"),
            ("fetch", "domain", "example.com"),
        ] {
            let arg = &tree[command]["arguments"][id];
            let mut argv = vec!["spider-agent".to_string(), command.to_string()];
            if let Some(long) = arg["long"].as_str() {
                argv.push(format!("--{long}"));
            }
            assert_eq!(arg["arity"]["min"], 1);
            argv.push(value.to_string());
            assert!(crate::cli::Cli::try_parse_from(argv).is_ok());
        }
        assert_eq!(tree["transform"]["arguments"]["input"]["required"], true);
        assert!(crate::cli::Cli::try_parse_from(["spider-agent", "transform"]).is_err());
        let args = &tree["run"]["arguments"];
        assert!(args["json"]["conflicts"]
            .as_array()
            .unwrap()
            .contains(&json!("ndjson")));
        assert_eq!(args["append"]["requires"], json!(["output"]));
        let mut argv = vec![
            "spider-agent".to_string(),
            "run".to_string(),
            "--append".to_string(),
        ];
        assert!(crate::cli::Cli::try_parse_from(&argv).is_err());
        for required in args["append"]["requires"].as_array().unwrap() {
            let arg = &args[required.as_str().unwrap()];
            argv.push(format!("--{}", arg["long"].as_str().unwrap()));
            argv.push("pages.ndjson".to_string());
        }
        assert!(crate::cli::Cli::try_parse_from(argv).is_ok());
        assert!(
            crate::cli::Cli::try_parse_from(["spider-agent", "run", "--json", "--ndjson"]).is_err()
        );
    }
}
