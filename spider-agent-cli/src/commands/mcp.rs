//! `spider-agent mcp`: the commands, served as tools over the Model Context
//! Protocol on stdin and stdout.
//!
//! An agent client starts this process once and calls it many times. That is
//! the reason it exists beside the plain commands: a tool call here pays for
//! no process start, and every call after the first reuses the connection the
//! first one opened, because all of them share one HTTP client.
//!
//! A tool call is a command line. The arguments are checked, turned into the
//! same words a person would type, parsed by the same parser and run by the
//! same code, with `--json` set. So a tool cannot drift from its command: what
//! `spider-agent scrape --json` writes is what `spider_scrape` answers with,
//! closing report included.
//!
//! The wire is JSON-RPC 2.0, one message per line. Stdout carries protocol
//! messages and nothing else. A command inside a call writes to that call's
//! buffer instead, through [`crate::session`]. Calls that arrive together run
//! together on the one runtime thread, and each answer leaves as its call
//! ends, so a slow crawl does not hold up a scrape sent after it.

use std::collections::HashMap;
use std::future::Future;
use std::io::{BufRead, Read, Write};
use std::pin::Pin;

use clap::Parser;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use spider_cloud_agent::Transport;

use crate::cli::Cli;
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;
use crate::session;

/// The protocol revisions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// How many pages `spider_crawl` reads when the call names no limit. A crawl
/// with no limit reads the whole site, which is not a default to hand a model.
const DEFAULT_CRAWL_LIMIT: u64 = 10;

/// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
/// In the range JSON-RPC leaves to the server.
const SERVER_BUSY: i64 = -32000;

/// The goals a tool may ask for. `fields` is reached by passing selectors and
/// `screenshot` is bytes, which a text answer cannot carry.
const GOALS: [&str; 6] = ["markdown", "text", "html", "links", "metadata", "raw"];

/// The shapes `spider_transform` converts into.
const SHAPES: [&str; 3] = ["markdown", "text", "html"];

/// How a page may be fetched.
const MODES: [&str; 3] = ["http", "smart", "browser"];

/// The longest line the server reads. A call that carries a whole page of
/// markup to convert fits many times over, and a client that never sends a
/// newline cannot grow the buffer past it.
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

/// How many calls may be open at once. Past it a call is refused at once
/// rather than queued, so the client learns to wait instead of piling up work.
const MAX_OPEN_CALLS: usize = 64;

/// The most pages one `spider_crawl` may ask for. A larger crawl belongs on
/// the command line, where the pages stream to a file as they arrive instead
/// of being held for one answer.
const MAX_CRAWL_LIMIT: u64 = 1000;

/// Serve until the client closes stdin and every open call has answered.
///
/// Calls still open when stdin closes are finished and answered, so a session
/// piped in from a file gets every answer. A client that wants a call stopped
/// sends `notifications/cancelled`.
pub async fn serve(log: Log) -> Run<Code> {
    let http = Transport::default_client().map_err(Failure::from)?;
    let mut lines = read_stdin()?;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let mut open = Open::default();
            let mut reading = true;
            while reading || !open.tasks.is_empty() {
                tokio::select! {
                    line = lines.recv(), if reading => match line {
                        None => reading = false,
                        Some(Line::TooLong) => write_line(&error(
                            &Value::Null,
                            PARSE_ERROR,
                            "the line is longer than this server reads",
                        ))?,
                        Some(Line::Text(line)) => match handle(&line) {
                            Step::Silent => {}
                            Step::Reply(text) => write_line(&text)?,
                            Step::Cancel(id) => open.cancel(&id),
                            Step::Call(call) if open.tasks.len() >= MAX_OPEN_CALLS => {
                                write_line(&error(
                                    &call.id,
                                    SERVER_BUSY,
                                    "too many calls are open. Wait for one to answer.",
                                ))?;
                            }
                            Step::Call(call) => {
                                let id = call.id.clone();
                                let work = answer(*call, http.clone(), log);
                                open.track(Some(id), work);
                            }
                            Step::Batch(steps) => {
                                let work = batch(steps, http.clone(), log);
                                open.track(None, work);
                            }
                        },
                    },
                    Some(joined) = open.tasks.join_next_with_id(), if !open.tasks.is_empty() => {
                        match joined {
                            Ok((task, text)) => {
                                open.forget(task);
                                if !text.is_empty() {
                                    write_line(&text)?;
                                }
                            }
                            Err(stopped) => {
                                let id = open.forget(stopped.id());
                                // A call the client cancelled gets no answer.
                                // One that stopped any other way still owes
                                // one, or the client waits on it for ever.
                                if !stopped.is_cancelled() {
                                    write_line(&error(
                                        &id.unwrap_or(Value::Null),
                                        INTERNAL_ERROR,
                                        "the call stopped before it answered",
                                    ))?;
                                }
                            }
                        }
                    }
                    // Both arms off means stdin is closed and nothing is
                    // open, which the loop condition is about to see.
                    else => break,
                }
            }
            Ok(Code::Ok)
        })
        .await
}

