//! What an agent client sees when it starts `spider-agent mcp`.
//!
//! Nothing here makes a call. The server is driven the way a client drives it,
//! through its stdin and stdout, with no key on the machine, so every tool
//! call stops at the missing key before a request would go out.

// A test may unwrap and may panic: a test that cannot set itself up should
// fail loudly rather than quietly measure nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// Start the server, send these lines, close stdin and wait for it to leave.
fn serve(lines: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .arg("mcp")
        // No key anywhere, so a tool call cannot reach out. See surface.rs.
        .env("SPIDER_API_KEY", "")
        .env("SPIDER_CLOUD_API_KEY", "")
        .env("HOME", std::env::temp_dir())
        .env("USERPROFILE", std::env::temp_dir())
        .env("SPIDER_AGENT_NO_UPDATE", "1")
        // And scrape, the one command that would go out without a key, is
        // told not to.
        .env("SPIDER_AGENT_NO_KEYLESS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    {
        let mut stdin = child.stdin.take().unwrap();
        for line in lines {
            stdin.write_all(line.as_bytes()).unwrap();
            stdin.write_all(b"\n").unwrap();
        }
        // Dropped here, which closes the stream and tells the server to finish.
    }
    child.wait_with_output().expect("the server leaves")
}

/// Every line the server wrote, parsed. A line that is not JSON fails the
/// test, because stdout carries protocol messages and nothing else.
fn messages(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}")))
        .collect()
}

fn with_id(messages: &[Value], id: i64) -> &Value {
    messages
        .iter()
        .find(|message| message["id"] == id)
        .unwrap_or_else(|| panic!("no answer to {id} in {messages:?}"))
}

#[test]
fn a_session_answers_each_request_and_leaves_cleanly_when_stdin_closes() {
    let output = serve(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
    ]);
    assert_eq!(output.status.code(), Some(0));
    let messages = messages(&output);
    // Three requests, three answers, and nothing for the notification.
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert_eq!(
        with_id(&messages, 1)["result"]["protocolVersion"],
        "2025-06-18"
    );
    assert_eq!(
        with_id(&messages, 2)["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    assert!(with_id(&messages, 3)["result"].is_object());
}

#[test]
fn a_tool_call_with_no_key_is_an_error_result_that_names_the_way_out() {
    let output = serve(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"spider_get_credits","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"spider_scrape","arguments":{"url":"https://example.com"}}}"#,
    ]);
    assert_eq!(output.status.code(), Some(0));
    let messages = messages(&output);
    assert_eq!(messages.len(), 2, "{messages:?}");
    for id in [1, 2] {
        let result = &with_id(&messages, id)["result"];
        // A tool that ran and failed answers with a result, not a protocol
        // error, so the model reads the reason.
        assert_eq!(result["isError"], true, "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("no api key"), "{text}");
        assert!(text.contains("SPIDER_API_KEY"), "{text}");
        // Inside a session the way to sign in is a tool.
        assert!(text.contains("spider_login"), "{text}");
    }
}

#[test]
fn a_refused_call_does_not_stop_the_calls_after_it() {
    let output = serve(&[
        "this is not json",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"spider_scrape","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"spider_scrape","arguments":{"url":"https://example.com","output":"/tmp/x"}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
    ]);
    assert_eq!(output.status.code(), Some(0));
    let messages = messages(&output);
    assert_eq!(messages.len(), 4, "{messages:?}");
    assert_eq!(messages[0]["error"]["code"], -32700);
    assert_eq!(with_id(&messages, 1)["error"]["code"], -32602);
    assert_eq!(with_id(&messages, 2)["error"]["code"], -32602);
    assert!(with_id(&messages, 3)["result"].is_object());
}

#[test]
fn the_schema_lists_the_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_spider-agent"))
        .args(["schema"])
        .env("SPIDER_AGENT_NO_UPDATE", "1")
        .output()
        .expect("the binary runs");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("\"mcp\""), "schema does not name mcp");
}
