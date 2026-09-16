//! Self update, end to end, against a release host that lives in the test.
//!
//! Every case copies the built binary into a directory of its own, gives it a
//! home directory of its own, and points it at a loopback stub through
//! `SPIDER_AGENT_UPDATE_BASE`, which only a debug build reads. Nothing here
//! reaches the network, and the binary under `target/` is never replaced.
//!
//! The "new release" is a shell script that answers `--version` the way the
//! real binary does and says so when it runs anything else, so a test can tell
//! from the output which binary ran a command.

#![cfg(unix)]
// A test may unwrap and panic: one that cannot set itself up should fail
// loudly. It may sleep: the background check is a separate process, and a
// plain thread polling for its result has no runtime to stall.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_methods
)]

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

const NEW: &str = "9.9.9";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn triple() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else {
        "x86_64-unknown-linux-gnu"
    }
}

fn asset(version: &str) -> String {
    format!("spider-agent-{version}-{}.tar.gz", triple())
}

// ---------------------------------------------------------------------------
// the release host
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Route {
    path: String,
    status: u16,
    headers: String,
    body: Vec<u8>,
}

fn route(path: &str, status: u16, headers: &str, body: &[u8]) -> Route {
    Route {
        path: path.to_string(),
        status,
        headers: headers.to_string(),
        body: body.to_vec(),
    }
}

/// A loopback server answering by path, 404 for anything it was not given.
/// Every request path goes down the channel.
struct Stub {
    base: String,
    seen: Receiver<String>,
}

impl Stub {
    fn paths(&self) -> Vec<String> {
        self.seen.try_iter().collect()
    }
}

fn serve(routes: Vec<Route>) -> Stub {
    let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0)).expect("a port");
    let address = listener.local_addr().expect("an address");
    let (sender, seen) = channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Ok(clone) = stream.try_clone() else {
                continue;
            };
            let mut reader = BufReader::new(clone);
            let mut request = String::new();
            if reader.read_line(&mut request).unwrap_or(0) == 0 {
                continue;
            }
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
            }
            let path = request.split(' ').nth(1).unwrap_or("").to_string();
            let _ = sender.send(path.clone());
            let found = routes.iter().find(|route| route.path == path);
            let (status, headers, body) = match found {
                Some(route) => (route.status, route.headers.clone(), route.body.clone()),
                None => (404, String::new(), b"not found".to_vec()),
            };
            let head = format!(
                "HTTP/1.1 {status} Scripted\r\ncontent-length: {}\r\nconnection: close\r\n{headers}\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    Stub {
        base: format!("http://{address}"),
        seen,
    }
}

/// A release at `version` with this archive and this checksum file. The
/// archive download goes through one redirect, the way GitHub's does.
fn release(version: &str, archive: &[u8], sums: &str) -> Stub {
    let tag = format!("v{version}");
    serve(vec![
        route("/latest", 302, &format!("location: /tag/{tag}\r\n"), b""),
        route(
            &format!("/download/{tag}/SHA256SUMS.txt"),
            200,
            "",
            sums.as_bytes(),
        ),
        route(
            &format!("/download/{tag}/{}", asset(version)),
            302,
            "location: /storage/archive\r\n",
            b"",
        ),
        route("/storage/archive", 200, "", archive),
    ])
}

/// A well formed release of [`NEW`] whose binary is `binary`.
fn good_release(binary: &[u8]) -> Stub {
    let archive = tarball(&[("spider-agent", binary)]);
    let sums = sums_for(&asset(NEW), &archive);
    release(NEW, &archive, &sums)
}

fn sums_for(name: &str, bytes: &[u8]) -> String {
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{}  spider-agent-{NEW}-some-other-platform.tar.gz\n{digest}  {name}\n",
        "0".repeat(64)
    )
}

/// What the new release's binary is: a script that answers like the real one.
fn new_binary(version: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"spider-agent {version}\"; exit 0; fi\necho \"new binary ran: $*\"\nexit 0\n"
    )
    .into_bytes()
}

fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, body) in entries {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..107].copy_from_slice(b"0000755");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        header[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
        header[136..147].copy_from_slice(b"00000000000");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(body);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.extend_from_slice(&[0u8; 1024]);
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tar).unwrap();
    encoder.finish().unwrap()
}

// ---------------------------------------------------------------------------
// an install of its own
// ---------------------------------------------------------------------------

struct Install {
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    exe: PathBuf,
    original: Vec<u8>,
}

impl Install {
    fn new(name: &str) -> Install {
        Install::under(name, "bin")
    }

    fn under(name: &str, bin: &str) -> Install {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir()
            .join("spider-agent-update-tests")
            .join(format!("{name}-{}-{nanos}", std::process::id()));
        let home = root.join("home");
        let bin = root.join(bin);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("spider-agent");
        std::fs::copy(env!("CARGO_BIN_EXE_spider-agent"), &exe).unwrap();
        let original = std::fs::read(&exe).unwrap();
        Install {
            root,
            home,
            bin,
            exe,
            original,
        }
    }

    fn command(&self, base: &str, args: &[&str]) -> Command {
        let mut command = Command::new(&self.exe);
        command
            .args(args)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("SPIDER_API_KEY", "")
            .env("SPIDER_CLOUD_API_KEY", "")
            .env("SPIDER_AGENT_UPDATE_BASE", base)
            .env_remove("SPIDER_AGENT_NO_UPDATE")
            .env_remove("SPIDER_AGENT_UPDATE_BACKGROUND");
        command
    }

    fn run(&self, base: &str, args: &[&str]) -> Output {
        self.command(base, args).output().expect("the binary runs")
    }

    fn run_with(&self, base: &str, args: &[&str], name: &str, value: &str) -> Output {
        self.command(base, args)
            .env(name, value)
            .output()
            .expect("the binary runs")
    }

    fn state_path(&self) -> PathBuf {
        self.home.join(".spider").join("update.json")
    }