/// The calls that have not answered yet.
#[derive(Default)]
struct Open {
    tasks: tokio::task::JoinSet<String>,
    /// The request each task answers, so one that stops early still gets an
    /// answer. A batch has no single request and is absent.
    requests: HashMap<tokio::task::Id, Value>,
    /// The way to stop a request, by its id as text.
    stops: HashMap<String, tokio::task::AbortHandle>,
}

impl Open {
    fn track(&mut self, id: Option<Value>, work: impl Future<Output = String> + 'static) {
        let stop = self.tasks.spawn_local(work);
        if let Some(id) = id {
            self.requests.insert(stop.id(), id.clone());
            self.stops.insert(id.to_string(), stop);
        }
    }

    fn cancel(&mut self, id: &Value) {
        if let Some(stop) = self.stops.remove(&id.to_string()) {
            stop.abort();
        }
    }

    fn forget(&mut self, task: tokio::task::Id) -> Option<Value> {
        let id = self.requests.remove(&task)?;
        self.stops.remove(&id.to_string());
        Some(id)
    }
}

/// Answer a batch: every message in order, and one array back.
///
/// The calls in a batch run one after another. A client that wants them side
/// by side sends them as separate lines. A batch of nothing but notifications
/// has no answer, which is the empty text the caller skips.
async fn batch(steps: Vec<Step>, http: reqwest::Client, log: Log) -> String {
    let mut answers = Vec::new();
    for step in steps {
        match step {
            Step::Silent | Step::Cancel(_) | Step::Batch(_) => {}
            Step::Reply(text) => answers.push(text),
            Step::Call(call) => answers.push(answer(*call, http.clone(), log).await),
        }
    }
    if answers.is_empty() {
        String::new()
    } else {
        format!("[{}]", answers.join(","))
    }
}

/// One line from stdin.
enum Line {
    Text(String),
    /// A line past [`MAX_LINE_BYTES`], already skipped.
    TooLong,
}

/// Read stdin on its own thread and hand each line over a channel.
///
/// Stdin has no portable async read, and a blocking read on the runtime thread
/// would stop every open call. A plain thread that does nothing but read is
/// the whole cost, and it ends when the client closes the stream.
fn read_stdin() -> Run<mpsc::UnboundedReceiver<Line>> {
    let (send, receive) = mpsc::unbounded_channel::<Line>();
    std::thread::Builder::new()
        .name("mcp-stdin".to_string())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                let read = (&mut stdin)
                    .take(MAX_LINE_BYTES)
                    .read_until(b'\n', &mut bytes);
                let line = match read {
                    Ok(0) | Err(_) => break,
                    Ok(_) if bytes.len() as u64 >= MAX_LINE_BYTES && !bytes.ends_with(b"\n") => {
                        if !skip_line(&mut stdin) {
                            break;
                        }
                        Line::TooLong
                    }
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&bytes);
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        Line::Text(line.to_string())
                    }
                };
                if send.send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| Failure::new(Code::Failed, format!("could not start reading stdin: {e}")))?;
    Ok(receive)
}

/// Read and drop the rest of a line that ran past the cap, a piece at a time.
/// False when the stream ended or failed first.
fn skip_line(stdin: &mut impl BufRead) -> bool {
    let mut piece = Vec::new();
    loop {
        piece.clear();
        match stdin.take(64 * 1024).read_until(b'\n', &mut piece) {
            Ok(0) | Err(_) => return false,
            Ok(_) if piece.ends_with(b"\n") => return true,
            Ok(_) => {}
        }
    }
}

