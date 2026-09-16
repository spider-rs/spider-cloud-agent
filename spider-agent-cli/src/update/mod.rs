//! Keeping the binary current.
//!
//! Three moments, in the order a release reaches a machine:
//!
//! 1. A run notices the last check is more than [`CHECK_INTERVAL`] old and
//!    starts a detached `spider-agent update` in the background, then gets on
//!    with its own command. The command's output, timing and exit code do not
//!    depend on that child in any way; nothing waits for it and its standard
//!    streams are closed.
//! 2. That child asks where the newest release is, downloads the archive for
//!    this platform and `SHA256SUMS.txt` from the same release, refuses the
//!    archive unless it matches, takes the binary out, checks it answers
//!    `--version` with the tagged version, and stages it beside the running
//!    binary. It records the staged file's SHA-256 in `~/.spider/update.json`.
//! 3. The next run finds the staged file, hashes it, and only if the digest
//!    matches the recorded one renames it over the binary. It then starts the
//!    new binary in its own place with the same arguments, so that run is
//!    already on the new version.
//!
//! `spider-agent update` does the same work in the foreground and installs at
//! once. [`OPT_OUT_ENV`] or `--no-update` turns every part of this off,
//! including step 3 for an update staged earlier.
//!
//! Every problem here becomes a line on stderr or nothing at all. None of them
//! changes what the caller's command does.

mod archive;
mod install;
mod release;
mod state;

use std::fmt;
use std::time::Duration;

use url::Url;

use crate::cli::{Command, Global};
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;

use self::install::{Applied, Target};
use self::release::{Release, Releases};
use self::state::{Lock, Notice, Staged, State};

/// Set to anything but an empty value to turn self update off entirely.
pub const OPT_OUT_ENV: &str = "SPIDER_AGENT_NO_UPDATE";

/// Marks the detached child that runs a background check. Not a setting.
const BACKGROUND_ENV: &str = "SPIDER_AGENT_UPDATE_BACKGROUND";

/// How often a background check may run.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Where releases are published.
pub const RELEASES: &str = "https://github.com/spider-rs/spider-cloud-agent/releases";

/// Where to look for releases, and whether plain http to loopback is allowed.
pub struct Settings {
    base: Url,
    allow_loopback_http: bool,
}

/// The settings this build runs under, or `None` when it does not update
/// itself.
///
/// A release build always reads [`RELEASES`] over https, and the code that
/// reads anything else is not compiled into it: the `debug_assertions` cfg is
/// off under the release profile this workspace ships, so neither the
/// environment read below nor the loopback exception exists there.
///
/// A debug build updates itself only when `SPIDER_AGENT_UPDATE_BASE` names a
/// release base, which is how the tests point it at a loopback stub. Without
/// it a debug build does nothing, so `cargo run` and `cargo test` never touch
/// the network or replace a binary under `target/`.
fn settings() -> Option<Settings> {
    #[cfg(debug_assertions)]
    {
        let base = std::env::var("SPIDER_AGENT_UPDATE_BASE").ok()?;
        Url::parse(&base).ok().map(|base| Settings {
            base,
            allow_loopback_http: true,
        })
    }
    #[cfg(not(debug_assertions))]
    {
        Url::parse(RELEASES).ok().map(|base| Settings {
            base,
            allow_loopback_http: false,
        })
    }
}

/// The platform part of the asset name, for the platforms releases are built
/// for. Anything else has no asset and is not updated.
pub const fn triple() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "aarch64"
    )) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "x86_64"
    )) {
        Some("x86_64-unknown-linux-gnu")
    } else {
        None
    }
}

