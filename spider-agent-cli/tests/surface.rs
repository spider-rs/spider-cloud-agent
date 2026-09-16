//! What another program sees when it calls this tool.
//!
//! Nothing here makes a call. Every case is a path the binary takes before a
//! request would go out, which is the part a caller has to be able to rely on:
//! the stream the payload arrives on, the record shape, and the code the
//! process leaves with.

// A test may unwrap and may panic: a test that cannot set itself up should
// fail loudly rather than quietly measure nothing. It may also sleep: these
// tests drive a child process from a plain thread, and there is no runtime
// here for a sleep to stall.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_methods
)]

use std::process::{Command, Output};

/// Run the binary with no key in the environment, so nothing can reach out.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .args(args)
        // A key resolved from the machine would turn a parse test into a call.
        .env("SPIDER_API_KEY", "")
        .env("SPIDER_CLOUD_API_KEY", "")
        // The credentials file is found under the home directory, which is
        // HOME on Unix and USERPROFILE on Windows. Both are pointed somewhere
        // empty so a developer machine with a key on it runs the same tests as
        // a build machine without one.
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .output()
        .expect("the binary runs")
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn help_leaves_with_zero_and_writes_to_stdout() {
    let output = run(&["--help"]);
    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains("spider-agent"));
    assert!(
        output.stderr.is_empty(),
        "help went to stderr as well: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn help_and_schema_share_the_key_resolution_wording() {
    let output = run(&["schema"]);
    assert_eq!(code(&output), 0);
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("a schema document");
    let note = document["notes"]
        .as_array()
        .expect("schema notes")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|note| note.starts_with("The key comes from "))
        .expect("the key resolution note");
    assert!(note.contains("then the keychain, when the binary was built with the keyring feature"));

    let help = run(&["--help"]);
    assert_eq!(code(&help), 0);
    let normalized_help = stdout(&help)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        normalized_help.contains(note),
        "help differs from schema: {note}"
    );
}

#[test]
fn a_command_that_does_not_exist_is_a_usage_failure() {
    let output = run(&["frobnicate"]);
    assert_eq!(code(&output), 2);
    assert!(
        output.stdout.is_empty(),
        "a failure reached stdout: {}",
        stdout(&output)
    );
}

#[test]
fn a_local_route_makes_no_call_and_names_a_transport() {
    let output = run(&["route", "https://example.com", "--json"]);
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("one document");
    let value = &document["items"][0];
    assert_eq!(value["type"], "route");
    assert!(value["mode"].is_string());
    assert!(value["proxy"].is_string());
}

#[test]
fn ndjson_is_one_object_per_line() {
    let output = run(&[
        "route",
        "https://example.com",
        "https://example.com/a",
        "--ndjson",
    ]);
    assert_eq!(code(&output), 0);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "{text}");
    for line in lines {
        let value: serde_json::Value = serde_json::from_str(line).expect("a line is one object");
        assert_eq!(value["type"], "route");
    }
}

#[test]
fn json_is_one_document_whatever_it_holds() {
    for count in [1usize, 2] {
        let mut args = vec!["route", "--json"];
        let addresses = ["https://example.com", "https://example.com/a"];
        args.extend(&addresses[..count]);
        let output = run(&args);
        let value: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("a document");
        assert!(value.is_object(), "{value}");
        assert_eq!(value["items"].as_array().map(Vec::len), Some(count));
    }
}

#[test]
fn the_schema_is_machine_readable_and_lists_the_records() {
    let output = run(&["schema"]);
    assert_eq!(code(&output), 0);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("a document");
    assert_eq!(value["tool"], "spider-agent");
    assert!(value["records"]["page"].is_object());
    assert!(value["records"]["report"].is_object());
    assert_eq!(value["exit_codes"]["4"], "budget");
}

#[test]
fn an_address_that_is_not_one_stops_before_anything_is_sent() {
    let output = run(&["route", "not a url"]);
    assert_eq!(code(&output), 2);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("not a url"), "{said}");
}

#[test]
fn a_goal_of_fields_without_selectors_is_refused_before_a_call() {
    let output = run(&["fetch", "https://example.com", "--goal", "fields"]);
    assert_eq!(code(&output), 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("--selectors"));
}