/// One protocol message on stdout, flushed, so the client has it now.
fn write_line(text: &str) -> Run<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// What one incoming line leads to.
enum Step {
    /// A notification, or a reply to something this server never asked.
    Silent,
    /// An answer that needs no work.
    Reply(String),
    /// A tool call to run.
    Call(Box<Call>),
    /// The client no longer wants the answer to this request.
    Cancel(Value),
    /// Several messages sent as one array, answered as one array.
    Batch(Vec<Step>),
}

/// A tool call, checked and parsed, ready to run.
struct Call {
    id: Value,
    parsed: Cli,
    input: Option<String>,
}

/// Read one line and decide what it leads to.
fn handle(line: &str) -> Step {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(_) => return Step::Reply(error(&Value::Null, PARSE_ERROR, "the line is not JSON")),
    };
    match &message {
        Value::Array(messages) if messages.is_empty() => Step::Reply(error(
            &Value::Null,
            INVALID_REQUEST,
            "a batch holds at least one message",
        )),
        Value::Array(messages) => Step::Batch(messages.iter().map(one).collect()),
        other => one(other),
    }
}

/// Decide what one message leads to.
fn one(message: &Value) -> Step {
    let Some(object) = message.as_object() else {
        return Step::Reply(error(
            &Value::Null,
            INVALID_REQUEST,
            "a message is a JSON object",
        ));
    };
    let id = object.get("id").filter(|id| !id.is_null());
    let empty = Map::new();
    let params = object
        .get("params")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // An answer to something this server never asked is dropped. Anything
        // else that carries an id is a request nobody can read, and it still
        // gets an answer so the client is not left waiting.
        let is_answer = object.contains_key("result") || object.contains_key("error");
        return match id {
            Some(id) if !is_answer => {
                Step::Reply(error(id, INVALID_REQUEST, "a request names a method"))
            }
            _ => Step::Silent,
        };
    };
    // No id makes it a notification, and a notification is never answered.
    let Some(id) = id else {
        return match (method, params.get("requestId")) {
            ("notifications/cancelled", Some(request)) => Step::Cancel(request.clone()),
            _ => Step::Silent,
        };
    };
    match method {
        "initialize" => Step::Reply(result(id, initialize(params))),
        "ping" => Step::Reply(result(id, json!({}))),
        "tools/list" => Step::Reply(result(id, json!({ "tools": tools() }))),
        "tools/call" => match call(id, params) {
            Ok(call) => Step::Call(Box::new(call)),
            Err(reason) => Step::Reply(error(id, INVALID_PARAMS, &reason)),
        },
        other => Step::Reply(error(
            id,
            METHOD_NOT_FOUND,
            &format!("{other} is not a method this server has"),
        )),
    }
}

/// A successful answer.
fn result(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// A protocol error. A tool that ran and failed is not one of these: it
/// answers with a result whose `isError` is set, so the model reads the reason.
fn error(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        .to_string()
}

/// The answer to `initialize`.
///
/// The client names the revision it wants. It gets that one back when this
/// server speaks it and the newest one this server speaks when it does not,
/// which is what the protocol asks of a server.
fn initialize(params: &Map<String, Value>) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = VERSIONS
        .iter()
        .copied()
        .find(|known| Some(*known) == asked)
        .unwrap_or(VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "spider-agent", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Web data tools from Spider. Each tool answers with one JSON document: `items` holds the results and `report` says what the call cost and how it ended. Ask for the smallest goal that answers the question, since links and metadata cost less than a full page. Pass a budget, in credits, to cap what one call may spend."
    })
}

/// Turn a `tools/call` into a parsed command line.
fn call(id: &Value, params: &Map<String, Value>) -> Result<Call, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("a tool call needs a name")?;
    let empty = Map::new();
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(arguments)) => arguments,
        Some(_) => return Err("arguments is a JSON object".to_string()),
    };
    let Plan { words, input } = plan(name, arguments)?;
    let parsed = Cli::try_parse_from(&words).map_err(|refusal| {
        // clap's first line is the reason. The rest is usage text for a shell.
        let text = refusal.to_string();
        text.lines()
            .next()
            .unwrap_or("the arguments were refused")
            .trim_start_matches("error: ")
            .to_string()
    })?;
    Ok(Call {
        id: id.clone(),
        parsed,
        input,
    })
}

