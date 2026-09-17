//! Where the running binary is, whether it is ours to replace, and replacing
//! it.
//!
//! Every write lands in a fresh file in the same directory as the binary, so
//! the final step is a `rename(2)` within one filesystem. The binary at the
//! path is either the old one or the new one, never a mix, and a process
//! already running the old one keeps running it, because it holds the old
//! inode rather than the path.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::archive::{self, Digest32};
use super::Problem;

/// The mode an installed binary gets.
#[cfg(unix)]
const BINARY_MODE: u32 = 0o755;

/// The mode a binary has while it is still being written: its owner's alone.
#[cfg(unix)]
const WRITING_MODE: u32 = 0o700;

/// How long the new binary has to answer `--version` before it is judged
/// unable to run here.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many unique names are tried for a temporary file.
const TEMP_ATTEMPTS: u32 = 8;

/// The binary this process was started from.
#[derive(Debug, Clone)]
pub struct Target {
    /// Canonical, so a symlink on the `PATH` resolves to the file it names
    /// and the link itself is left alone.
    pub path: PathBuf,
    pub dir: PathBuf,
    pub name: String,
}

impl Target {
    /// The running binary, resolved.
    pub fn current() -> Result<Target, Problem> {
        let exe = std::env::current_exe()
            .and_then(|exe| exe.canonicalize())
            .map_err(|error| {
                Problem::Local(format!("could not tell where this binary is: {error}"))
            })?;
        let dir = exe
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| Problem::Local("the binary has no directory".to_string()))?;
        let name = exe
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .ok_or_else(|| Problem::Local("the binary's name is not text".to_string()))?;
        Ok(Target {
            path: exe,
            dir,
            name,
        })
    }

    /// Where a verified update waits for the next run: beside the binary, so
    /// installing it is a rename on the same filesystem.
    pub fn staged_path(&self) -> PathBuf {
        self.dir.join(format!(".{}.update", self.name))
    }
}

/// Why this install is not one to replace, as a line that says what to do
/// instead, or `None` when it is.
///
/// A binary under a cargo install root is left to cargo. Cargo records what
/// it installed, at which version, in `.crates.toml` and `.crates2.json` at
/// that root, and a binary swapped behind its back makes `cargo install`
/// believe the old version is still there. Homebrew and Nix installs are left
/// to them for the same reason. A directory this user cannot write is a
/// system install, and no update is attempted there at all.
pub fn not_ours(target: &Target, manual: &str) -> Option<String> {
    let cargo_root = target
        .dir
        .file_name()
        .is_some_and(|name| name == "bin")
        .then(|| target.dir.parent())
        .flatten();
    if let Some(root) = cargo_root {
        if root.join(".crates2.json").exists() || root.join(".crates.toml").exists() {
            return Some(format!(
                "{} was installed by cargo install, which keeps its own record of it. Run cargo install spider-agent-cli --locked to update it.",
                target.path.display()
            ));
        }
    }
    let text = target.path.to_string_lossy();
    if text.contains("/Cellar/") || text.starts_with("/nix/store/") {
        return Some(format!(
            "{} belongs to a package manager. Update it through that.",
            target.path.display()
        ));
    }
    match create_temp(&target.dir, &target.name) {
        Ok((file, path)) => {
            drop(file);
            let _ = std::fs::remove_file(path);
            None
        }
        Err(error) => Some(format!(
            "{} cannot be replaced by this user ({error}). {manual}",
            target.dir.display()
        )),
    }
}

/// A fresh file in `dir` under a name nothing else holds, owner only while it
/// is written. `create_new` means it never opens a file that is already
/// there, so it follows no planted symlink and truncates nobody else's write.
fn create_temp(dir: &Path, name: &str) -> std::io::Result<(std::fs::File, PathBuf)> {
    let pid = std::process::id();
    let mut last = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free name");
    for attempt in 0..TEMP_ATTEMPTS {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!(".{name}.{pid}.{nanos}.{attempt}.tmp"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(WRITING_MODE);
        }
        match options.open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

/// Write the bytes to a temporary file beside the target, synced and at the
/// installed mode, and return its path. The caller renames it.
fn write_temp(target: &Target, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let (mut file, path) = create_temp(&target.dir, &target.name)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .and_then(|()| set_binary_mode(&path));
    drop(file);
    match written {
        Ok(()) => Ok(path),
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            Err(error)
        }
    }
}

fn set_binary_mode(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(BINARY_MODE))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Rename, removing the source if the rename fails, then sync the directory
/// so the rename survives a crash. The directory sync is best effort: the
/// rename has happened whether or not it can be synced.
fn rename_into_place(from: &Path, to: &Path, dir: &Path) -> std::io::Result<()> {
    if let Err(error) = std::fs::rename(from, to) {
        let _ = std::fs::remove_file(from);
        return Err(error);
    }
    #[cfg(unix)]
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Run the new binary with `--version` and require it to name the version the
/// release was tagged with. This is the check that the bytes can run on this
/// machine at all: an asset built for the wrong platform fails here rather
/// than after it has replaced a working binary. The bytes have already
/// matched `SHA256SUMS.txt` by the time this runs.
async fn probe(path: &Path, version: &str) -> Result<(), Problem> {
    let mut child = std::process::Command::new(path)
        .arg("--version")
        .env(super::OPT_OUT_ENV, "1")
        .env_remove(super::BACKGROUND_ENV)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            Problem::Broken(format!(
                "the new binary does not start on this machine: {error}"
            ))
        })?;
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Problem::Broken(
                    "the new binary did not answer --version".to_string(),
                ));
            }
            Err(error) => {
                return Err(Problem::Broken(format!(
                    "the new binary could not be watched: {error}"
                )))
            }
        }
    };
    let mut printed = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut printed);
    }
    let expected = format!("spider-agent {version}");
    if status.success() && printed.trim() == expected {
        Ok(())
    } else {
        Err(Problem::Broken(format!(
            "the new binary answered --version with {:?} where {expected:?} was expected",
            printed.trim()
        )))
    }
}

