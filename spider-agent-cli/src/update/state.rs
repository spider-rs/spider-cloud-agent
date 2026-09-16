//! What the update check remembers between runs, and the lock file that keeps
//! two processes from working on it at once.
//!
//! Both live in `~/.spider`, beside the credentials file, and are written the
//! way that file is: owner only from the moment they exist, and replaced whole
//! by a rename rather than rewritten in place.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use spider_cloud_agent::auth::store;

/// The state file's name inside `~/.spider`.
pub const STATE_FILE: &str = "update.json";

/// The lock file's name inside `~/.spider`.
pub const LOCK_FILE: &str = "update.lock";

/// The most a state file may weigh and still be read.
const MAX_STATE_BYTES: u64 = 64 * 1024;

/// A lock older than this belongs to a process that died holding it. A
/// background check gives up on the network after two minutes a request, so a
/// live holder never gets near it.
const STALE_LOCK: Duration = Duration::from_secs(15 * 60);

/// Everything the update check keeps.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    /// When a check last started, in seconds since the epoch.
    #[serde(default)]
    pub last_check: u64,
    /// A binary that was downloaded, verified and staged, waiting for the next
    /// run to put it in place.
    #[serde(default)]
    pub staged: Option<Staged>,
    /// A line for the next run to print on stderr, once.
    #[serde(default)]
    pub notice: Option<Notice>,
    /// The version the "update this yourself" note was last given for, so it
    /// is said once per release and not once a day.
    #[serde(default)]
    pub told: Option<String>,
    /// How the last background check ended, in a few words. Read by tests and
    /// by a person wondering what happened; nothing branches on it.
    #[serde(default)]
    pub outcome: Option<String>,
}

/// A staged binary, and what makes it trusted.
///
/// The digest is of the binary as it was written after the archive it came
/// from matched `SHA256SUMS.txt`. The apply step hashes the staged file again
/// and installs it only if the two agree, so a file put at the staged path by
/// anything other than the check is deleted, not installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Staged {
    pub version: String,
    pub sha256: String,
    /// The binary this update is for, as a canonical path.
    pub target: PathBuf,
}

/// A deferred message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notice {
    pub text: String,
    /// Printed even under `--quiet`, because it reports refused bytes.
    #[serde(default)]
    pub warning: bool,
}

/// Seconds since the epoch, or zero on a clock set before it.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Whether a check is due. A last check in the future means the clock moved
/// back, and that counts as due rather than as a long wait.
pub fn due(last_check: u64, now: u64, interval: Duration) -> bool {
    last_check > now || now - last_check >= interval.as_secs()
}

fn path(name: &str) -> Option<PathBuf> {
    store::spider_dir().map(|dir| dir.join(name))
}

/// The state, or an empty one when there is none or it cannot be read. A
/// damaged file costs one early check and nothing else.
pub fn read() -> State {
    use std::io::Read;
    let Some(path) = path(STATE_FILE) else {
        return State::default();
    };
    let Ok(file) = std::fs::File::open(path) else {
        return State::default();
    };
    let mut text = String::new();
    if file
        .take(MAX_STATE_BYTES)
        .read_to_string(&mut text)
        .is_err()
    {
        return State::default();
    }
    serde_json::from_str(&text).unwrap_or_default()
}

/// Replace the state file.
pub fn write(state: &State) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    store::write_private_file(STATE_FILE, &text).map(|_| ())
}

/// Held while a process reads and rewrites the state, stages a download, or
/// installs one. Dropping it removes the file.
///
/// Locks in the `Mutex` sense are disallowed in this workspace, and would not
/// help anyway: the processes racing here are separate `spider-agent` runs,
/// not threads. The file is created with `O_EXCL`, which the kernel makes
/// atomic, so exactly one process gets it and every other one is told the
/// file exists. Nobody waits for it: a run that finds it held skips the
/// update work, because that work is optional and the command is not.
///
/// A holder that died leaves the file behind, so one older than
/// [`STALE_LOCK`] is removed and taken again. Two processes that both find it
/// stale can in principle both take it. What they then do is still safe,
/// because every file they produce is written under a unique name and renamed
/// into place whole, and the staged binary is checked against its recorded
/// digest before it is installed.
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    /// The lock, or `None` when another process holds it or there is nowhere
    /// to put it.
    pub fn try_take() -> Option<Lock> {
        let dir = store::ensure_spider_dir().ok()?;
        let path = dir.join(LOCK_FILE);
        match create(&path) {
            Ok(()) => return Some(Lock { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        let stale = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_LOCK);
        if !stale {
            return None;
        }
        std::fs::remove_file(&path).ok()?;
        create(&path).ok().map(|()| Lock { path })
    }
}

fn create(path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(store::FILE_MODE);
    }
    let mut file = options.open(path)?;
    // The holder's pid, for a person looking at a lock that will not go away.
    let _ = writeln!(file, "{}", std::process::id());
    Ok(())
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_throttle_waits_out_the_interval_and_forgives_a_clock_moved_back() {
        let day = Duration::from_secs(86_400);
        assert!(due(0, 1_000_000, day));
        assert!(!due(1_000_000, 1_000_000 + 3_600, day));
        assert!(due(1_000_000, 1_000_000 + 86_400, day));
        assert!(due(2_000_000, 1_000_000, day));
    }
}