/// A command line, and the text that stands in for its stdin.
#[derive(Debug)]
struct Plan {
    words: Vec<String>,
    input: Option<String>,
}

/// The command line a tool call stands for.
///
/// Every value the caller supplied sits either behind a flag, where the parser
/// reads it as that flag's value, or after `--`, where it can only be a
/// positional. An address that starts with a dash is therefore an address that
/// fails to parse, and never a flag.
fn plan(name: &str, arguments: &Map<String, Value>) -> Result<Plan, String> {
    let mut words = vec!["spider-agent".to_string()];
    let mut tail = Vec::new();
    let mut input = None;
    let known: &[&str] = match name {
        "spider_scrape" => {
            words.push("scrape".to_string());
            goal(arguments, &mut words, &mut input)?;
            tail.push(required(arguments, "url")?);
            &["url", "goal", "selectors"]
        }
        "spider_crawl" => {
            words.push("crawl".to_string());
            goal(arguments, &mut words, &mut input)?;
            let limit = whole(arguments, "limit")?.unwrap_or(DEFAULT_CRAWL_LIMIT);
            // Zero is how the service spells "the whole site", which is the
            // one thing the default above exists to keep from a model.
            if !(1..=MAX_CRAWL_LIMIT).contains(&limit) {
                return Err(format!(
                    "limit is between 1 and {MAX_CRAWL_LIMIT}. Run a larger crawl from the command line."
                ));
            }
            words.extend(["--limit".to_string(), limit.to_string()]);
            if let Some(depth) = whole(arguments, "depth")? {
                words.extend(["--depth".to_string(), depth.to_string()]);
            }
            tail.push(required(arguments, "url")?);
            &["url", "goal", "selectors", "limit", "depth"]
        }
        "spider_search" => {
            words.push("search".to_string());
            if let Some(limit) = whole(arguments, "limit")? {
                words.extend(["--limit".to_string(), limit.to_string()]);
            }
            if flag(arguments, "fetch_pages")? {
                words.push("--fetch-pages".to_string());
            }
            tail.push(required(arguments, "query")?);
            &["query", "limit", "fetch_pages"]
        }
        "spider_links" => {
            words.push("links".to_string());
            tail.push(required(arguments, "url")?);
            &["url"]
        }
        "spider_transform" => {
            words.extend(["transform".to_string(), "--input".to_string(), "-".to_string()]);
            input = Some(required(arguments, "html")?);
            if let Some(shape) = choice(arguments, "to", &SHAPES)? {
                words.extend(["--to".to_string(), shape]);
            }
            if let Some(url) = text(arguments, "url")? {
                words.extend(["--url".to_string(), url]);
            }
            if flag(arguments, "readability")? {
                words.push("--readability".to_string());
            }
            &["html", "to", "url", "readability"]
        }
        "spider_get_credits" => {
            words.push("credits".to_string());
            &[]
        }
        other => return Err(format!("{other} is not a tool this server has")),
    };
    let fetches = !matches!(name, "spider_transform" | "spider_get_credits");
    if fetches {
        caps(arguments, &mut words)?;
    }
    for key in arguments.keys() {
        let shared = fetches && CAPS.contains(&key.as_str());
        if !shared && !known.contains(&key.as_str()) {
            return Err(format!("{name} takes no argument named {key}"));
        }
    }
    words.extend(["--json".to_string(), "--quiet".to_string()]);
    if !tail.is_empty() {
        words.push("--".to_string());
        words.extend(tail);
    }
    Ok(Plan { words, input })
}

/// The arguments every tool that fetches takes.
const CAPS: [&str; 4] = ["budget", "max_tokens", "mode", "country"];

/// Write the caps and transport choices a call named.
fn caps(arguments: &Map<String, Value>, words: &mut Vec<String>) -> Result<(), String> {
    if let Some(budget) = arguments.get("budget") {
        let credits = budget
            .as_f64()
            .filter(|credits| credits.is_finite() && *credits >= 0.0)
            .ok_or("budget is a number of credits, zero or more")?;
        words.extend(["--budget".to_string(), credits.to_string()]);
    }
    if let Some(tokens) = whole(arguments, "max_tokens")? {
        words.extend(["--max-tokens".to_string(), tokens.to_string()]);
    }
    if let Some(mode) = choice(arguments, "mode", &MODES)? {
        words.extend(["--mode".to_string(), mode]);
    }
    if let Some(country) = text(arguments, "country")? {
        words.extend(["--country".to_string(), country]);
    }
    Ok(())
}

