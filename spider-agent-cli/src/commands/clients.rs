//! `spider-agent setup`: tell the agent clients on this machine about the
//! server in `spider-agent mcp`.
//!
//! Each client keeps its list of servers its own way. Three of them have a
//! command for adding one, and this runs that command, because the client is
//! the authority on its own files. The other three keep a small JSON file,
//! and this adds one entry to it and leaves every other entry alone.
//!
//! Nothing is written for a client that is not on the machine, and running it
//! twice changes nothing the second time. `--dry-run` says what would happen
//! and touches nothing.
//!
//! The entry names this binary by its full path. A client started from a dock
//! or a launcher does not have the shell's `PATH`, so a bare `spider-agent`
//! that works in a terminal fails there.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Map, Value};

use crate::cli::{Client, Format, Global, SetupArgs};
use crate::emit::Item;
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;
use crate::setup;

/// The name the server is registered under in every client.
const SERVER_NAME: &str = "spider";

/// The most of a client's config file this reads. They are a few kilobytes.
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

/// Every client this knows, in the order they are reported.
const CLIENTS: [Client; 6] = [
    Client::ClaudeCode,
    Client::Codex,
    Client::Cursor,
    Client::Windsurf,
    Client::ClaudeDesktop,
    Client::Vscode,
];

/// What happened for one client.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    client: Client,
    status: Status,
    /// The file that was written or the command that was run.
    how: String,
    /// The reason, when there is one worth saying.
    detail: Option<String>,
}

/// How one client ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// The entry was written.
    Added,
    /// The entry was already there and already right.
    Already,
    /// `--dry-run`: the entry would be written.
    WouldAdd,
    /// The client is not on this machine.
    NotFound,
    /// The client is here and the entry could not be written.
    Failed,
}

impl Status {
    const fn label(self) -> &'static str {
        match self {
            Status::Added => "added",
            Status::Already => "already",
            Status::WouldAdd => "would_add",
            Status::NotFound => "not_found",
            Status::Failed => "failed",
        }
    }
}