/// Put verified bytes in place of the target now. For the foreground
/// `update` command, where the caller asked for exactly this.
pub async fn install(target: &Target, bytes: &[u8], version: &str) -> Result<(), Problem> {
    let temp = write_temp(target, bytes)
        .map_err(|error| Problem::Local(format!("could not write the new binary: {error}")))?;
    if let Err(problem) = probe(&temp, version).await {
        let _ = std::fs::remove_file(&temp);
        return Err(problem);
    }
    rename_into_place(&temp, &target.path, &target.dir).map_err(|error| {
        Problem::Local(format!(
            "could not move the new binary over {}: {error}",
            target.path.display()
        ))
    })
}

/// Put verified bytes at the staged path and return their digest, for the
/// state file to record. The target itself is not touched.
pub async fn stage(target: &Target, bytes: &[u8], version: &str) -> Result<Digest32, Problem> {
    let temp = write_temp(target, bytes)
        .map_err(|error| Problem::Local(format!("could not write the update: {error}")))?;
    if let Err(problem) = probe(&temp, version).await {
        let _ = std::fs::remove_file(&temp);
        return Err(problem);
    }
    rename_into_place(&temp, &target.staged_path(), &target.dir)
        .map_err(|error| Problem::Local(format!("could not stage the update: {error}")))?;
    Ok(archive::sha256(bytes))
}

/// What happened to a staged file on the way to being installed.
#[derive(Debug)]
pub enum Applied {
    /// It is now the binary.
    Installed,
    /// It is not a regular file, or its bytes are not the ones that were
    /// verified, and it has been deleted.
    Refused(String),
    /// It could not be moved into place.
    Failed(String),
}

/// Install the staged file if its bytes hash to `expected`.
///
/// The file is hashed as it is opened, not by path, and must be a regular
/// file rather than a link. A mismatch deletes it.
pub fn apply(target: &Target, expected: &Digest32) -> Applied {
    let staged = target.staged_path();
    let is_file = std::fs::symlink_metadata(&staged)
        .map(|meta| meta.file_type().is_file())
        .unwrap_or(false);
    if !is_file {
        let _ = std::fs::remove_file(&staged);
        return Applied::Refused("the staged update is not a regular file".to_string());
    }
    match hash_file(&staged) {
        Ok(actual) if actual == *expected => {}
        Ok(_) => {
            let _ = std::fs::remove_file(&staged);
            return Applied::Refused(
                "the staged update does not match the checksum recorded when it was verified"
                    .to_string(),
            );
        }
        Err(error) => return Applied::Failed(format!("could not read the staged update: {error}")),
    }
    if let Err(error) = set_binary_mode(&staged) {
        return Applied::Failed(format!("could not mark the update executable: {error}"));
    }
    match rename_into_place(&staged, &target.path, &target.dir) {
        Ok(()) => Applied::Installed,
        Err(error) => Applied::Failed(format!(
            "could not move the update over {}: {error}",
            target.path.display()
        )),
    }
}

fn hash_file(path: &Path) -> std::io::Result<Digest32> {
    let file = std::fs::File::open(path)?;
    let cap = u64::try_from(archive::MAX_UNPACKED_BYTES).unwrap_or(u64::MAX);
    let mut reader = file.take(cap);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or_default());
    }
    Ok(hasher.finalize().into())
}

/// Start the installed binary in place of this process, with the same
/// arguments, so the command the caller asked for runs on the new version.
/// Returns only if that could not be done, and then this process carries on.
#[cfg(unix)]
pub fn restart(target: &Target) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let argv0 = args.next();
    let mut command = std::process::Command::new(&target.path);
    command.args(args);
    if let Some(argv0) = argv0 {
        command.arg0(argv0);
    }
    command.exec()
}

/// Start a detached `spider-agent update` that checks and stages in the
/// background. Its standard streams are closed, so it cannot hold open a pipe
/// the caller is waiting on, and on Unix it runs in its own process group, so
/// a Ctrl-C meant for the command does not land on it. Nothing waits for it.
pub fn spawn_background_check(target: &Target) -> std::io::Result<()> {
    let mut command = std::process::Command::new(&target.path);
    command
        .arg("update")
        .env(super::BACKGROUND_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().map(drop)
}
