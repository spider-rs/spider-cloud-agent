//! The commands that make no call.

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
/// It is a hand written constant rather than a reflection of the clap tree,
/// because the point is the output contract and clap does not know that.
pub fn schema(global: &Global) -> Run<Code> {
    let mut emitter = setup::emitter(global, Format::Json)?;
    emitter.write_sole(Item::structured(document()))?;
    emitter.finish()?;
    Ok(Code::Ok)
}

/// The document `schema` prints.
fn document() -> serde_json::Value {
    json!({
        "tool": "spider-agent",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": {
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
            "route": "what transport would be chosen. Local, no call, no spend.",
            "schema": "this document."
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
                "content": "text|markdown|html|xml|bytes|screenshot|fields|multi|empty|other",
                "body": "string or null. Null when the content is bytes or fields.",
                "fields": "object, present when the request asked for named fields",
                "metadata": "object, present when the request asked for metadata",
                "links": "array of strings, present when the request asked for links",
                "bytes": "integer",
                "duration_ms": "integer",
                "cost_credits": "number"
            },
            "failed": {
                "type": "failed",
                "url": "string",
                "status": "integer, the status the site returned",
                "error": "string or null",
                "hint": "try_browser|try_residential_proxy|try_different_country|needs_session|permanent|slow_down|unknown",
                "billed": "boolean",
                "cost_credits": "number"
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
        "exit_codes": {
            "0": "done",
            "1": "failed, none of the others describes it",
            "2": "usage, or an input that could not be read",
            "3": "auth: no key, a refused key, or a balance of zero",
            "4": "budget: a cap stopped the run",
            "5": "the site refused every attempt",
            "6": "transport: the call never reached the service, or it failed",
            "7": "output: a destination could not be written"
        },
        "notes": [
            "results go to stdout, diagnostics to stderr, and no escape codes are written",
            "a reader that closes stdout early, such as head, ends the run quietly with code 0 and no further page is fetched",
            "addresses can be piped in on stdin, one per line",
            "the key is read from SPIDER_API_KEY, SPIDER_CLOUD_API_KEY, then ~/.spider/credentials, and there is no flag for it"
        ]
    })
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
            "page", "failed", "link", "result", "row", "route", "credits", "error", "report",
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
            "route",
            "schema",
        ] {
            assert!(commands.contains_key(name), "{name} is missing");
        }
        assert_eq!(
            commands.len(),
            17,
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
        let codes = document["exit_codes"]
            .as_object()
            .expect("exit_codes is an object");
        assert_eq!(codes.len(), 8, "a code was added without a line here");
    }
}