/// Where a client keeps its servers, and so how to add one.
enum Place {
    /// The client has a command for it. `check` asks whether the server is
    /// already there, when the client can say.
    Tool {
        program: &'static str,
        check: Option<Vec<String>>,
        add: Vec<String>,
    },
    /// The client reads a JSON file with the servers under `key`.
    File { path: PathBuf, key: &'static str },
}

/// Register the server with every client found, or with the ones named.
pub fn command(global: &Global, args: &SetupArgs, log: Log) -> Run<Code> {
    let binary = this_binary()?;
    let home = home();
    let chosen: Vec<Client> = if args.client.is_empty() {
        CLIENTS.to_vec()
    } else {
        args.client.clone()
    };

    let mut outcomes = Vec::new();
    for client in chosen {
        outcomes.push(match place(client, &binary, home.as_deref()) {
            Some(place) => apply(client, place, &binary, args.dry_run),
            None => Outcome {
                client,
                status: Status::NotFound,
                how: String::new(),
                detail: None,
            },
        });
    }

    let mut emitter = setup::emitter(global, Format::Text)?;
    for outcome in &outcomes {
        // A client that is not installed is noise in a list a person reads,
        // unless they asked for it by name.
        if outcome.status == Status::NotFound && args.client.is_empty() {
            continue;
        }
        emitter.write(item(outcome))?;
    }
    emitter.finish()?;

    let found = outcomes.iter().any(|o| o.status != Status::NotFound);
    if !found {
        log.say(format!(
            "no agent client found on this machine. For any client that speaks the Model Context Protocol, the server is the command {binary} with the argument mcp"
        ));
    }
    if outcomes.iter().any(|o| o.status == Status::Failed) {
        return Err(Failure::new(Code::Output, String::new()));
    }
    Ok(Code::Ok)
}

/// One client's outcome as a record and as a line.
fn item(outcome: &Outcome) -> Item {
    let name = outcome.client.label();
    let line = match (outcome.status, &outcome.detail) {
        (Status::Added, _) => format!("{name}: added ({})", outcome.how),
        (Status::Already, _) => format!("{name}: already set up ({})", outcome.how),
        (Status::WouldAdd, _) => format!("{name}: would add ({})", outcome.how),
        (Status::NotFound, _) => format!("{name}: not found on this machine"),
        (Status::Failed, Some(detail)) => format!("{name}: failed, {detail}"),
        (Status::Failed, None) => format!("{name}: failed"),
    };
    Item::structured(json!({
        "type": "setup",
        "client": name,
        "status": outcome.status.label(),
        "how": outcome.how,
        "detail": outcome.detail,
    }))
    .with_text(line, "txt")
}

/// The full path of the running binary, as the clients will start it.
fn this_binary() -> Run<String> {
    let path = std::env::current_exe().map_err(|e| {
        Failure::new(
            Code::Failed,
            format!("cannot tell where this binary is: {e}"),
        )
    })?;
    path.to_str().map(str::to_string).ok_or_else(|| {
        Failure::new(
            Code::Failed,
            "this binary's path is not text a config file can hold",
        )
    })
}

fn home() -> Option<PathBuf> {
    spider_cloud_agent::auth::store::spider_dir()
        .and_then(|dir| dir.parent().map(Path::to_path_buf))
}

/// Where this client keeps its servers, or `None` when it is not installed.
fn place(client: Client, binary: &str, home: Option<&Path>) -> Option<Place> {
    let words = |words: &[&str]| words.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
    match client {
        Client::ClaudeCode => on_path("claude").then(|| Place::Tool {
            program: "claude",
            check: Some(words(&["mcp", "get", SERVER_NAME])),
            add: words(&[
                "mcp",
                "add",
                "--scope",
                "user",
                SERVER_NAME,
                "--",
                binary,
                "mcp",
            ]),
        }),
        Client::Codex => on_path("codex").then(|| Place::Tool {
            program: "codex",
            check: Some(words(&["mcp", "get", SERVER_NAME])),
            add: words(&["mcp", "add", SERVER_NAME, "--", binary, "mcp"]),
        }),
        Client::Vscode => on_path("code").then(|| Place::Tool {
            program: "code",
            check: None,
            add: vec![
                "--add-mcp".to_string(),
                json!({ "name": SERVER_NAME, "command": binary, "args": ["mcp"] }).to_string(),
            ],
        }),
        Client::Cursor => file_in(home?.join(".cursor"), "mcp.json"),
        Client::Windsurf => file_in(home?.join(".codeium").join("windsurf"), "mcp_config.json"),
        Client::ClaudeDesktop => file_in(desktop_dir(home?)?, "claude_desktop_config.json"),
    }
}

/// A config file, when the directory the client keeps it in exists. The
/// directory is the sign the client is installed. The file may not exist yet.
fn file_in(dir: PathBuf, name: &str) -> Option<Place> {
    dir.is_dir().then(|| Place::File {
        path: dir.join(name),
        key: "mcpServers",
    })
}

/// Where Claude Desktop keeps its config on this operating system.
fn desktop_dir(home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(
            home.join("Library")
                .join("Application Support")
                .join("Claude"),
        )
    } else if cfg!(target_os = "windows") {
        // A literal so the leak checker can audit which variables ship.
        std::env::var_os("APPDATA").map(|roaming| PathBuf::from(roaming).join("Claude"))
    } else {
        Some(home.join(".config").join("Claude"))
    }
}

/// Whether a program by this name is on the search path.
fn on_path(program: &str) -> bool {
    // A literal so the leak checker can audit which variables ship.
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    let names: &[&str] = if cfg!(target_os = "windows") {
        &[".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(&paths).any(|dir| {
        names
            .iter()
            .any(|ending| dir.join(format!("{program}{ending}")).is_file())
    })
}