/// Write what the call wants back: a goal, or named fields.
///
/// Selectors travel as the call's input and are read through `--selectors -`,
/// the way a shell would pipe them, so they never touch the disk.
fn goal(
    arguments: &Map<String, Value>,
    words: &mut Vec<String>,
    input: &mut Option<String>,
) -> Result<(), String> {
    if let Some(goal) = choice(arguments, "goal", &GOALS)? {
        words.extend(["--goal".to_string(), goal]);
    }
    match arguments.get("selectors") {
        None | Some(Value::Null) => {}
        Some(selectors @ Value::Object(_)) => {
            words.extend(["--selectors".to_string(), "-".to_string()]);
            *input = Some(selectors.to_string());
        }
        Some(_) => {
            return Err("selectors is a JSON object of field name to selector".to_string());
        }
    }
    Ok(())
}

/// A string argument that has to be there.
fn required(arguments: &Map<String, Value>, key: &str) -> Result<String, String> {
    text(arguments, key)?.ok_or_else(|| format!("{key} is required"))
}

/// A string argument, when the call named one.
fn text(arguments: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.clone())),
        Some(Value::String(_)) => Err(format!("{key} is empty")),
        Some(_) => Err(format!("{key} is a string")),
    }
}

/// A string argument that has to be one of a few names.
fn choice(
    arguments: &Map<String, Value>,
    key: &str,
    names: &[&str],
) -> Result<Option<String>, String> {
    match text(arguments, key)? {
        None => Ok(None),
        Some(name) if names.contains(&name.as_str()) => Ok(Some(name)),
        Some(name) => Err(format!(
            "{name} is not a {key}. Use one of: {}.",
            names.join(", ")
        )),
    }
}

/// A whole number argument, when the call named one.
fn whole(arguments: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} is a whole number, zero or more")),
    }
}

/// A switch, off unless the call turned it on.
fn flag(arguments: &Map<String, Value>, key: &str) -> Result<bool, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(on)) => Ok(*on),
        Some(_) => Err(format!("{key} is true or false")),
    }
}

/// Run one call and build its answer.
async fn answer(call: Call, http: reqwest::Client, log: Log) -> String {
    let Call { id, parsed, input } = call;
    let (outcome, written) = session::scope(http, input, run(parsed, log)).await;
    // Moved, not copied: a crawl's document can be large.
    let mut document = String::from_utf8(written)
        .unwrap_or_else(|bad| String::from_utf8_lossy(bad.as_bytes()).into_owned());
    let kept = document.trim_end().len();
    document.truncate(kept);
    let reason = match outcome {
        Ok(Code::Ok) => None,
        Ok(code) => Some(format!(
            "The call ended as {} (exit code {}). The items say what happened to each address.",
            code.label(),
            code.number()
        )),
        Err(failure) if failure.is_silent() => None,
        Err(failure) => Some(failure.message),
    };
    let mut content = Vec::new();
    if !document.is_empty() {
        content.push(json!({ "type": "text", "text": document }));
    }
    if let Some(reason) = &reason {
        content.push(json!({ "type": "text", "text": reason }));
    }
    result(
        &id,
        json!({ "content": content, "isError": reason.is_some() }),
    )
}

/// The command a call stands for, boxed.
///
/// `dispatch` reaches this module for the `mcp` command and this module
/// reaches `dispatch` for every tool, so one of the two futures has to be
/// behind a pointer or neither has a size.
fn run(parsed: Cli, log: Log) -> Pin<Box<dyn Future<Output = Run<Code>>>> {
    Box::pin(async move { crate::dispatch(&parsed, log).await })
}

