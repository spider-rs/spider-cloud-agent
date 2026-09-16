//! What another program sees when it calls this tool.
//!
//! Nothing here makes a call. Every case is a path the binary takes before a
//! request would go out, which is the part a caller has to be able to rely on:
//! the stream the payload arrives on, the record shape, and the code the
//! process leaves with.

// A test may unwrap and may panic: a test that cannot set itself up should
// fail loudly rather than quietly measure nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
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
    assert_eq!(value["exit_codes"]["4"], "budget: a cap stopped the run");
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
    let scrape = value["commands"]["scrape"]
        .as_str()
        .expect("a scrape command");
    let fetch = value["commands"]["fetch"]
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
    let keys = commands["keys"].as_str().expect("a line");
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