/// Add the server to one client.
fn apply(client: Client, place: Place, binary: &str, dry_run: bool) -> Outcome {
    match place {
        Place::Tool {
            program,
            check,
            add,
        } => {
            let how = format!("{program} {}", add.join(" "));
            let done = |status, detail| Outcome {
                client,
                status,
                how: how.clone(),
                detail,
            };
            // The client is asked first, so a second run reports what is
            // true instead of an "already exists" failure.
            if let Some(check) = check {
                if run(program, &check).is_ok() {
                    return done(Status::Already, None);
                }
            }
            if dry_run {
                return done(Status::WouldAdd, None);
            }
            match run(program, &add) {
                Ok(()) => done(Status::Added, None),
                Err(reason) => done(Status::Failed, Some(reason)),
            }
        }
        Place::File { path, key } => {
            let how = path.display().to_string();
            let done = |status, detail| Outcome {
                client,
                status,
                how: how.clone(),
                detail,
            };
            let entry = json!({ "command": binary, "args": ["mcp"] });
            let document = match read_config(&path) {
                Ok(document) => document,
                Err(reason) => return done(Status::Failed, Some(reason)),
            };
            let Some(updated) = with_entry(document, key, &entry) else {
                return done(Status::Already, None);
            };
            if dry_run {
                return done(Status::WouldAdd, None);
            }
            match write_config(&path, &updated) {
                Ok(()) => done(Status::Added, None),
                Err(reason) => done(Status::Failed, Some(reason)),
            }
        }
    }
}

/// Run a client's own command and say whether it succeeded.
///
/// It gets no stdin, so it cannot stop to ask a question, and its output is
/// kept off this process's stdout, which is the payload stream.
fn run(program: &str, arguments: &[String]) -> Result<(), String> {
    let output = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{program} did not start: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.lines().find(|line| !line.trim().is_empty());
    Err(match said {
        Some(line) => format!("{program} said: {}", line.trim()),
        None => format!("{program} left with {}", output.status),
    })
}

/// Read a client's config file. A file that is not there yet is an empty one.
fn read_config(path: &Path) -> Result<Map<String, Value>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("{} could not be read: {e}", path.display())),
    };
    let mut text = String::new();
    file.take(MAX_CONFIG_BYTES)
        .read_to_string(&mut text)
        .map_err(|e| format!("{} could not be read: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    // A file with comments or a trailing comma is not rewritten: parsing it
    // loosely and writing it back strictly would throw the comments away.
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(document)) => Ok(document),
        _ => Err(format!(
            "{} is not plain JSON, so it was left alone. Add the server by hand",
            path.display()
        )),
    }
}

/// The document with the server's entry in it, or `None` when it already
/// holds exactly that entry.
fn with_entry(
    mut document: Map<String, Value>,
    key: &str,
    entry: &Value,
) -> Option<Map<String, Value>> {
    let servers = document
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    // Something other than an object under the key is not a list of servers
    // this can add to. Replacing it gives the client a file it can read.
    if !servers.is_object() {
        *servers = Value::Object(Map::new());
    }
    let servers = servers.as_object_mut()?;
    if servers.get(SERVER_NAME) == Some(entry) {
        return None;
    }
    servers.insert(SERVER_NAME.to_string(), entry.clone());
    Some(document)
}

