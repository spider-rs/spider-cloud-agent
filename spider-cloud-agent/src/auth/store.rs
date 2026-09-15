//! Finding a key that is already on this machine, and writing one back.
//!
//! The order is fixed: a key passed in wins, then `SPIDER_API_KEY`, then
//! `SPIDER_CLOUD_API_KEY`, then the operating system keychain when the `keyring`
//! feature is on, then `~/.spider/credentials`. The first one that holds a
//! non-empty value after trimming is the answer, so an empty variable does not
//! shadow a stored key.
//!
//! The file and the keychain slot are the ones the Spider command line tools
//! already use. The file is one trimmed line and nothing else, and the keychain
//! entry is the service and user name in [`KEYRING_SERVICE`] and
//! [`KEYRING_USER`]. Signing in with either tool signs you in for both.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::Error;
use crate::Result;

/// The environment variable read first when no key was passed in.
pub const API_KEY_ENV: &str = "SPIDER_API_KEY";

/// The environment variable read second, for callers already set up for the
/// other clients.
pub const API_KEY_ENV_ALT: &str = "SPIDER_CLOUD_API_KEY";

/// Where the command line tool keeps a key, relative to the home directory.
pub const CREDENTIALS_PATH: &str = ".spider/credentials";

/// The keychain service name, shared with the Spider Cloud command line tool.
pub const KEYRING_SERVICE: &str = "spider_client";

/// The keychain user name, shared with the Spider Cloud command line tool.
pub const KEYRING_USER: &str = "default";

/// Shown wherever a key would otherwise be printed.
const REDACTED: &str = "<redacted>";

/// The mode a credentials file is created with: the owner reads and writes, and
/// nobody else has any access at all.
#[cfg(unix)]
pub const FILE_MODE: u32 = 0o600;

/// The mode the `.spider` directory is created with.
#[cfg(unix)]
pub const DIR_MODE: u32 = 0o700;

/// Warning about a readable credentials file is worth saying once per process
/// and tiresome after that.
static WARNED_ABOUT_PERMISSIONS: AtomicBool = AtomicBool::new(false);

/// Where a key came from.
///
/// Useful in a sign in flow that wants to say which place it read, and it holds
/// no part of the key itself, so it is safe to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// Passed in by the caller.
    Explicit,
    /// Read from the named environment variable.
    Env(&'static str),
    /// Read from the operating system keychain.
    Keyring,
    /// Read from `~/.spider/credentials`.
    File,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Explicit => f.write_str("the key passed in"),
            Source::Env(name) => write!(f, "{name}"),
            Source::Keyring => f.write_str("the operating system keychain"),
            Source::File => write!(f, "~/{CREDENTIALS_PATH}"),
        }
    }
}

/// Where a key was written.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stored {
    /// Into the operating system keychain.
    Keyring,
    /// Into a file at this path.
    File(PathBuf),
}

/// An API key and the place it was found.
///
/// `Debug` prints the source and a placeholder. There is no `Display`, no
/// `Serialize` and no `AsRef<str>`: reading the value takes the deliberate step
/// of calling [`Credentials::expose`], which makes every place the key escapes
/// to greppable.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    key: String,
    source: Source,
}

impl fmt::Debug for Credentials {
    /// Written by hand so the key cannot reach a log line through a derived
    /// `Debug` on something that happens to hold one of these.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("key", &REDACTED)
            .field("source", &self.source)
            .finish()
    }
}

impl Credentials {
    /// The first key this machine offers, or `None` when it offers none.
    ///
    /// Nothing here logs what it found. The caller learns whether there was one
    /// and where it came from, and nothing else.
    pub fn resolve() -> Option<Credentials> {
        Credentials::resolve_with(None)
    }

    /// The same search, with a key the caller already holds taking precedence.
    ///
    /// A blank explicit key is treated as no key rather than as an answer, so a
    /// caller reading an unset variable of its own falls through to the rest of
    /// the order instead of failing.
    pub fn resolve_with(explicit: Option<&str>) -> Option<Credentials> {
        resolve_in(&Lookup {
            explicit,
            // Read as literals so the leak checker can see which variables a
            // published crate touches. The test below holds them to the
            // constants.
            env: [
                (API_KEY_ENV, std::env::var("SPIDER_API_KEY").ok()),
                (API_KEY_ENV_ALT, std::env::var("SPIDER_CLOUD_API_KEY").ok()),
            ],
            keyring: &keyring_read,
            home: home_dir(),
        })
    }

    /// The key itself.
    ///
    /// Everything that holds the returned string owns the problem of keeping it
    /// out of a log line.
    pub fn expose(&self) -> &str {
        &self.key
    }

    /// The key, taking ownership.
    pub fn into_key(self) -> String {
        self.key
    }

    /// Where it was found.
    pub fn source(&self) -> Source {
        self.source
    }