/// The tools, as `tools/list` returns them.
fn tools() -> Value {
    let caps = json!({
        "budget": { "type": "number", "minimum": 0, "description": "The most this call may spend, in credits." },
        "max_tokens": { "type": "integer", "minimum": 0, "description": "The most to hand back, in approximate tokens, shared across the pages." },
        "mode": { "type": "string", "enum": MODES, "description": "How the page is fetched. Leave it out and the tool picks: plain HTTP when that is enough, a browser when the page needs one." },
        "country": { "type": "string", "description": "The country to appear to be in, as a two letter code." }
    });
    let goal = json!({ "type": "string", "enum": GOALS, "description": "What to return. markdown is the default. links and metadata skip the page content and cost less." });
    let selectors = json!({ "type": "object", "minProperties": 1, "additionalProperties": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }] }, "description": "Named fields to pull out, as field name to CSS selector, or to a list of selectors tried in order. A selector that starts with a slash is XPath. With selectors only the fields come back, never the page." });
    let url = json!({ "type": "string", "description": "The address, with its scheme." });
    json!([
        tool(
            "spider_scrape",
            "Read one web page and return it as markdown, text, HTML, links, metadata or named fields.",
            json!({ "url": url, "goal": goal, "selectors": selectors }),
            &["url"],
            Some(&caps),
        ),
        tool(
            "spider_crawl",
            "Read a site by following its links from a starting page. Reads 10 pages unless limit says otherwise.",
            json!({
                "url": url, "goal": goal, "selectors": selectors,
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_CRAWL_LIMIT, "description": "How many pages the crawl may read. 10 when left out." },
                "depth": { "type": "integer", "minimum": 0, "description": "How many links deep from the starting page the crawl may go." }
            }),
            &["url"],
            Some(&caps),
        ),
        tool(
            "spider_search",
            "Search the web and return the results. Set fetch_pages to also read the page behind each result.",
            json!({
                "query": { "type": "string", "description": "What to search for." },
                "limit": { "type": "integer", "minimum": 1, "description": "How many results to return." },
                "fetch_pages": { "type": "boolean", "description": "Read the page behind every result. One fetch per result." }
            }),
            &["query"],
            Some(&caps),
        ),
        tool(
            "spider_links",
            "Collect the links on a page without reading its content.",
            json!({ "url": url }),
            &["url"],
            Some(&caps),
        ),
        tool(
            "spider_transform",
            "Convert HTML you already hold into markdown or text, without fetching anything.",
            json!({
                "html": { "type": "string", "description": "The markup to convert." },
                "to": { "type": "string", "enum": SHAPES, "description": "The shape to convert into. markdown when left out." },
                "url": { "type": "string", "description": "Where the markup came from, so relative links resolve." },
                "readability": { "type": "boolean", "description": "Strip navigation, adverts and other page furniture first." }
            }),
            &["html"],
            None,
        ),
        tool(
            "spider_get_credits",
            "Read the credits left on the account. Costs nothing.",
            json!({}),
            &[],
            None,
        ),
    ])
}