/// Replace the config file whole: written beside it, then renamed over it, so
/// a client reading at the wrong moment sees the old file or the new one.
fn write_config(path: &Path, document: &Map<String, Value>) -> Result<(), String> {
    // A config that is a link, as one kept in a dotfiles checkout is, stays a
    // link: the file it points at is the one replaced.
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = resolved.as_path();
    let failed = |e: std::io::Error| format!("{} could not be written: {e}", path.display());
    let mut text = serde_json::to_string_pretty(document)
        .map_err(|e| format!("{} could not be written: {e}", path.display()))?;
    text.push('\n');
    let mut beside = path.as_os_str().to_owned();
    beside.push(format!(".spider-agent-{}", std::process::id()));
    let beside = PathBuf::from(beside);
    let written = (|| {
        use std::io::Write;
        // Fails if the name is taken, so two runs never share a scratch file.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&beside)?;
        // The file keeps the mode the client gave it. A config that was
        // readable by its owner only stays that way.
        if let Ok(existing) = std::fs::metadata(path) {
            file.set_permissions(existing.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        // On disk before the rename, so a crash leaves the old file or the
        // whole new one and never half of either.
        file.sync_all()?;
        std::fs::rename(&beside, path)
    })();
    written.map_err(|e| {
        let _ = std::fs::remove_file(&beside);
        failed(e)
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

    const BINARY: &str = "/opt/tools/bin/spider-agent";

    fn entry() -> Value {
        json!({ "command": BINARY, "args": ["mcp"] })
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("spider-agent-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_place(dir: &Path) -> Place {
        Place::File {
            path: dir.join("mcp.json"),
            key: "mcpServers",
        }
    }

    #[test]
    fn a_client_with_no_config_file_gets_one_holding_the_server() {
        let dir = scratch("fresh");
        let outcome = apply(Client::Cursor, file_place(&dir), BINARY, false);
        assert_eq!(outcome.status, Status::Added, "{outcome:?}");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("mcp.json")).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["spider"], entry());
    }

    #[test]
    fn every_other_entry_in_the_file_is_kept() {
        let dir = scratch("keep");
        std::fs::write(
            dir.join("mcp.json"),
            r#"{"theme":"dark","mcpServers":{"other":{"command":"x","args":[]}}}"#,
        )
        .unwrap();
        let outcome = apply(Client::Cursor, file_place(&dir), BINARY, false);
        assert_eq!(outcome.status, Status::Added);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("mcp.json")).unwrap()).unwrap();
        assert_eq!(written["theme"], "dark");
        assert_eq!(written["mcpServers"]["other"]["command"], "x");
        assert_eq!(written["mcpServers"]["spider"], entry());
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let dir = scratch("twice");
        apply(Client::Cursor, file_place(&dir), BINARY, false);
        let first = std::fs::read(dir.join("mcp.json")).unwrap();
        let outcome = apply(Client::Cursor, file_place(&dir), BINARY, false);
        assert_eq!(outcome.status, Status::Already);
        assert_eq!(std::fs::read(dir.join("mcp.json")).unwrap(), first);
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let dir = scratch("dry");
        let outcome = apply(Client::Cursor, file_place(&dir), BINARY, true);
        assert_eq!(outcome.status, Status::WouldAdd);
        assert!(!dir.join("mcp.json").exists());
    }

    #[test]
    fn a_file_with_comments_is_left_exactly_as_it_was() {
        let dir = scratch("comments");
        let before = "{\n  // my servers\n  \"mcpServers\": {}\n}\n";
        std::fs::write(dir.join("mcp.json"), before).unwrap();
        let outcome = apply(Client::Cursor, file_place(&dir), BINARY, false);
        assert_eq!(outcome.status, Status::Failed);
        assert!(outcome.detail.unwrap().contains("by hand"));
        assert_eq!(
            std::fs::read_to_string(dir.join("mcp.json")).unwrap(),
            before
        );
    }

    #[test]
    fn a_moved_binary_updates_the_entry_rather_than_adding_a_second() {
        let dir = scratch("moved");
        apply(Client::Cursor, file_place(&dir), "/old/spider-agent", false);
        let outcome = apply(Client::Cursor, file_place(&dir), BINARY, false);
        assert_eq!(outcome.status, Status::Added);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("mcp.json")).unwrap()).unwrap();
        assert_eq!(written["mcpServers"].as_object().unwrap().len(), 1);
        assert_eq!(written["mcpServers"]["spider"]["command"], BINARY);
    }

    #[test]
    fn a_client_that_is_not_installed_has_no_place() {
        let home = scratch("empty-home");
        for client in [Client::Cursor, Client::Windsurf] {
            assert!(place(client, BINARY, Some(&home)).is_none());
        }
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        assert!(place(Client::Cursor, BINARY, Some(&home)).is_some());
    }

    #[test]
    fn a_record_names_the_client_and_what_happened() {
        let outcome = Outcome {
            client: Client::ClaudeCode,
            status: Status::WouldAdd,
            how: "claude mcp add".to_string(),
            detail: None,
        };
        let item = item(&outcome);
        assert_eq!(item.value["type"], "setup");
        assert_eq!(item.value["client"], "claude-code");
        assert_eq!(item.value["status"], "would_add");
        assert!(item.text.unwrap().starts_with("claude-code: would add"));
    }
}