    /// Write a key where the next run and the command line tools will find it.
    ///
    /// The keychain is tried first. When there is no keychain, or it refuses,
    /// the key goes to `~/.spider/credentials`, which is created at
    /// [`FILE_MODE`] inside a directory created at [`DIR_MODE`] before the key
    /// is written into it, never after.
    ///
    /// Returns where it landed.
    pub fn store(key: &str) -> Result<Stored> {
        let key = key.trim();
        if key.is_empty() {
            return Err(Error::Auth(
                "refusing to store an empty api key".to_string(),
            ));
        }
        if keyring_write(key) {
            return Ok(Stored::Keyring);
        }
        let home =
            home_dir().ok_or_else(|| Error::Auth("no home directory to write to".to_string()))?;
        let path = write_key_file(&home, key)
            .map_err(|e| Error::Auth(format!("could not write ~/{CREDENTIALS_PATH}: {e}")))?;
        Ok(Stored::File(path))
    }

    /// Narrow an existing credentials file to owner only.
    ///
    /// Reading never does this on its own. A file whose permissions are wider
    /// than owner only has already been readable for as long as it has existed,
    /// and silently changing a mode the user chose hides that from them, so the
    /// crate warns and leaves it. Call this to fix it.
    ///
    /// Returns the path when a mode was changed, and `None` when there was
    /// nothing to change. Does nothing on platforms without Unix permissions.
    pub fn tighten_file_permissions() -> Result<Option<PathBuf>> {
        let home =
            home_dir().ok_or_else(|| Error::Auth("no home directory to read".to_string()))?;
        let path = home.join(CREDENTIALS_PATH);
        tighten(&path).map_err(|e| Error::Auth(format!("could not change the mode: {e}")))
    }
}

/// Every place a key can come from, gathered, so the order can be tested
/// without an environment variable, a keychain or a home directory.
struct Lookup<'a> {
    explicit: Option<&'a str>,
    env: [(&'static str, Option<String>); 2],
    keyring: &'a dyn Fn() -> Option<String>,
    home: Option<PathBuf>,
}

/// The resolution order itself, with every source injected.
fn resolve_in(lookup: &Lookup<'_>) -> Option<Credentials> {
    if let Some(key) = trimmed(lookup.explicit.map(str::to_string)) {
        return Some(Credentials {
            key,
            source: Source::Explicit,
        });
    }
    for (name, value) in &lookup.env {
        if let Some(key) = trimmed(value.clone()) {
            return Some(Credentials {
                key,
                source: Source::Env(name),
            });
        }
    }
    if let Some(key) = trimmed((lookup.keyring)()) {
        return Some(Credentials {
            key,
            source: Source::Keyring,
        });
    }
    let home = lookup.home.as_deref()?;
    let key = trimmed(read_key_file(&home.join(CREDENTIALS_PATH)))?;
    Some(Credentials {
        key,
        source: Source::File,
    })
}

/// Read the credentials file, warning once when anyone but the owner can read
/// it. The mode is left exactly as it was found.
fn read_key_file(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    if let Some(mode) = wide_permissions(path) {
        warn_wide_permissions(path, mode, &WARNED_ABOUT_PERMISSIONS);
    }
    Some(contents)
}

/// The mode of a credentials file that anyone but its owner can reach, or
/// `None` when the file is owner only or the platform has no such notion.
fn wide_permissions(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
        (mode & 0o077 != 0).then_some(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Say it once. The flag is a parameter so a test can watch the gate work
/// instead of racing the process wide one.
fn warn_wide_permissions(path: &Path, mode: u32, once: &AtomicBool) {
    if once.swap(true, Ordering::Relaxed) {
        return;
    }
    log::warn!(
        "{} is readable by more than its owner (mode {mode:o}). The api key in it has been \
         readable for as long as the file has existed. Narrow it with chmod 600, or call \
         Credentials::tighten_file_permissions.",
        path.display()
    );
}

/// Narrow a file to owner only, if it is wider and it exists.
fn tighten(path: &Path) -> std::io::Result<Option<PathBuf>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if wide_permissions(path).is_none() {
            return Ok(None);
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(FILE_MODE))?;
        Ok(Some(path.to_path_buf()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(None)
    }
}

/// Write the key under the given home directory, creating the directory and the
/// file with their final modes at the moment they are created.
///
/// The order matters. A file created at the default mode and narrowed
/// afterwards is world readable for the window in between, and that window is
/// long enough: it has already leaked a key in this repository's history.
fn write_key_file(home: &Path, key: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;

    let path = home.join(CREDENTIALS_PATH);
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent directory")
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(parent)?;
        // A directory that was already there keeps whatever mode it has. Only
        // the file is forced, because that is the one holding the key.
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(parent)?;

    // An existing file keeps its old mode through a truncating open, so narrow
    // it before anything is written into it.
    #[cfg(unix)]
    if path.exists() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(FILE_MODE))?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(&path)?;
    file.write_all(key.as_bytes())?;
    file.flush()?;
    Ok(path)
}

/// The home directory, which is the one the command line tool lands in too, so
/// both tools read and write the same file.
fn home_dir() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir()
}

/// The key in the keychain slot the command line tool uses.
fn keyring_read() -> Option<String> {
    #[cfg(feature = "keyring")]
    {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .ok()?
            .get_password()
            .ok()
    }
    #[cfg(not(feature = "keyring"))]
    {
        None
    }
}

/// Put the key in the keychain. False when there is no keychain to put it in,
/// which is the signal to fall back to the file.
fn keyring_write(key: &str) -> bool {
    #[cfg(feature = "keyring")]
    {
        match keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            Ok(entry) => entry.set_password(key).is_ok(),
            Err(_) => false,
        }
    }
    #[cfg(not(feature = "keyring"))]
    {
        let _ = key;
        false
    }
}