#[test]
fn a_file_that_is_already_there_is_not_clobbered() {
    let path = std::env::temp_dir().join(format!(
        "spider-agent-clobber-{}.ndjson",
        std::process::id()
    ));
    std::fs::write(&path, "held\n").expect("a file");
    let name = path.to_string_lossy().to_string();

    let refused = run(&["route", "https://example.com", "-o", &name]);
    assert_eq!(code(&refused), 7);
    assert_eq!(
        std::fs::read_to_string(&path).expect("still there"),
        "held\n"
    );

    let replaced = run(&["route", "https://example.com", "-o", &name, "--force"]);
    assert_eq!(code(&replaced), 0);
    assert!(std::fs::read_to_string(&path)
        .expect("written")
        .contains("example.com"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_directory_destination_names_a_file_per_address() {
    let dir = std::env::temp_dir().join(format!("spider-agent-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let name = dir.to_string_lossy().to_string();

    let output = run(&[
        "route",
        "https://example.com/one",
        "https://example.com/two",
        "-d",
        &name,
        "--mkdir",
    ]);
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut written: Vec<String> = std::fs::read_dir(&dir)
        .expect("the directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    written.sort();
    assert_eq!(written.len(), 2, "{written:?}");
    assert!(written[0].starts_with("example-com-one-"), "{written:?}");
    assert!(!written[0].contains('/'), "{written:?}");

    // The same run again writes the same names, which is what makes two runs
    // comparable.
    let again = run(&[
        "route",
        "https://example.com/one",
        "https://example.com/two",
        "-d",
        &name,
        "--force",
    ]);
    assert_eq!(code(&again), 0);
    let mut second: Vec<String> = std::fs::read_dir(&dir)
        .expect("the directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    second.sort();
    assert_eq!(written, second);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn addresses_arrive_on_stdin() {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .args(["route", "--ndjson"])
        .env("SPIDER_API_KEY", "")
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    child
        .stdin
        .as_mut()
        .expect("a pipe")
        .write_all(b"# a note\nhttps://example.com\nhttps://example.com/a\n")
        .expect("written");
    let output = child.wait_with_output().expect("it finishes");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).lines().count(), 2, "{}", stdout(&output));
}

#[test]
fn no_key_anywhere_is_an_auth_failure_and_not_a_crash() {
    let output = run(&["credits"]);
    assert_eq!(
        code(&output),
        3,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("SPIDER_API_KEY"), "{said}");
}

#[test]
fn nothing_the_tool_prints_carries_an_escape_code() {
    for args in [
        vec!["--help"],
        vec!["schema"],
        vec!["route", "https://example.com"],
        vec!["credits"],
    ] {
        let output = run(&args);
        let printed = format!(
            "{}{}",
            stdout(&output),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!printed.contains('\u{1b}'), "escape code in {args:?}");
    }
}

/// The defect this pins: on 0.1.x `spider-agent fetch` posted to `/scrape`,
/// and there was no `scrape` command at all. Both names now exist and they are
/// two different endpoints.
#[test]
fn scrape_and_fetch_are_both_commands_and_are_not_the_same_one() {
    let listed = stdout(&run(&["--help"]));
    assert!(listed.contains("scrape"), "no scrape command: {listed}");
    assert!(listed.contains("fetch"), "no fetch command: {listed}");

    let scrape = run(&["scrape", "--help"]);
    assert_eq!(code(&scrape), 0);
    let fetch = run(&["fetch", "--help"]);
    assert_eq!(code(&fetch), 0);
    assert_ne!(
        stdout(&scrape),
        stdout(&fetch),
        "the two commands print the same help, so nothing tells them apart"
    );
}

/// Someone who has not read the API docs has to be able to tell the two apart
/// from the help alone.
#[test]
fn the_fetch_help_says_what_makes_it_different_from_scrape() {
    let help = stdout(&run(&["fetch", "--help"]));
    assert!(
        help.contains("DOMAIN") && help.contains("PATH"),
        "the help does not say it takes a host and a path: {help}"
    );
    assert!(
        help.contains("not scrape") || help.contains("is not scrape"),
        "the help never mentions scrape, so nothing warns a caller off: {help}"
    );
    assert!(
        help.contains("config"),
        "the help does not say the service brings its own configuration: {help}"
    );
}

/// A command whose name is a host reads as a host, and `fetch` takes one
/// rather than an address.
#[test]
fn fetch_takes_a_host_and_not_an_address() {
    // No key is set, so this stops at auth rather than reaching the service.
    // What is being asserted is that the arguments parse at all, which is code
    // 3 and not code 2.
    let output = run(&["fetch", "example.com", "/docs"]);
    assert_eq!(
        code(&output),
        3,
        "a host and a path did not parse: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The schema is the contract another program reads instead of the help text,
/// so a renamed command has to show up in it.
#[test]
fn the_schema_names_scrape_and_fetch_separately() {
    let output = run(&["schema"]);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("a document");
    let scrape = value["command_notes"]["scrape"]
        .as_str()
        .expect("a scrape command");
    let fetch = value["command_notes"]["fetch"]
        .as_str()
        .expect("a fetch command");
    assert_ne!(scrape, fetch);
    assert!(
        fetch.contains("not scrape"),
        "the schema does not tell the two apart: {fetch}"
    );
}

/// The defect this pins: `table <name>` read whatever name it was given under
/// `/data`, so the tool was a way to ask the service for any table it holds.
/// 0.3.0 removed it in favour of commands that name their own thing. The
/// parser has to refuse the old spelling rather than treat `table` as an
/// address to scrape.
#[test]
fn there_is_no_command_that_reads_a_table_by_name() {
    let listed = stdout(&run(&["--help"]));
    assert!(
        !listed.contains("table"),
        "table is still in the help: {listed}"
    );

    // `table` on its own parses as an address, and `spider-agent` answers an
    // address that is not one with a usage failure. The pair below is what
    // proves the subcommand is gone: a subcommand would have read the name
    // after it and stopped at auth instead.
    let output = run(&["table", "websites"]);
    assert_eq!(
        code(&output),
        2,
        "table <name> still parses: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("table"), "{said}");

    // With the subcommand gone, `table --help` is the top level help, and that
    // page must not list it either.
    let help = stdout(&run(&["table", "--help"]));
    assert!(!help.contains("table"), "table is still listed: {help}");
}

/// The three reads that replaced it. Each parses, and each stops at auth
/// rather than at usage, which is what says the arguments were understood and
/// a call would have gone out.
#[test]
fn the_named_account_reads_parse_and_stop_at_auth() {
    for args in [
        vec!["sites"],
        vec!["sites", "--limit", "5"],
        vec!["keys"],
        vec!["keys", "--page", "2"],
        vec!["profile"],
    ] {
        let output = run(&args);
        assert_eq!(
            code(&output),
            3,
            "{args:?} did not reach auth: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "{args:?} wrote to stdout");
    }
}

/// `profile` answers with one row, so paging it would mean nothing and the
/// flags are not offered.
#[test]
fn profile_takes_no_paging() {
    assert_eq!(code(&run(&["profile", "--limit", "5"])), 2);
    assert_eq!(code(&run(&["profile", "--page", "2"])), 2);
}

/// Anyone reading `keys --help` has to learn that no key comes back, because
/// the alternative is a caller who assumes one does.
#[test]
fn the_keys_help_says_no_key_is_returned() {
    let help = stdout(&run(&["keys", "--help"]));
    assert!(code(&run(&["keys", "--help"])) == 0);
    assert!(
        help.contains("No key is returned"),
        "the help does not say what it does not return: {help}"
    );
    assert!(
        help.contains("token_name"),
        "the help does not name the columns: {help}"
    );
}

/// The help for a read that pages has to name its columns, because the columns
/// belong to the service and a caller cannot guess them.
#[test]
fn the_sites_help_names_its_columns() {
    let help = stdout(&run(&["sites", "--help"]));
    for column in ["domain", "crawl_budget", "anti_bot", "headless"] {
        assert!(help.contains(column), "{column} is not in the help: {help}");
    }
}

/// The schema is what a calling program reads instead of the help, so the
/// three commands are named there and the one that went is not.
#[test]
fn the_schema_names_the_account_reads_and_not_table() {
    let output = run(&["schema"]);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("a document");
    let commands = value["commands"].as_object().expect("commands");
    for name in ["sites", "keys", "profile"] {
        assert!(commands.contains_key(name), "{name} is missing");
    }
    assert!(!commands.contains_key("table"), "table is back");
    let keys = value["command_notes"]["keys"].as_str().expect("a line");
    assert!(
        keys.contains("metadata"),
        "the schema does not say what keys returns: {keys}"
    );
}

/// A stub of the service, for the cases that have to read what the tool sent.
///
/// It answers one request from a script and hands the body back down a channel.
/// The binary is pointed at it with `SPIDER_API_URL`, so nothing here reaches
/// the network.
fn stub(reply: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, BufReader, Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("an address");
    let (sender, seen) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Ok(clone) = stream.try_clone() else { break };
            let mut reader = BufReader::new(clone);
            let mut length = 0usize;
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 {
                    break;
                }
                if header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            if reader.read_exact(&mut body).is_err() {
                break;
            }
            if sender
                .send(String::from_utf8_lossy(&body).to_string())
                .is_err()
            {
                break;
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                reply.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://{address}"), seen)
}

/// The same stub with the status, extra header lines and body scripted per
/// request. Once the script runs out the last answer is repeated.
fn stub_script(script: &'static [(u16, &'static str, &'static str)]) -> String {
    use std::io::{BufRead, BufReader, Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("an address");

    std::thread::spawn(move || {
        for (answered, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { break };
            let Ok(clone) = stream.try_clone() else { break };
            let mut reader = BufReader::new(clone);
            let mut length = 0usize;
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 {
                    break;
                }
                if header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            if reader.read_exact(&mut body).is_err() {
                break;
            }
            let Some((status, headers, reply)) = script.get(answered).or(script.last()) else {
                break;
            };
            let head = format!(
                "HTTP/1.1 {status} Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{headers}\r\n",
                reply.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });

    format!("http://{address}")
}

/// Run the binary with a key and an address that go nowhere but the stub.
fn run_against(base: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .args(args)
        .env("SPIDER_API_KEY", "not-a-real-key")
        .env("SPIDER_API_URL", base)
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .output()
        .expect("the binary runs")
}

/// One page and its links, the way the service answers a scrape that asked for
/// both.
const PAGE_AND_LINKS: &str = r##"[{"url":"https://example.com/","status":200,
    "content":"# Example","links":["https://example.com/a"],
    "costs":{"total_cost":0.0003}}]"##;

/// Every mode the service reads is a mode the tool takes.
#[test]
fn the_mode_flag_takes_the_three_the_service_reads() {
    for mode in ["http", "smart", "browser"] {
        let output = run(&["route", "https://example.com", "--mode", mode, "--json"]);
        assert_eq!(
            code(&output),
            0,
            "--mode {mode} was refused: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// An unrecognised mode is read as plain HTTP at the service and nothing says
/// so, so the tool refuses it here rather than sending a request that quietly
/// does something else.
#[test]
fn a_mode_that_is_not_one_is_a_usage_failure_rather_than_a_fallback() {
    let output = run(&["route", "https://example.com", "--mode", "headful"]);
    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty(), "{}", stdout(&output));
    let said = String::from_utf8_lossy(&output.stderr);
    for mode in ["http", "smart", "browser"] {
        assert!(said.contains(mode), "the refusal names no modes: {said}");
    }
}

/// The mode the caller fixed reaches the socket with the spelling the service
/// reads, rather than stopping at the builder.
#[test]
fn the_mode_flag_reaches_the_wire() {
    for (mode, spelling) in [
        ("http", r#""request":"http""#),
        ("smart", r#""request":"smart""#),
        ("browser", r#""request":"browser""#),
    ] {
        let (base, seen) = stub(PAGE_AND_LINKS);
        let output = run_against(
            &base,
            &["scrape", "https://example.com", "--mode", mode, "--json"],
        );
        assert_eq!(
            code(&output),
            0,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let sent = seen.recv().expect("a request");
        assert!(sent.contains(spelling), "--mode {mode} sent {sent}");
    }
}

/// The links arrive with the page rather than costing a second call, and they
/// are written out with it.
#[test]
fn asking_for_links_sends_one_request_and_writes_them_out() {
    let (base, seen) = stub(PAGE_AND_LINKS);
    let output = run_against(
        &base,
        &["scrape", "https://example.com", "--with-links", "--json"],
    );
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sent = seen.recv().expect("a request");
    assert!(
        sent.contains(r#""return_page_links":true"#),
        "the flag never reached the wire: {sent}"
    );
    assert!(seen.try_recv().is_err(), "the links cost a second call");

    let document: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("one document");
    let page = &document["items"][0];
    assert_eq!(page["links"][0], "https://example.com/a");
    assert!(
        page["body"]
            .as_str()
            .unwrap_or_default()
            .contains("Example"),
        "the content went missing: {page}"
    );
}

/// The help has to say the links come back in the same call, because the
/// reason to reach for the flag is that they cost no extra call.
#[test]
fn the_links_flag_help_says_it_costs_no_second_call() {
    let help = stdout(&run(&["--help"]));
    assert!(help.contains("--with-links"), "{help}");
    assert!(
        help.contains("same call"),
        "the help does not say where the links come from: {help}"
    );
}

/// Spawn the binary with no key and every stream piped, for a case that has
/// to write to stdin or close a pipe itself.
fn spawn(args: &[&str]) -> std::process::Child {
    use std::process::Stdio;
    Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .args(args)
        .env("SPIDER_API_KEY", "")
        .env("SPIDER_CLOUD_API_KEY", "")
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs")
}

/// The same, pointed at a stub with a key that goes nowhere else.
fn spawn_against(base: &str, args: &[&str]) -> std::process::Child {
    use std::process::Stdio;
    Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .args(args)
        .env("SPIDER_API_KEY", "not-a-real-key")
        .env("SPIDER_API_URL", base)
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs")
}

/// Enough addresses that the run is still writing when a reader leaves.
fn many_addresses() -> Vec<String> {
    (0..400)
        .map(|n| format!("https://example.com/page/{n}"))
        .collect()
}

/// `spider-agent ... | head -1` is the ordinary way to look at the first
/// record. The reader leaving is not a failure of the run, so the run ends
/// with nothing on stderr and code 0, the way cat and grep end. The release
/// profile aborts on a panic, so a print macro that panics on a closed pipe
/// would hand a caller a core dump for having typed head.
#[test]
fn a_reader_that_closes_the_pipe_ends_the_run_quietly() {
    let addresses = many_addresses();
    for format in [&["--format", "text"][..], &["--ndjson"][..]] {
        let mut args: Vec<&str> = vec!["route"];
        args.extend(format.iter().copied());
        args.extend(addresses.iter().map(String::as_str));
        let mut child = spawn(&args);
        drop(child.stdin.take());
        // The reader takes nothing and goes away, which is head -0.
        drop(child.stdout.take());
        let output = child.wait_with_output().expect("it finishes");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{format:?}: the run did not end quietly: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "{format:?}: a closed pipe was reported as a failure: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A closed pipe stops the run rather than only silencing it. Every page
/// fetched after the reader left would be paid for and thrown away.
#[test]
fn a_closed_pipe_stops_the_run_before_the_next_page_is_paid_for() {
    let (base, seen) = stub(PAGE_AND_LINKS);
    let mut child = spawn_against(
        &base,
        &[
            "scrape",
            "https://example.com/one",
            "https://example.com/two",
            "https://example.com/three",
            "--ndjson",
        ],
    );
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("it finishes");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    seen.recv().expect("the first page went out");
    assert!(
        seen.recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "a second page was fetched after the reader had gone"
    );
}

/// Diagnostics go to stderr, and stderr can be a pipe whose reader has gone
/// as well. A note that cannot be written is dropped, not turned into a panic.
#[test]
fn a_broken_stderr_does_not_bring_the_tool_down() {
    let addresses = many_addresses();
    let mut args: Vec<&str> = vec!["route", "--verbose"];
    args.extend(addresses.iter().map(String::as_str));
    let mut child = spawn(&args);
    drop(child.stdin.take());
    drop(child.stderr.take());
    let output = child.wait_with_output().expect("it finishes");
    assert_eq!(
        output.status.code(),
        Some(0),
        "the tool died on a note nobody was reading"
    );
    assert_eq!(
        stdout(&output).lines().count(),
        addresses.len(),
        "the payload was cut short"
    );
}

/// A list saved by an editor on Windows starts with a byte order mark and
/// ends its lines in CRLF. Neither is part of an address.
#[test]
fn a_byte_order_mark_and_crlf_are_not_part_of_an_address() {
    use std::io::Write as _;
    let mut child = spawn(&["route", "--ndjson"]);
    child
        .stdin
        .take()
        .expect("a pipe")
        .write_all(b"\xEF\xBB\xBFhttps://example.com\r\nhttps://example.com/a\r\n")
        .expect("written");
    let output = child.wait_with_output().expect("it finishes");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 2, "{text}");
    let first: serde_json::Value =
        serde_json::from_str(text.lines().next().expect("a line")).expect("a record");
    assert_eq!(first["url"], "https://example.com/");
}

/// `-` names stdin on every flag that reads a file, so on the one flag that
/// writes it names stdout. Before this it created a file called `-` in the
/// working directory and put the payload there.
#[test]
fn a_dash_as_the_output_is_stdout_and_not_a_file() {
    let dir = std::env::temp_dir().join(format!("spider-agent-dash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let output = Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .args(["route", "https://example.com", "-o", "-"])
        .current_dir(&dir)
        .env("SPIDER_API_KEY", "")
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .output()
        .expect("the binary runs");
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout(&output).contains("example.com"),
        "nothing reached stdout: {}",
        stdout(&output)
    );
    assert!(
        !dir.join("-").exists(),
        "a file called - was created in the working directory"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--budget` is documented as the most the whole run may spend. On a list
/// of addresses it was applied to every page afresh, so a run of a hundred
/// pages under a cap of one credit paid for a hundred pages. `run` had the
/// check and the other page commands did not.
#[test]
fn the_budget_caps_the_whole_run_and_not_each_page() {
    let selectors = std::env::temp_dir().join(format!(
        "spider-agent-budget-selectors-{}.json",
        std::process::id()
    ));
    std::fs::write(&selectors, r#"{"title":"h1"}"#).expect("a file");
    let name = selectors.to_string_lossy().to_string();
    for command in ["scrape", "crawl", "extract", "links", "screenshot"] {
        let (base, seen) = stub(PAGE_AND_LINKS);
        let mut args = vec![
            command,
            "https://example.com/one",
            "https://example.com/two",
            "https://example.com/three",
            "--budget",
            "0.5",
            "--ndjson",
        ];
        if command == "extract" {
            args.extend(["--selectors", &name]);
        }
        let output = run_against(&base, &args);
        // The stub prices a page above the cap. The first page goes out,
        // because a price is not known until it is asked for, and the cap
        // stops the second.
        seen.recv().expect("the first page went out");
        assert!(
            seen.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "{command}: a page was fetched after the cap was spent"
        );
        assert_eq!(
            code(&output),
            4,
            "{command}: a run that ran out of budget left with {} rather than 4: {}",
            code(&output),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = stdout(&output);
        let last = text.lines().last().expect("a report");
        let report: serde_json::Value = serde_json::from_str(last).expect("a record");
        assert_eq!(report["type"], "report", "{command}: {last}");
        assert_eq!(report["stopped"], "budget", "{command}: {last}");
    }
    let _ = std::fs::remove_file(&selectors);
}

/// The time cap is the same contract and was applied per page the same way.
/// Unlike a price, the clock is known before a page is asked for, so a wall
/// of zero seconds is spent before the first page and nothing goes out.
#[test]
fn the_wall_caps_the_whole_run_and_not_each_page() {
    let (base, seen) = stub(PAGE_AND_LINKS);
    let output = run_against(
        &base,
        &[
            "scrape",
            "https://example.com/one",
            "https://example.com/two",
            "--wall",
            "0",
            "--ndjson",
        ],
    );
    assert!(
        seen.recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "a page was fetched after the wall was spent"
    );
    assert_eq!(
        code(&output),
        4,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = stdout(&output);
    let last = text.lines().last().expect("a report");
    let report: serde_json::Value = serde_json::from_str(last).expect("a record");
    assert_eq!(report["stopped"], "time", "{last}");
}

/// A cap that is not a number is no cap at all, and a caller who typed one
/// meant to be stopped. Refused at the command line rather than ignored.
#[test]
fn a_budget_that_is_not_a_number_is_refused() {
    for value in ["nan", "-1", "-inf"] {
        let flag = format!("--budget={value}");
        let output = run(&["route", "https://example.com", &flag]);
        assert_eq!(code(&output), 2, "{flag} was accepted");
        let flag = format!("--max-per-page={value}");
        let output = run(&["route", "https://example.com", &flag]);
        assert_eq!(code(&output), 2, "{flag} was accepted");
    }
    let output = run(&["route", "https://example.com", "--budget", "0.5"]);
    assert_eq!(code(&output), 0);
}

/// An argument that is not UTF-8 is a usage failure, not a panic.
#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_a_usage_failure() {
    use std::os::unix::ffi::OsStrExt as _;
    let output = Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .arg("route")
        .arg(std::ffi::OsStr::from_bytes(b"https://example.com/\xff"))
        .env("SPIDER_API_KEY", "")
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .output()
        .expect("the binary runs");
    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty());
}

/// Ctrl-C in the middle of a run. Every record written before it is on disk,
/// because NDJSON and text are flushed per record, and the process is gone
/// promptly rather than waiting out the page it was on. The exit is the
/// signal itself, which a shell reports as 130. JSON output is the exception
/// by design: it buffers until the run ends, and an interrupted run has no
/// end.
#[cfg(unix)]
#[test]
fn an_interrupt_mid_run_keeps_what_was_written_and_leaves_promptly() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::process::ExitStatusExt as _;

    // A stub that answers the first request and holds every later one open
    // for as long as the process lives.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("an address");
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for (index, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { break };
            let Ok(clone) = stream.try_clone() else { break };
            let mut reader = BufReader::new(clone);
            let mut length = 0usize;
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            if index == 0 {
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    PAGE_AND_LINKS.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(PAGE_AND_LINKS.as_bytes());
                let _ = stream.flush();
            } else {
                held.push(stream);
            }
        }
    });

    let path = std::env::temp_dir().join(format!(
        "spider-agent-interrupt-{}.ndjson",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let name = path.to_string_lossy().to_string();
    let mut child = spawn_against(
        &format!("http://{address}"),
        &[
            "scrape",
            "https://example.com/one",
            "https://example.com/two",
            "--ndjson",
            "-o",
            &name,
        ],
    );

    // Wait for the first record to land, then interrupt while the second
    // page hangs.
    let started = std::time::Instant::now();
    while std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) == 0 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "the first record never landed"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(sent.success());

    let interrupted = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        assert!(
            interrupted.elapsed() < std::time::Duration::from_secs(5),
            "the process did not leave on an interrupt"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(status.signal(), Some(2), "{status:?}");

    let written = std::fs::read_to_string(&path).expect("the file");
    let first: serde_json::Value =
        serde_json::from_str(written.lines().next().expect("a record")).expect("a record");
    assert_eq!(first["type"], "page", "{written}");
    let _ = std::fs::remove_file(&path);
}

/// Keep each plan private to its test, including concurrent test processes.
fn plan_file(label: &str, text: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "spider-agent-plan-{label}-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, text).expect("write plan");
    path
}

fn report(output: &Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).lines().last().expect("report")).expect("JSON report")
}

#[test]
fn a_bom_plan_reads_all_six_keys_and_stops_at_its_page_cap() {
    let path = plan_file("all", "\u{feff}{\"goal\":\"fields\",\"urls\":[\"https://example.com/\"],\"expand\":2,\"max_pages\":1,\"budget\":10,\"selectors\":{\"title\":[\"h1\",\"title\"]}}");
    let (base, seen) = stub(PAGE_AND_LINKS);
    let output = run_against(&base, &["run", "--plan", path.to_str().unwrap()]);
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request: serde_json::Value = serde_json::from_str(&seen.recv().unwrap()).unwrap();
    assert_eq!(request["return_page_links"], true, "{request}");
    assert!(request.to_string().contains("h1"), "{request}");
    assert_eq!(report(&output)["served"], 1);
    assert_eq!(report(&output)["stopped"], "pages");
    assert!(seen.try_recv().is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn plan_budget_is_enforced() {
    let path = plan_file(
        "budget",
        r#"{"urls":["https://example.com/","https://example.com/b"],"budget":1}"#,
    );
    let (base, seen) = stub(PAGE_AND_LINKS);
    let output = run_against(&base, &["run", "--plan", path.to_str().unwrap()]);
    assert_eq!(
        code(&output),
        4,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report(&output)["served"], 1);
    assert_eq!(seen.try_iter().count(), 1);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn invalid_plan_values_fail_before_a_request_even_when_overridden() {
    for (index, invalid) in [r#""10""#, "-1", "true", "1e999"].iter().enumerate() {
        let path = plan_file(
            &format!("invalid-{index}"),
            &format!(r#"{{"urls":["https://example.com/"],"budget":{invalid}}}"#),
        );
        let (base, seen) = stub(PAGE_AND_LINKS);
        let output = run_against(
            &base,
            &["run", "--plan", path.to_str().unwrap(), "--budget", "20"],
        );
        assert_eq!(code(&output), 2);
        assert!(String::from_utf8_lossy(&output.stderr).contains("plan"));
        assert!(output.stdout.is_empty());
        assert!(seen.try_recv().is_err());
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn explicit_goal_and_zero_expand_win_and_urls_accumulate() {
    let path = plan_file(
        "override",
        r#"{"goal":"html","expand":3,"max_pages":1,"budget":0,"urls":["https://example.com/b"]}"#,
    );
    let urls = plan_file("urls", "https://example.com/c\n");
    let (base, seen) = stub(PAGE_AND_LINKS);
    let output = run_against(
        &base,
        &[
            "run",
            "https://example.com/",
            "--urls-from",
            urls.to_str().unwrap(),
            "--plan",
            path.to_str().unwrap(),
            "--goal",
            "markdown",
            "--expand",
            "0",
            "--max-pages",
            "3",
            "--budget",
            "10",
        ],
    );
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report(&output)["targets"], 3);
    assert_eq!(report(&output)["served"], 3);
    let requests: Vec<serde_json::Value> = seen
        .try_iter()
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request["return_format"], "markdown", "{request}");
        assert_ne!(request["return_page_links"], true, "{request}");
    }
    assert_eq!(requests[0]["url"], "https://example.com/");
    assert_eq!(requests[1]["url"], "https://example.com/b");
    assert_eq!(requests[2]["url"], "https://example.com/c");
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(urls).unwrap();
}

#[test]
fn a_site_that_refuses_every_attempt_leaves_with_five() {
    let (base, seen) =
        stub(r#"[{"url":"https://example.com/","status":404,"content":"not found"}]"#);
    let output = run_against(&base, &["run", "https://example.com/"]);
    assert_eq!(
        code(&output),
        5,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("no usable page"));
    assert!(seen.try_iter().count() > 0);
}

/// A page came back blank, the walk climbed a step, and the service then rate
/// limited every retry. The run reached the site, so it leaves with the refused
/// code, the same as before, and the line says what stopped it.
#[test]
fn a_rate_limit_after_a_page_leaves_with_five() {
    const BLANK: &str = r#"[{"url":"https://example.com/","status":200,"content":"",
        "costs":{"total_cost":0.0001}}]"#;
    const LIMITED: (u16, &str, &str) = (429, "retry-after: 0\r\n", r#"{"error":"slow down"}"#);
    let base = stub_script(&[(200, "", BLANK), LIMITED, LIMITED, LIMITED, LIMITED]);
    let output = run_against(&base, &["run", "https://example.com/"]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(code(&output), 5, "{said}");
    assert!(said.contains("no usable page"), "{said}");
    assert!(
        said.contains("429"),
        "the line does not say what stopped it: {said}"
    );
}

#[test]
fn an_unreachable_service_leaves_with_six() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let output = run_against(&base, &["credits"]);
    assert_eq!(
        code(&output),
        6,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("transport"));
}

#[test]
fn a_failed_conversion_leaves_with_one_and_names_the_reason() {
    let input = plan_file("conversion", "<h1>Example</h1>");
    let (base, seen) =
        stub(r#"[{"url":"https://example.com/","status":404,"content":"conversion failed"}]"#);
    let output = run_against(&base, &["transform", "--input", input.to_str().unwrap()]);
    assert_eq!(
        code(&output),
        1,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("transform produced no usable document")
    );
    assert!(seen.try_iter().count() > 0);
    std::fs::remove_file(input).unwrap();
}

#[test]
fn plan_goal_and_expansion_apply_without_flags() {
    let path = plan_file(
        "goal",
        r#"{"goal":"html","urls":["https://example.com/"],"expand":1}"#,
    );
    let (base, seen) = stub(PAGE_AND_LINKS);
    let output = run_against(&base, &["run", "--plan", path.to_str().unwrap()]);
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report(&output)["served"], 2);
    let requests: Vec<serde_json::Value> = seen
        .try_iter()
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["return_format"], "raw");
    assert_eq!(requests[1]["url"], "https://example.com/a");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn command_line_selectors_replace_the_plan_map() {
    let path = plan_file(
        "selectors",
        r#"{"urls":["https://example.com/"],"selectors":{"title":"title"}}"#,
    );
    let fields = plan_file("fields", r#"{"price":".price"}"#);
    let (base, seen) = stub(PAGE_AND_LINKS);
    let output = run_against(
        &base,
        &[
            "run",
            "--plan",
            path.to_str().unwrap(),
            "--selectors",
            fields.to_str().unwrap(),
        ],
    );
    assert_eq!(
        code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = seen.recv().unwrap();
    assert!(request.contains(".price"), "{request}");
    assert!(!request.contains("title"), "{request}");
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(fields).unwrap();
}
