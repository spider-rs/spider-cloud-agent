// Integration tests are their own crate root, so the workspace's panic rules
// are opted out of here. A test that cannot set itself up should stop loudly.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::string_slice
)]

//! The collector binary against a stub of the service.
//!
//! The stub is the pattern of `spider-cloud-agent/tests/wire.rs`: a thread and a
//! `TcpListener` that answers every request from one script and hands each
//! request body back down a channel. No key is read anywhere here: a dry run
//! sends a placeholder, and the tests that stop at a usage error never get as
//! far as looking for one.

use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};

/// A served page with a first party script and two third party groups, billed
/// one credit.
const SERVED: &str = r#"[{
  "url": "https://collector-page.example/a",
  "status": 200,
  "content": "The harbour rebuilt pier number four after the winter storms and reopened the ferry.",
  "response_map": {
    "https://collector-page.example/a": 48213.0,
    "https://collector-page.example/app.js": 40000.0,
    "https://tracker-alpha.example/t.js": 90000.0,
    "https://cdn-beta.example/big.js": 300000.0
  },
  "costs": {"total_cost": 0.0001}
}]"#;

const PAGES: [&str; 2] = [
    "https://collector-page.example/a",
    "https://another-site.example/b",
];

/// One switch, one mode change, and an append of the heaviest observed group.
const SPEC: &str = r#"[
  [{"key": "block_stylesheets", "op": "set", "value": false}],
  [{"key": "request", "op": "set", "value": "browser"}],
  [{"key": "network_blacklist", "op": "append", "value": {"observed": 0}}]
]"#;

struct Stub {
    base: String,
    host: String,
    seen: Receiver<String>,
}

impl Stub {
    fn serve(reply: &'static str) -> Stub {
        let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, seen) = channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let Some(body) = read_body(&mut stream) else {
                    continue;
                };
                if sender.send(body).is_err() {
                    break;
                }
                let head = format!(
                    "HTTP/1.1 200 Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    reply.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        Stub {
            base: format!("http://{address}"),
            host: address.ip().to_string(),
            seen,
        }
    }

    fn bodies(&self) -> Vec<Value> {
        self.seen
            .try_iter()
            .map(|body| serde_json::from_str(&body).unwrap())
            .collect()
    }
}