    fn state(&self) -> serde_json::Value {
        std::fs::read_to_string(self.state_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(serde_json::Value::Null)
    }

    /// How the background check ended, once it has, and its lock is gone.
    fn wait_for_outcome(&self) -> String {
        let lock = self.home.join(".spider").join("update.lock");
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(60) {
            if let Some(outcome) = self.state()["outcome"].as_str() {
                if !lock.exists() {
                    return outcome.to_string();
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("the background check never finished: {}", self.state());
    }

    fn staged(&self) -> PathBuf {
        self.bin.join(".spider-agent.update")
    }

    fn installed(&self) -> Vec<u8> {
        std::fs::read(&self.exe).unwrap()
    }

    fn untouched(&self) -> bool {
        self.installed() == self.original
    }

    /// Only the binary and, when named, the staged file: no temporary file
    /// left behind by a refused or failed write.
    fn only_files(&self, expected: &[&str]) {
        let mut names: Vec<String> = std::fs::read_dir(&self.bin)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        let mut expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(names, expected);
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.bin, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

const LOCAL: &[&str] = &["route", "https://example.com/"];

// ---------------------------------------------------------------------------
// the cases
// ---------------------------------------------------------------------------

/// The whole path: a run stages the release in the background without its
/// own output changing, and the next run installs it first and runs on it.
#[test]
fn a_newer_release_is_staged_by_one_run_and_installed_by_the_next() {
    let install = Install::new("staged-then-applied");
    let stub = good_release(&new_binary(NEW));

    let control = install.run(
        &stub.base,
        &["route", "https://example.com/", "--no-update"],
    );
    let first = install.run(&stub.base, LOCAL);
    assert_eq!(code(&first), code(&control));
    assert_eq!(text(&first.stdout), text(&control.stdout));
    assert_eq!(text(&first.stderr), text(&control.stderr));

    assert_eq!(install.wait_for_outcome(), "staged");
    assert!(install.untouched(), "the running binary was replaced");
    assert_eq!(std::fs::read(install.staged()).unwrap(), new_binary(NEW));
    assert_eq!(install.state()["staged"]["version"], NEW);
    let paths = stub.paths();
    assert_eq!(paths.first().map(String::as_str), Some("/latest"));
    assert!(paths.iter().any(|path| path.ends_with("SHA256SUMS.txt")));

    let second = install.run(&stub.base, LOCAL);
    assert_eq!(code(&second), 0, "{}", text(&second.stderr));
    assert!(
        text(&second.stderr).contains(&format!("spider-agent {NEW} replaced {CURRENT}")),
        "{}",
        text(&second.stderr)
    );
    assert_eq!(install.installed(), new_binary(NEW));
    assert_eq!(
        text(&second.stdout).trim(),
        "new binary ran: route https://example.com/",
        "the command did not run on the new binary"
    );
    assert!(!install.staged().exists());
    assert!(install.state()["staged"].is_null());
    let mode = std::fs::metadata(&install.exe)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755);
    install.only_files(&["spider-agent"]);
}

#[test]
fn the_update_command_installs_at_once_and_reports_on_stderr_only() {
    let install = Install::new("foreground");
    let stub = good_release(&new_binary(NEW));
    let output = install.run(&stub.base, &["update"]);
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    assert!(
        text(&output.stderr).contains(&format!("installed spider-agent {NEW} over {CURRENT}")),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(install.installed(), new_binary(NEW));
    install.only_files(&["spider-agent"]);
}

#[test]
fn an_install_on_the_newest_release_is_left_alone() {
    let install = Install::new("up-to-date");
    let stub = release(CURRENT, b"never fetched", "never fetched");

    let first = install.run(&stub.base, LOCAL);
    assert_eq!(code(&first), 0);
    assert_eq!(install.wait_for_outcome(), "up to date");
    assert_eq!(stub.paths(), vec!["/latest".to_string()]);
    assert!(install.untouched());
    install.only_files(&["spider-agent"]);

    let output = install.run(&stub.base, &["update"]);
    assert_eq!(code(&output), 0);
    assert!(output.stdout.is_empty());
    assert!(text(&output.stderr).contains("is the newest release"));
    assert!(install.untouched());
}

#[test]
fn an_archive_that_does_not_match_its_checksum_is_refused() {
    let install = Install::new("mismatch");
    let archive = tarball(&[("spider-agent", &new_binary(NEW))]);
    let sums = sums_for(&asset(NEW), b"some other bytes entirely");
    let stub = release(NEW, &archive, &sums);

    let output = install.run(&stub.base, &["update"]);
    assert_eq!(code(&output), 1, "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("did not match SHA256SUMS.txt"),
        "{}",
        text(&output.stderr)
    );
    assert!(text(&output.stderr).contains("Nothing was installed"));
    assert!(install.untouched());
    install.only_files(&["spider-agent"]);

    // In the background the refusal is kept for the next run to report.
    let background = Install::new("mismatch-background");
    let first = background.run(&stub.base, LOCAL);
    assert_eq!(code(&first), 0);
    assert!(background.wait_for_outcome().starts_with("refused"));
    assert!(!background.staged().exists());
    let next = background.run(&stub.base, &["route", "https://example.com/", "--quiet"]);
    assert_eq!(code(&next), 0);
    assert!(
        text(&next.stderr).contains("did not match SHA256SUMS.txt"),
        "a refusal is reported even under --quiet: {}",
        text(&next.stderr)
    );
    let after = background.run(&stub.base, LOCAL);
    assert!(!text(&after.stderr).contains("SHA256SUMS"), "said twice");
    assert!(background.untouched());
    background.only_files(&["spider-agent"]);
}

#[test]
fn a_truncated_or_malformed_archive_is_refused_even_when_its_checksum_matches() {
    let whole = tarball(&[("spider-agent", &vec![b'#'; 200_000])]);
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "truncated",
            whole[..whole.len() / 2].to_vec(),
            "does not decompress",
        ),
        (
            "garbage",
            b"this is not a gzip stream".to_vec(),
            "does not decompress",
        ),
        (
            "no-binary",
            tarball(&[("README", b"nothing to run")]),
            "holds no spider-agent",
        ),
    ];
    for (name, archive, expected) in cases {
        let install = Install::new(name);
        let stub = release(NEW, &archive, &sums_for(&asset(NEW), &archive));
        let output = install.run(&stub.base, &["update"]);
        assert_eq!(code(&output), 1, "{name}: {}", text(&output.stderr));
        assert!(
            text(&output.stderr).contains(expected),
            "{name}: {}",
            text(&output.stderr)
        );
        assert!(install.untouched(), "{name}");
        install.only_files(&["spider-agent"]);
    }
}

#[test]
fn a_binary_that_does_not_answer_with_the_tagged_version_is_refused() {
    let install = Install::new("wrong-version");
    let stub = good_release(&new_binary("1.2.3"));
    let output = install.run(&stub.base, &["update"]);
    assert_eq!(code(&output), 1, "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("--version"));
    assert!(install.untouched());
    install.only_files(&["spider-agent"]);
}

#[test]
fn a_directory_this_user_cannot_write_is_skipped_with_a_manual_note_said_once() {
    let install = Install::new("read-only");
    std::fs::set_permissions(&install.bin, std::fs::Permissions::from_mode(0o555)).unwrap();
    let stub = good_release(&new_binary(NEW));

    let output = install.run(&stub.base, &["update"]);
    assert_eq!(code(&output), 7, "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("cannot be replaced by this user"));
    assert!(text(&output.stderr).contains("To update by hand"));
    assert!(
        !stub.paths().iter().any(|path| path.contains("download")),
        "downloaded for an install it could not write"
    );

    let background = Install::new("read-only-background");
    std::fs::set_permissions(&background.bin, std::fs::Permissions::from_mode(0o555)).unwrap();
    let first = background.run(&stub.base, LOCAL);
    assert_eq!(code(&first), 0);
    assert!(background.wait_for_outcome().starts_with("skipped"));
    let next = background.run(&stub.base, LOCAL);
    assert_eq!(code(&next), 0);
    assert!(
        text(&next.stderr).contains("To update by hand"),
        "{}",
        text(&next.stderr)
    );
    let after = background.run(&stub.base, LOCAL);
    assert!(
        !text(&after.stderr).contains("To update by hand"),
        "said twice"
    );
    assert!(install.untouched());
    assert!(background.untouched());
}

#[test]
fn a_cargo_install_is_left_to_cargo() {
    let install = Install::under("cargo", "bin");
    std::fs::write(install.root.join(".crates2.json"), "{}").unwrap();
    let stub = good_release(&new_binary(NEW));
    let output = install.run(&stub.base, &["update"]);
    assert_eq!(code(&output), 7, "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("cargo install spider-agent-cli"));
    assert!(install.untouched());
}

#[test]
fn the_opt_out_turns_off_the_check_the_command_and_a_staged_install() {
    let install = Install::new("opt-out");
    let stub = good_release(&new_binary(NEW));

    let by_env = install.run_with(&stub.base, LOCAL, "SPIDER_AGENT_NO_UPDATE", "1");
    let by_flag = install.run(
        &stub.base,
        &["route", "https://example.com/", "--no-update"],
    );
    assert_eq!(code(&by_env), 0);
    assert_eq!(code(&by_flag), 0);
    let refused_env = install.run_with(&stub.base, &["update"], "SPIDER_AGENT_NO_UPDATE", "1");
    let refused_flag = install.run(&stub.base, &["update", "--no-update"]);
    assert_eq!(code(&refused_env), 2, "{}", text(&refused_env.stderr));
    assert_eq!(code(&refused_flag), 2, "{}", text(&refused_flag.stderr));
    std::thread::sleep(Duration::from_millis(1500));
    assert!(stub.paths().is_empty(), "an opted out run reached the host");
    assert!(!install.state_path().exists());

    // Stage one, then check that neither form of the opt out installs it.
    let staging = install.run(&stub.base, LOCAL);
    assert_eq!(code(&staging), 0);
    assert_eq!(install.wait_for_outcome(), "staged");
    let by_env = install.run_with(&stub.base, LOCAL, "SPIDER_AGENT_NO_UPDATE", "yes");
    let by_flag = install.run(
        &stub.base,
        &["route", "https://example.com/", "--no-update"],
    );
    assert_eq!(code(&by_env), 0);
    assert_eq!(code(&by_flag), 0);
    assert!(by_env.stderr.is_empty() && by_flag.stderr.is_empty());
    assert!(install.untouched());
    assert!(install.staged().exists());
}

#[test]
fn a_second_run_inside_the_interval_does_not_check_again() {
    let install = Install::new("throttle");
    let stub = release(CURRENT, b"", "");
    let first = install.run(&stub.base, LOCAL);
    assert_eq!(code(&first), 0);
    assert_eq!(install.wait_for_outcome(), "up to date");
    let checked = install.state()["last_check"].as_u64().unwrap();
    assert_eq!(stub.paths().len(), 1);

    for _ in 0..3 {
        assert_eq!(code(&install.run(&stub.base, LOCAL)), 0);
    }
    std::thread::sleep(Duration::from_millis(1500));
    assert!(stub.paths().is_empty(), "checked again inside the interval");
    assert_eq!(install.state()["last_check"].as_u64(), Some(checked));
}

#[test]
fn a_failing_update_leaves_the_command_its_own_exit_code_and_output() {
    let failing = serve(vec![route("/latest", 500, "", b"broken")]);
    let limited = serve(vec![route("/latest", 403, "", b"rate limited")]);
    let closed = {
        let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0)).unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let scrape = ["scrape", "https://example.com/"];
    for (name, base) in [
        ("server-error", failing.base.as_str()),
        ("rate-limited", limited.base.as_str()),
        ("unreachable", closed.as_str()),
    ] {
        let install = Install::new(name);
        let control = install.run_with(base, &scrape, "SPIDER_AGENT_NO_UPDATE", "1");
        assert_eq!(code(&control), 3, "{name}: no key is an auth failure");
        let output = install.run(base, &scrape);
        assert_eq!(code(&output), code(&control), "{name}");
        assert_eq!(text(&output.stdout), text(&control.stdout), "{name}");
        assert_eq!(text(&output.stderr), text(&control.stderr), "{name}");
        assert!(install.wait_for_outcome().starts_with("skipped"), "{name}");
        let next = install.run(base, &scrape);
        assert_eq!(code(&next), 3, "{name}");
        assert_eq!(
            text(&next.stderr),
            text(&control.stderr),
            "{name}: a skip is silent"
        );
        assert!(install.untouched(), "{name}");
    }
}

#[test]
fn a_staged_file_whose_bytes_changed_is_deleted_and_the_command_still_runs() {
    let install = Install::new("tampered");
    let stub = good_release(&new_binary(NEW));
    assert_eq!(code(&install.run(&stub.base, LOCAL)), 0);
    assert_eq!(install.wait_for_outcome(), "staged");
    std::fs::write(install.staged(), b"#!/bin/sh\necho swapped\n").unwrap();

    let output = install.run(&stub.base, &["scrape", "https://example.com/"]);
    assert_eq!(code(&output), 3, "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("does not match the checksum recorded"),
        "{}",
        text(&output.stderr)
    );
    assert!(install.untouched());
    assert!(!install.staged().exists());
}

#[test]
fn a_file_dropped_where_updates_are_staged_is_never_installed() {
    let install = Install::new("dropped");
    let stub = release(CURRENT, b"", "");
    std::fs::write(install.staged(), new_binary(NEW)).unwrap();
    std::fs::set_permissions(install.staged(), std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = install.run(&stub.base, LOCAL);
    assert_eq!(code(&output), 0);
    assert!(install.untouched());
    assert!(!install.staged().exists());
    assert_eq!(install.wait_for_outcome(), "up to date");
}

/// A sanity check on the fixture itself, so the tests above cannot pass on an
/// archive the real release process would never produce.
#[test]
fn the_fixture_archive_matches_the_contract() {
    let bytes = tarball(&[("spider-agent", b"x")]);
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
    assert!(asset(NEW).starts_with("spider-agent-9.9.9-"));
    assert!(Path::new(env!("CARGO_BIN_EXE_spider-agent")).exists());
}