/// A release version: three numbers and nothing after them. A tag with a
/// pre-release suffix is not a version this tool moves to on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u64, u64, u64);

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.split('.');
        let mut number = || -> Option<u64> {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let version = Version(number()?, number()?, number()?);
        parts.next().is_none().then_some(version)
    }

    /// The version of this binary.
    pub fn current() -> Option<Version> {
        Version::parse(env!("CARGO_PKG_VERSION"))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Why an update did not happen.
#[derive(Debug)]
pub enum Problem {
    /// The release host could not be reached, or answered with a failure.
    Unreachable(String),
    /// The host's limit for callers without an account was hit.
    RateLimited,
    /// Downloaded bytes did not match `SHA256SUMS.txt`.
    Mismatch(String),
    /// The release is not usable: no checksum line, an archive that does not
    /// open, or a binary that does not run here.
    Broken(String),
    /// Something on this machine failed.
    Local(String),
}

impl Problem {
    fn message(&self) -> String {
        match self {
            Problem::Unreachable(text) | Problem::Broken(text) | Problem::Local(text) => {
                text.clone()
            }
            Problem::RateLimited => {
                "the release host is limiting requests from this address. Try again later."
                    .to_string()
            }
            Problem::Mismatch(text) => format!("{text}. Nothing was installed."),
        }
    }

    fn code(&self) -> Code {
        match self {
            Problem::Unreachable(_) | Problem::RateLimited => Code::Transport,
            Problem::Mismatch(_) | Problem::Broken(_) | Problem::Local(_) => Code::Failed,
        }
    }
}

fn opted_out(global: &Global) -> bool {
    global.no_update || std::env::var_os("SPIDER_AGENT_NO_UPDATE").is_some_and(|v| !v.is_empty())
}

fn in_background() -> bool {
    std::env::var_os("SPIDER_AGENT_UPDATE_BACKGROUND").is_some_and(|v| !v.is_empty())
}

/// The line that tells someone how to update by hand.
fn manual(releases_page: &str) -> String {
    let asset = triple().map_or_else(
        || "the archive for this platform".to_string(),
        |triple| format!("spider-agent-<version>-{triple}.tar.gz"),
    );
    format!(
        "To update by hand, download {asset} and SHA256SUMS.txt from {releases_page}, check the archive with shasum -a 256 -c, and replace the binary."
    )
}

/// Everything that happens before the caller's command runs: a deferred
/// notice, an update staged earlier, and a background check if one is due.
///
/// Returns normally in every case but one: when a staged update was installed
/// and the new binary started in this process's place, this never returns.
pub fn before_command(global: &Global, command: Option<&Command>, log: Log) {
    if opted_out(global) || matches!(command, Some(Command::Update)) || in_background() {
        return;
    }
    if settings().is_none() || triple().is_none() {
        return;
    }
    let Ok(target) = Target::current() else {
        return;
    };
    let mut state = state::read();
    if state.notice.is_some() {
        print_notice(log);
    }
    if std::fs::symlink_metadata(target.staged_path()).is_ok() || state.staged.is_some() {
        apply_staged(&target, log);
        state = state::read();
    }
    if state::due(state.last_check, state::now(), CHECK_INTERVAL) {
        start_background_check(&target);
    }
}

/// Print and clear the deferred notice. Under the lock, so two runs starting
/// together print it once between them.
fn print_notice(log: Log) {
    let Some(_lock) = Lock::try_take() else {
        return;
    };
    let mut state = state::read();
    let Some(notice) = state.notice.take() else {
        return;
    };
    if notice.warning {
        log.failed(format!("update: {}", notice.text));
    } else {
        log.say(format!("update: {}", notice.text));
    }
    let _ = state::write(&state);
}

/// Install a staged update if it is for this binary, newer than it, and still
/// the bytes that were verified; then run the caller's command on it.
fn apply_staged(target: &Target, log: Log) {
    let Some(lock) = Lock::try_take() else {
        return;
    };
    let mut state = state::read();
    let record = state
        .staged
        .clone()
        .filter(|staged| staged.target == target.path);
    let staged_path = target.staged_path();
    let Some(record) = record else {
        // A file at the staged path that no check recorded is not trusted,
        // whoever put it there.
        if std::fs::symlink_metadata(&staged_path).is_ok() {
            let _ = std::fs::remove_file(&staged_path);
            log.note("update: removed a staged file no update check recorded");
        }
        return;
    };
    state.staged = None;
    if std::fs::symlink_metadata(&staged_path).is_err() {
        // Recorded, but gone: installed by another run, or cleaned up.
        let _ = state::write(&state);
        return;
    }
    let newer = Version::parse(&record.version)
        .zip(Version::current())
        .is_some_and(|(staged, current)| staged > current);
    let digest = archive::parse_hex(&record.sha256);
    let outcome = match (newer, digest) {
        (true, Some(digest)) => install::apply(target, &digest),
        _ => {
            let _ = std::fs::remove_file(&staged_path);
            Applied::Refused(String::new())
        }
    };
    let _ = state::write(&state);
    drop(lock);
    match outcome {
        Applied::Installed => {
            log.say(format!(
                "update: spider-agent {} replaced {}",
                record.version,
                env!("CARGO_PKG_VERSION")
            ));
            #[cfg(unix)]
            {
                let error = install::restart(target);
                log.failed(format!(
                    "update: could not start the new version ({error}). This run continues on {}.",
                    env!("CARGO_PKG_VERSION")
                ));
            }
        }
        Applied::Refused(reason) if !reason.is_empty() => {
            log.failed(format!(
                "update: {reason}, so it was deleted and nothing was installed."
            ));
        }
        Applied::Refused(_) => {}
        Applied::Failed(reason) => log.failed(format!("update: {reason}")),
    }
}

/// Stamp the check time and start the child. The stamp goes first and under
/// the lock, so runs that start together start one child between them, and a
/// child that fails still waits out the interval before the next one.
fn start_background_check(target: &Target) {
    let Some(_lock) = Lock::try_take() else {
        return;
    };
    let mut state = state::read();
    let now = state::now();
    if !state::due(state.last_check, now, CHECK_INTERVAL) {
        return;
    }
    state.last_check = now;
    state.outcome = None;
    // No state file means no throttle, and a check on every run. Rather than
    // that, a machine that cannot keep the file does not check.
    if state::write(&state).is_err() {
        return;
    }
    let _ = install::spawn_background_check(target);
}

/// `spider-agent update`.
pub async fn command(global: &Global, log: Log) -> Run<Code> {
    if in_background() {
        background().await;
        return Ok(Code::Ok);
    }
    if opted_out(global) {
        return Err(Failure::usage(format!(
            "self update is turned off by {OPT_OUT_ENV} or --no-update"
        )));
    }
    let Some(settings) = settings() else {
        return Err(Failure::usage(
            "a debug build does not update itself. Build with --release, or install a release.",
        ));
    };
    let Some(triple) = triple() else {
        return Err(Failure::new(
            Code::Failed,
            format!(
                "no release is built for this platform. {}",
                manual(RELEASES)
            ),
        ));
    };
    let Some(current) = Version::current() else {
        return Err(Failure::new(
            Code::Failed,
            "this binary's version cannot be read",
        ));
    };
    let failure =
        |problem: Problem| Failure::new(problem.code(), format!("update: {}", problem.message()));
    let target = Target::current().map_err(failure)?;
    let Some(_lock) = Lock::try_take() else {
        return Err(Failure::new(
            Code::Failed,
            "update: another spider-agent is updating right now. Try again in a minute.",
        ));
    };
    let releases = Releases::new(&settings).map_err(failure)?;
    let latest = releases.latest().await.map_err(failure)?;
    let mut state = state::read();
    state.last_check = state::now();
    if latest.version <= current {
        let _ = state::write(&state);
        log.say(format!(
            "update: spider-agent {current} is the newest release"
        ));
        return Ok(Code::Ok);
    }
    if let Some(reason) = install::not_ours(&target, &manual(&releases.page(&latest.tag))) {
        let _ = state::write(&state);
        return Err(Failure::new(
            Code::Output,
            format!("update: spider-agent {} is out. {reason}", latest.version),
        ));
    }
    let bytes = release::fetch_verified(&releases, &latest, triple)
        .await
        .map_err(failure)?;
    let version = latest.version.to_string();
    install::install(&target, &bytes, &version)
        .await
        .map_err(failure)?;
    // Whatever was staged is older than or equal to what is now installed.
    if state
        .staged
        .as_ref()
        .is_some_and(|staged| staged.target == target.path)
    {
        state.staged = None;
        let _ = std::fs::remove_file(target.staged_path());
    }
    let _ = state::write(&state);
    log.say(format!(
        "update: installed spider-agent {version} over {current} at {}",
        target.path.display()
    ));
    Ok(Code::Ok)
}

/// The detached child. Nobody reads its output, so what it has to say goes
/// into the state file for the next run.
async fn background() {
    let Some(_lock) = Lock::try_take() else {
        return;
    };
    let mut state = state::read();
    let outcome = check_and_stage(&mut state).await;
    state.outcome = Some(outcome);
    let _ = state::write(&state);
}

async fn check_and_stage(state: &mut State) -> String {
    let (Some(settings), Some(triple), Some(current)) = (settings(), triple(), Version::current())
    else {
        return "skipped: this build does not update itself".to_string();
    };
    let target = match Target::current() {
        Ok(target) => target,
        Err(problem) => return format!("skipped: {}", problem.message()),
    };
    let releases = match Releases::new(&settings) {
        Ok(releases) => releases,
        Err(problem) => return format!("skipped: {}", problem.message()),
    };
    // A rate limit, an outage or a missing release is not worth a line on
    // anyone's terminal. The next check is a day away either way.
    let latest: Release = match releases.latest().await {
        Ok(latest) => latest,
        Err(problem) => return format!("skipped: {}", problem.message()),
    };
    if latest.version <= current {
        return "up to date".to_string();
    }
    let version = latest.version.to_string();
    let already = state
        .staged
        .as_ref()
        .is_some_and(|staged| staged.target == target.path && staged.version == version)
        && target.staged_path().is_file();
    if already {
        return "staged".to_string();
    }
    if let Some(reason) = install::not_ours(&target, &manual(&releases.page(&latest.tag))) {
        if state.told.as_deref() != Some(version.as_str()) {
            state.notice = Some(Notice {
                text: format!("spider-agent {version} is out. {reason}"),
                warning: false,
            });
            state.told = Some(version);
        }
        return "skipped: not ours to replace".to_string();
    }
    let bytes = match release::fetch_verified(&releases, &latest, triple).await {
        Ok(bytes) => bytes,
        Err(problem @ (Problem::Mismatch(_) | Problem::Broken(_))) => {
            state.notice = Some(Notice {
                text: problem.message(),
                warning: true,
            });
            return format!("refused: {}", problem.message());
        }
        Err(problem) => return format!("skipped: {}", problem.message()),
    };
    match install::stage(&target, &bytes, &version).await {
        Ok(digest) => {
            state.staged = Some(Staged {
                version,
                sha256: archive::hex(&digest),
                target: target.path,
            });
            "staged".to_string()
        }
        Err(problem @ Problem::Broken(_)) => {
            state.notice = Some(Notice {
                text: format!("{}. Nothing was installed.", problem.message()),
                warning: true,
            });
            format!("refused: {}", problem.message())
        }
        Err(problem) => format!("skipped: {}", problem.message()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn versions_compare_as_numbers_and_refuse_anything_else() {
        let v = |text| Version::parse(text).unwrap();
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("0.4.0"), v("0.4.0"));
        for bad in [
            "0.4",
            "0.4.0.a",
            "0.4.0-rc.1",
            "v0.4.0",
            "",
            "0..1",
            "+1.2.3",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        assert!(Version::current().is_some());
    }
}