fn read_body(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut length = 0usize;
    let mut first = String::new();
    if reader.read_line(&mut first).ok()? == 0 {
        return None;
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// A fresh directory under the system temp dir.
fn scratch(name: &str) -> PathBuf {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "optimize-collect-{name}-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run the collector with the url list and spec written into `dir`.
fn collect(dir: &Path, urls: &[&str], spec: &str, flags: &[&str]) -> Output {
    std::fs::write(dir.join("urls.txt"), urls.join("\n")).unwrap();
    std::fs::write(dir.join("spec.json"), spec).unwrap();
    Command::new(env!("CARGO_BIN_EXE_optimize-collect"))
        .arg("--urls")
        .arg(dir.join("urls.txt"))
        .arg("--candidates")
        .arg(dir.join("spec.json"))
        .arg("--out")
        .arg(dir.join("out"))
        .arg("--salt-file")
        .arg(dir.join("salt.bin"))
        .args(["--seed", "7"])
        .args(flags)
        // Nothing here may find a real key, whatever the machine has.
        .env_remove("SPIDER_API_KEY")
        .env_remove("SPIDER_CLOUD_API_KEY")
        .env("HOME", dir)
        .output()
        .unwrap()
}

fn dry_run<'a>(stub: &'a Stub, extra: &[&'a str]) -> Vec<&'a str> {
    let mut flags = vec!["--dry-run", "--base-url", stub.base.as_str()];
    flags.extend_from_slice(extra);
    flags
}

fn rows(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("out/rows.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("out/manifest.json")).unwrap()).unwrap()
}

fn grouped(rows: &[Value]) -> BTreeMap<u64, Vec<&Value>> {
    let mut pairs: BTreeMap<u64, Vec<&Value>> = BTreeMap::new();
    for row in rows {
        pairs
            .entry(row["pair"].as_u64().unwrap())
            .or_default()
            .push(row);
    }
    pairs
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Every string in a value, keys included.
fn strings<'v>(value: &'v Value, out: &mut Vec<&'v str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Value::Object(map) => {
            for (key, inner) in map {
                out.push(key);
                strings(inner, out);
            }
        }
        _ => {}
    }
}

/// Whether text holds `label.tld`, the shape `dataset.HOST_LIKE` refuses:
/// `[a-z0-9-]+\.[a-z]{2,}`.
fn host_like(text: &str) -> bool {
    let bytes = text.to_ascii_lowercase().into_bytes();
    bytes.iter().enumerate().any(|(at, byte)| {
        *byte == b'.'
            && at > 0
            && matches!(bytes[at - 1], b'a'..=b'z' | b'0'..=b'9' | b'-')
            && bytes[at + 1..]
                .iter()
                .take_while(|next| next.is_ascii_lowercase())
                .count()
                >= 2
    })
}

/// The validator's pair, host and shape rules, less the corpus size floors a
/// test directory is far below.
fn check_structure(dir: &Path) {
    let rows = rows(dir);
    let manifest = manifest(dir);
    assert_eq!(manifest["rows"], rows.len());
    assert_eq!(manifest["synthetic"], false);
    for (name, field) in [
        ("schema_v", "schema_version"),
        ("feat_v", "feature_version"),
        ("edit_feat_v", "edit_feature_version"),
    ] {
        for row in &rows {
            assert_eq!(row[name], manifest[field], "{name}");
        }
    }
    for row in &rows {
        let mut found = Vec::new();
        strings(row, &mut found);
        for text in found {
            assert!(
                !text.contains("://") && !host_like(text),
                "host-like {text:?}"
            );
        }
        let base = row["base"].as_array().unwrap();
        let edit = row["edit_feats"].as_array().unwrap();
        assert_eq!(base.len(), 152);
        assert_eq!(edit.len(), manifest["edit_dim"].as_u64().unwrap() as usize);
        for value in base.iter().chain(edit) {
            let value = value.as_f64().unwrap();
            assert!(
                value.is_finite() && (-1.0..=1.0).contains(&value),
                "{value}"
            );
        }
    }
    let pairs = grouped(&rows);
    assert_eq!(manifest["pairs"], pairs.len());
    for (pair, arms) in &pairs {
        let baselines = arms.iter().filter(|arm| arm["arm"] == "baseline").count();
        assert_eq!(baselines, 1, "pair {pair}");
        assert!(
            arms.iter().all(|arm| arm["day"] == arms[0]["day"]),
            "pair {pair} day"
        );
        assert!(
            arms.iter().all(|arm| arm["dk"] == arms[0]["dk"]),
            "pair {pair} dk"
        );
    }
}

#[test]
fn each_candidate_makes_one_pair_of_two_rows_per_url() {
    let stub = Stub::serve(SERVED);
    let dir = scratch("pairs");
    let output = collect(
        &dir,
        &PAGES,
        SPEC,
        &dry_run(&stub, &["--repeat-share", "0"]),
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let rows = rows(&dir);
    let pairs = grouped(&rows);
    assert_eq!(rows.len(), PAGES.len() * 3 * 2, "{}", stderr(&output));
    assert_eq!(pairs.len(), PAGES.len() * 3);
    let mut keys = Vec::new();
    for arms in pairs.values() {
        assert_eq!(arms.len(), 2);
        let candidate = arms.iter().find(|arm| arm["arm"] == "candidate").unwrap();
        let baseline = arms.iter().find(|arm| arm["arm"] == "baseline").unwrap();
        assert!(baseline["edit"].is_null());
        assert!(baseline["shingle_jaccard"].is_null());
        // The stub serves the same page to both arms.
        assert_eq!(candidate["shingle_jaccard"], 1.0);
        assert_eq!(candidate["byte_ratio"], 1.0);
        assert_eq!(candidate["content_ok"], true);
        keys.push(candidate["edit"]["key"].as_u64().unwrap());
    }
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), 3, "every entry was applied: {keys:?}");
    check_structure(&dir);

    // What went out: the baseline observes, the blacklist arm names the
    // heaviest third party group with hints off, and nothing else observes.
    let bodies = stub.bodies();
    assert_eq!(bodies.len(), PAGES.len() * 4);
    let observing = bodies
        .iter()
        .filter(|body| body["event_tracker"]["responses"] == true)
        .count();
    assert_eq!(observing, PAGES.len());
    let blacklisted: Vec<&Value> = bodies
        .iter()
        .filter(|body| body.get("network_blacklist").is_some())
        .collect();
    assert_eq!(blacklisted.len(), PAGES.len());
    for body in blacklisted {
        assert_eq!(
            body["network_blacklist"],
            serde_json::json!(["cdn-beta.example"])
        );
        assert_eq!(body["disable_hints"], true);
        assert!(body.get("event_tracker").is_none());
    }

    let manifest = manifest(&dir);
    assert_eq!(manifest["credits_spent"], 8.0);
    assert_eq!(manifest["tau"], 0.8);
    assert_eq!(manifest["service_revision"], "unattested");
    assert_eq!(manifest["salt_id"].as_str().unwrap().len(), 8);
    assert!(manifest["day_min"].as_u64().unwrap() <= manifest["day_max"].as_u64().unwrap());
}

#[test]
fn a_repeat_pair_holds_two_baseline_runs() {
    let stub = Stub::serve(SERVED);
    let dir = scratch("repeat");
    let spec = r#"[[{"key": "block_stylesheets", "op": "set", "value": false}]]"#;
    let output = collect(
        &dir,
        &PAGES[..1],
        spec,
        &dry_run(&stub, &["--repeat-share", "1"]),
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let rows = rows(&dir);
    assert_eq!(rows.len(), 4);
    let pairs = grouped(&rows);
    let repeats: Vec<_> = pairs
        .values()
        .filter(|arms| arms.iter().all(|arm| arm["edit"].is_null()))
        .collect();
    assert_eq!(repeats.len(), 1, "one pair of two baseline runs");
    let second = repeats[0]
        .iter()
        .find(|arm| arm["arm"] != "baseline")
        .unwrap();
    // Written the way the trainer reads a repeat: not the baseline arm, no edit.
    assert_eq!(second["arm"], "candidate");
    assert_eq!(second["shingle_jaccard"], 1.0);
    assert_eq!(second["content_ok"], true);
    assert_eq!(stub.bodies().len(), 3);
    check_structure(&dir);
}

#[test]
fn without_dry_run_or_spend_nothing_starts() {
    let dir = scratch("refuse");
    let output = collect(&dir, &PAGES, SPEC, &[]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(!dir.join("out").exists());
    assert!(!dir.join("salt.bin").exists());

    let output = collect(&dir, &PAGES, SPEC, &["--spend"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("--max-credits"));

    // A dry run only talks to loopback.
    let output = collect(
        &dir,
        &PAGES,
        SPEC,
        &["--dry-run", "--base-url", "https://api.spider.cloud"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

#[test]
fn an_invalid_spec_names_the_entry_before_any_request() {
    let stub = Stub::serve(SERVED);
    let dir = scratch("spec");
    let spec = r#"[
      [{"key": "block_ads", "op": "set", "value": true}],
      [{"key": "stealth", "op": "set", "value": true}]
    ]"#;
    let output = collect(&dir, &PAGES, spec, &dry_run(&stub, &[]));
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("entry 1"), "{}", stderr(&output));
    assert!(stub.bodies().is_empty());
}

#[test]
fn the_credit_cap_stops_the_run_with_exit_3() {
    let stub = Stub::serve(SERVED);
    let dir = scratch("cap");
    let spec = r#"[[{"key": "block_stylesheets", "op": "set", "value": false}]]"#;
    // A credit a run: the first url's pair spends two, the second url's
    // baseline reaches three, and nothing after it is sent.
    let output = collect(
        &dir,
        &PAGES,
        spec,
        &dry_run(&stub, &["--max-credits", "3", "--repeat-share", "0"]),
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("2 rows in 1 pairs"),
        "{}",
        stderr(&output)
    );

    assert_eq!(rows(&dir).len(), 2);
    let manifest = manifest(&dir);
    assert_eq!(manifest["rows"], 2);
    assert_eq!(manifest["credits_spent"], 3.0);
    let bodies = stub.bodies();
    assert_eq!(bodies.len(), 3);
    // The cap is mirrored onto every request.
    for body in &bodies {
        assert_eq!(body["max_credits_allowed"], 3);
    }
    check_structure(&dir);
}

#[test]
fn no_row_or_manifest_names_a_host_or_holds_the_salt() {
    let stub = Stub::serve(SERVED);
    let dir = scratch("hosts");
    let output = collect(
        &dir,
        &PAGES,
        SPEC,
        &dry_run(&stub, &["--repeat-share", "1"]),
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let text = std::fs::read_to_string(dir.join("out/rows.jsonl")).unwrap();
    let manifest = std::fs::read_to_string(dir.join("out/manifest.json")).unwrap();
    for needle in [
        stub.host.as_str(),
        "collector-page",
        "another-site",
        "tracker-alpha",
        "cdn-beta",
        "example",
        "http",
    ] {
        assert!(!text.contains(needle), "{needle:?} reached rows.jsonl");
        assert!(
            !manifest.contains(needle),
            "{needle:?} reached manifest.json"
        );
    }

    let salt = std::fs::read(dir.join("salt.bin")).unwrap();
    assert_eq!(salt.len(), 8);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("salt.bin"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let salt_number = u64::from_le_bytes(salt.try_into().unwrap()).to_string();
    assert!(!text.contains(&salt_number) && !manifest.contains(&salt_number));
    check_structure(&dir);

    // The same salt file gives the same site keys on a second run.
    let first: Vec<u64> = rows(&dir)
        .iter()
        .map(|row| row["dk"].as_u64().unwrap())
        .collect();
    let again = scratch("hosts-again");
    std::fs::copy(dir.join("salt.bin"), again.join("salt.bin")).unwrap();
    let output = collect(
        &again,
        &PAGES,
        SPEC,
        &dry_run(&stub, &["--repeat-share", "1"]),
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let second: Vec<u64> = rows(&again)
        .iter()
        .map(|row| row["dk"].as_u64().unwrap())
        .collect();
    assert_eq!(first, second);
    assert_ne!(first[0], first[first.len() - 1], "two sites, two keys");
}