/// Trim, and treat whitespace as absence.
fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;
    use std::sync::atomic::AtomicUsize;

    const SECRET: &str = "sk-live-not-a-real-key-0123456789";

    /// A home directory of our own, removed when the test ends. No test in this
    /// file reads or writes the developer's real `~/.spider/credentials`.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(tag: &str) -> TempHome {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("spider-auth-{tag}-{unique}"));
            std::fs::create_dir_all(&path).unwrap();
            TempHome(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write_credentials(&self, contents: &str) -> PathBuf {
            let path = self.0.join(CREDENTIALS_PATH);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The two variables as the resolver sees them, with only the named ones
    /// set.
    fn env_of(pairs: &[(&str, &str)]) -> [(&'static str, Option<String>); 2] {
        let value = |wanted: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == wanted)
                .map(|(_, v)| v.to_string())
        };
        [
            (API_KEY_ENV, value(API_KEY_ENV)),
            (API_KEY_ENV_ALT, value(API_KEY_ENV_ALT)),
        ]
    }

    fn no_keyring() -> Option<String> {
        None
    }

    fn some_keyring() -> Option<String> {
        Some("from-the-keyring".to_string())
    }

    #[test]
    fn an_explicit_key_wins_over_every_other_source() {
        let home = TempHome::new("explicit");
        home.write_credentials("from-the-file");
        let env = env_of(&[(API_KEY_ENV, "from-the-env")]);

        let found = resolve_in(&Lookup {
            explicit: Some(SECRET),
            env,
            keyring: &some_keyring,
            home: Some(home.path().to_path_buf()),
        })
        .unwrap();

        assert_eq!(found.expose(), SECRET);
        assert_eq!(found.source(), Source::Explicit);
    }

    #[test]
    fn the_first_variable_wins_when_no_key_was_passed_in() {
        let home = TempHome::new("env-first");
        home.write_credentials("from-the-file");
        let env = env_of(&[(API_KEY_ENV, "first"), (API_KEY_ENV_ALT, "second")]);

        let found = resolve_in(&Lookup {
            explicit: None,
            env,
            keyring: &some_keyring,
            home: Some(home.path().to_path_buf()),
        })
        .unwrap();

        assert_eq!(found.expose(), "first");
        assert_eq!(found.source(), Source::Env(API_KEY_ENV));
    }

    #[test]
    fn the_second_variable_is_read_when_the_first_is_absent() {
        let env = env_of(&[(API_KEY_ENV_ALT, "second")]);

        let found = resolve_in(&Lookup {
            explicit: None,
            env,
            keyring: &no_keyring,
            home: None,
        })
        .unwrap();

        assert_eq!(found.expose(), "second");
        assert_eq!(found.source(), Source::Env(API_KEY_ENV_ALT));
    }

    #[test]
    fn an_empty_variable_does_not_shadow_a_later_source() {
        let env = env_of(&[(API_KEY_ENV, "   ")]);

        let found = resolve_in(&Lookup {
            explicit: None,
            env,
            keyring: &some_keyring,
            home: None,
        })
        .unwrap();

        assert_eq!(found.source(), Source::Keyring);
    }

    #[test]
    fn the_keychain_is_read_after_the_variables_and_before_the_file() {
        let home = TempHome::new("keyring-before-file");
        home.write_credentials("from-the-file");
        let env = env_of(&[]);

        let found = resolve_in(&Lookup {
            explicit: None,
            env,
            keyring: &some_keyring,
            home: Some(home.path().to_path_buf()),
        })
        .unwrap();

        assert_eq!(found.expose(), "from-the-keyring");
        assert_eq!(found.source(), Source::Keyring);
    }

    #[test]
    fn the_file_is_read_last_and_is_trimmed() {
        let home = TempHome::new("file-last");
        home.write_credentials("  from-the-file\n");
        let env = env_of(&[]);

        let found = resolve_in(&Lookup {
            explicit: None,
            env,
            keyring: &no_keyring,
            home: Some(home.path().to_path_buf()),
        })
        .unwrap();

        assert_eq!(found.expose(), "from-the-file");
        assert_eq!(found.source(), Source::File);
    }

    #[test]
    fn nothing_anywhere_is_not_an_error_it_is_no_key() {
        let home = TempHome::new("empty");
        let env = env_of(&[]);

        assert!(resolve_in(&Lookup {
            explicit: None,
            env,
            keyring: &no_keyring,
            home: Some(home.path().to_path_buf()),
        })
        .is_none());
    }

    #[test]
    fn the_variable_names_match_the_literals_that_are_actually_read() {
        assert_eq!(API_KEY_ENV, "SPIDER_API_KEY");
        assert_eq!(API_KEY_ENV_ALT, "SPIDER_CLOUD_API_KEY");
    }

    #[test]
    fn credentials_never_print_the_key() {
        let found = Credentials {
            key: SECRET.to_string(),
            source: Source::File,
        };
        let printed = format!("{found:?}");
        assert!(!printed.contains(SECRET), "{printed}");
        assert!(printed.contains(REDACTED), "{printed}");
    }

    #[test]
    fn a_source_prints_no_part_of_a_key() {
        for source in [
            Source::Explicit,
            Source::Env(API_KEY_ENV),
            Source::Keyring,
            Source::File,
        ] {
            let printed = format!("{source:?} {source}");
            assert!(!printed.contains(SECRET), "{printed}");
        }
    }

    #[test]
    fn stored_prints_no_part_of_a_key() {
        let printed = format!("{:?}", Stored::File(PathBuf::from("/tmp/x")));
        assert!(!printed.contains(SECRET), "{printed}");
        assert!(!format!("{:?}", Stored::Keyring).contains(SECRET));
    }

    #[test]
    fn storing_an_empty_key_is_refused() {
        assert!(matches!(Credentials::store("   "), Err(Error::Auth(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_written_file_is_owner_only_inside_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let home = TempHome::new("modes");
        let path = write_key_file(home.path(), SECRET).unwrap();

        let file = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file, FILE_MODE, "file mode {file:o}");

        let dir = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir, DIR_MODE, "directory mode {dir:o}");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), SECRET);
    }

    #[cfg(unix)]
    #[test]
    fn rewriting_a_wide_file_narrows_it_before_the_key_goes_in() {
        use std::os::unix::fs::PermissionsExt;

        let home = TempHome::new("rewrite");
        let path = home.write_credentials("old");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_key_file(home.path(), SECRET).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, FILE_MODE, "mode {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn a_wide_file_is_reported_and_left_alone() {
        use std::os::unix::fs::PermissionsExt;

        let home = TempHome::new("wide");
        let path = home.write_credentials(SECRET);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(wide_permissions(&path), Some(0o644));

        let before = WARNINGS.load(Ordering::Relaxed);
        let flag = AtomicBool::new(false);
        install_counting_logger();
        warn_wide_permissions(&path, 0o644, &flag);
        warn_wide_permissions(&path, 0o644, &flag);
        let warned = WARNINGS.load(Ordering::Relaxed) - before;
        assert_eq!(warned, 1, "warned {warned} times, wanted once");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "the mode was changed to {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn an_owner_only_file_is_not_reported() {
        let home = TempHome::new("narrow");
        let path = home.write_credentials(SECRET);
        tighten(&path).unwrap();
        assert_eq!(wide_permissions(&path), None);
    }

    #[cfg(unix)]
    #[test]
    fn tightening_reports_the_path_it_changed_and_then_has_nothing_to_do() {
        use std::os::unix::fs::PermissionsExt;

        let home = TempHome::new("tighten");
        let path = home.write_credentials(SECRET);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();

        assert_eq!(tighten(&path).unwrap(), Some(path.clone()));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, FILE_MODE, "mode {mode:o}");
        assert_eq!(tighten(&path).unwrap(), None);
    }

    /// Counts warnings so the once gate can be observed. Only the wide
    /// permissions test emits one, so the count is not raced.
    static WARNINGS: AtomicUsize = AtomicUsize::new(0);
    static LOGGER_INSTALLED: std::sync::Once = std::sync::Once::new();

    struct Counting;

    static COUNTING: Counting = Counting;

    impl log::Log for Counting {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            if record.level() == log::Level::Warn {
                WARNINGS.fetch_add(1, Ordering::Relaxed);
            }
        }
        fn flush(&self) {}
    }

    fn install_counting_logger() {
        LOGGER_INSTALLED.call_once(|| {
            if log::set_logger(&COUNTING).is_ok() {
                log::set_max_level(log::LevelFilter::Warn);
            }
        });
    }
}