/// One tool definition.
fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    caps: Option<&Value>,
) -> Value {
    let mut properties = match properties {
        Value::Object(properties) => properties,
        _ => Map::new(),
    };
    if let Some(Value::Object(caps)) = caps {
        for (key, schema) in caps {
            properties.insert(key.clone(), schema.clone());
        }
    }
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }
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

    fn arguments(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn reply(line: &str) -> Value {
        match handle(line) {
            Step::Reply(text) => serde_json::from_str(&text).unwrap(),
            _ => panic!("{line} did not lead to a reply"),
        }
    }

    #[test]
    fn initialize_answers_with_the_revision_the_client_asked_for() {
        let answer = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(answer["result"]["serverInfo"]["name"], "spider-agent");
        assert!(answer["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_offers_its_newest_revision_for_one_it_does_not_know() {
        let answer = reply(
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(answer["result"]["protocolVersion"], VERSIONS[0]);
        assert_eq!(answer["id"], "a");
    }

    #[test]
    fn a_notification_is_never_answered() {
        assert!(matches!(
            handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Step::Silent
        ));
        assert!(matches!(
            handle(r#"{"jsonrpc":"2.0","method":"tools/list"}"#),
            Step::Silent
        ));
    }

    #[test]
    fn a_line_that_is_not_json_is_a_parse_error() {
        let answer = reply("not json");
        assert_eq!(answer["error"]["code"], PARSE_ERROR);
        assert!(answer["id"].is_null());
    }

    #[test]
    fn a_batch_is_answered_message_by_message() {
        let step = handle(
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"nope"}]"#,
        );
        let Step::Batch(steps) = step else {
            panic!("not a batch");
        };
        assert_eq!(steps.len(), 3);
        assert!(matches!(steps[0], Step::Reply(_)));
        assert!(matches!(steps[1], Step::Silent));
        assert!(matches!(steps[2], Step::Reply(_)));
    }

    #[test]
    fn an_empty_batch_is_refused() {
        let answer = reply("[]");
        assert_eq!(answer["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn a_request_with_no_method_is_answered_and_a_stray_answer_is_not() {
        let answer = reply(r#"{"jsonrpc":"2.0","id":1}"#);
        assert_eq!(answer["error"]["code"], INVALID_REQUEST);
        assert_eq!(answer["id"], 1);
        let answer = reply(r#"{"jsonrpc":"2.0","id":2,"method":5}"#);
        assert_eq!(answer["error"]["code"], INVALID_REQUEST);
        assert!(matches!(
            handle(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#),
            Step::Silent
        ));
    }

    #[test]
    fn a_cancellation_names_the_request_to_stop() {
        let step = handle(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"r1","reason":"timeout"}}"#,
        );
        match step {
            Step::Cancel(id) => assert_eq!(id, "r1"),
            _ => panic!("not a cancellation"),
        }
    }

    #[test]
    fn a_crawl_limit_is_between_one_and_the_ceiling() {
        for limit in [0, MAX_CRAWL_LIMIT + 1] {
            let refused = plan(
                "spider_crawl",
                &arguments(json!({ "url": "https://example.com", "limit": limit })),
            )
            .unwrap_err();
            assert!(refused.contains("between 1 and"), "{refused}");
        }
        assert!(plan(
            "spider_crawl",
            &arguments(json!({ "url": "https://example.com", "limit": MAX_CRAWL_LIMIT })),
        )
        .is_ok());
    }

    #[test]
    fn an_unknown_method_says_so() {
        let answer = reply(r#"{"jsonrpc":"2.0","id":7,"method":"resources/list"}"#);
        assert_eq!(answer["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(answer["id"], 7);
    }

    #[test]
    fn ping_answers_with_an_empty_result() {
        let answer = reply(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
        assert_eq!(answer["result"], json!({}));
    }

    #[test]
    fn the_list_names_every_tool_with_a_closed_schema() {
        let answer = reply(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        let tools = answer["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "spider_scrape",
                "spider_crawl",
                "spider_search",
                "spider_links",
                "spider_transform",
                "spider_get_credits"
            ]
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert!(!tool["description"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn every_listed_tool_has_a_plan_and_every_listed_argument_is_accepted() {
        // The list and the planner are written apart. This is what keeps a
        // tool from advertising an argument the planner then refuses.
        let listed = tools();
        for tool in listed.as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            let properties = tool["inputSchema"]["properties"].as_object().unwrap();
            let mut call = Map::new();
            for (key, schema) in properties {
                let value = match (schema["type"].as_str().unwrap(), schema.get("enum")) {
                    ("string", Some(names)) => names[0].clone(),
                    ("string", None) => json!("https://example.com"),
                    ("integer", _) => json!(3),
                    ("number", _) => json!(1.5),
                    ("boolean", _) => json!(true),
                    ("object", _) => json!({"title": "h1"}),
                    (other, _) => panic!("{name}.{key} has type {other}"),
                };
                call.insert(key.clone(), value);
            }
            let plan = plan(name, &call).unwrap_or_else(|e| panic!("{name}: {e}"));
            Cli::try_parse_from(&plan.words).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn scrape_puts_the_address_after_the_separator() {
        let plan = plan(
            "spider_scrape",
            &arguments(json!({"url": "https://example.com", "goal": "text", "budget": 2})),
        )
        .unwrap();
        assert_eq!(
            plan.words,
            [
                "spider-agent",
                "scrape",
                "--goal",
                "text",
                "--budget",
                "2",
                "--json",
                "--quiet",
                "--",
                "https://example.com"
            ]
        );
        assert!(plan.input.is_none());
    }

    #[test]
    fn an_address_that_looks_like_a_flag_stays_an_address() {
        let plan = plan(
            "spider_scrape",
            &arguments(json!({"url": "--output=/tmp/owned"})),
        )
        .unwrap();
        let parsed = Cli::try_parse_from(&plan.words).unwrap();
        assert!(parsed.global.output.is_none());
        match parsed.command {
            Some(crate::cli::Command::Scrape(args)) => {
                assert_eq!(args.targets.urls, ["--output=/tmp/owned"]);
            }
            other => panic!("parsed as {other:?}"),
        }
    }

    #[test]
    fn a_value_that_looks_like_a_flag_cannot_become_one() {
        // A country is a flag's value. The parser refuses a value that starts
        // with a dash rather than reading it as the next flag.
        let plan = plan(
            "spider_links",
            &arguments(json!({"url": "https://example.com", "country": "--output=/tmp/owned"})),
        )
        .unwrap();
        match Cli::try_parse_from(&plan.words) {
            Err(_) => {}
            Ok(parsed) => assert!(parsed.global.output.is_none()),
        }
    }

    #[test]
    fn crawl_reads_ten_pages_unless_told_otherwise() {
        let unset = plan("spider_crawl", &arguments(json!({"url": "https://example.com"}))).unwrap();
        let at = unset.words.iter().position(|w| w == "--limit").unwrap();
        assert_eq!(unset.words[at + 1], "10");

        let set = plan(
            "spider_crawl",
            &arguments(json!({"url": "https://example.com", "limit": 3})),
        )
        .unwrap();
        let at = set.words.iter().position(|w| w == "--limit").unwrap();
        assert_eq!(set.words[at + 1], "3");
    }

    #[test]
    fn selectors_travel_as_the_input_and_never_as_a_file_name() {
        let plan = plan(
            "spider_scrape",
            &arguments(json!({"url": "https://example.com", "selectors": {"title": "h1"}})),
        )
        .unwrap();
        let at = plan.words.iter().position(|w| w == "--selectors").unwrap();
        assert_eq!(plan.words[at + 1], "-");
        assert_eq!(plan.input.as_deref(), Some(r#"{"title":"h1"}"#));
    }

    #[test]
    fn transform_reads_the_markup_from_the_call() {
        let plan = plan(
            "spider_transform",
            &arguments(json!({"html": "<p>hi</p>", "to": "text", "readability": true})),
        )
        .unwrap();
        assert_eq!(plan.input.as_deref(), Some("<p>hi</p>"));
        assert!(plan.words.contains(&"--readability".to_string()));
        assert!(!plan.words.contains(&"--".to_string()));
    }

    #[test]
    fn an_argument_the_tool_does_not_take_is_refused() {
        let refusal = plan(
            "spider_links",
            &arguments(json!({"url": "https://example.com", "output": "/tmp/x"})),
        )
        .unwrap_err();
        assert!(refusal.contains("output"), "{refusal}");

        // A cap belongs to the tools that fetch, and to no other.
        let refusal =
            plan("spider_get_credits", &arguments(json!({"budget": 1}))).unwrap_err();
        assert!(refusal.contains("budget"), "{refusal}");
    }

    #[test]
    fn a_wrong_type_or_a_missing_address_is_refused_before_anything_runs() {
        assert!(plan("spider_scrape", &arguments(json!({}))).is_err());
        assert!(plan("spider_scrape", &arguments(json!({"url": 7}))).is_err());
        assert!(plan("spider_scrape", &arguments(json!({"url": "  "}))).is_err());
        assert!(plan(
            "spider_scrape",
            &arguments(json!({"url": "https://example.com", "goal": "screenshot"}))
        )
        .is_err());
        assert!(plan(
            "spider_crawl",
            &arguments(json!({"url": "https://example.com", "limit": -1}))
        )
        .is_err());
        assert!(plan(
            "spider_scrape",
            &arguments(json!({"url": "https://example.com", "budget": -1}))
        )
        .is_err());
    }

    #[test]
    fn an_unknown_tool_is_an_invalid_params_error() {
        let answer = reply(
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"spider_delete_everything","arguments":{}}}"#,
        );
        assert_eq!(answer["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn a_good_call_becomes_a_parsed_command() {
        let step = handle(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"spider_get_credits"}}"#,
        );
        match step {
            Step::Call(call) => {
                assert_eq!(call.id, 4);
                assert!(matches!(call.parsed.command, Some(crate::cli::Command::Credits)));
                assert!(call.parsed.global.json);
            }
            _ => panic!("not a call"),
        }
    }
}
