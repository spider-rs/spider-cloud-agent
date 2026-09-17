//! How the optimizer runs, from the flags and from the file this machine keeps.
//!
//! The optimizer is a second decision layer. After the router picks the mode,
//! the pool and the wait, it lists the valid edits to the request, scores each
//! one against leaving the request alone, and writes at most one set that
//! clears the gate. It never edits a field the caller set, it only touches the
//! first attempt, and a request that fixes the mode, the pool or the country is
//! not passed to it at all.
//!
//! Nothing here fetches a page. `~/.spider/optimize.json` holds the mode and
//! the path to a weights file, and is written the way the router file is
//! written: owner only from the moment it exists, and replaced whole. It holds
//! no key, and neither does a weights file.

use serde::{Deserialize, Serialize};

use spider_cloud_agent::auth::store;

use crate::cli::{Global, Mode as CliMode};
use crate::exit::{Code, Failure, Run};

/// Where the settings live, relative to the home directory.
pub const OPTIMIZE_PATH: &str = ".spider/optimize.json";

/// The file name inside `~/.spider`.
const OPTIMIZE_FILE: &str = "optimize.json";

/// The most the settings file may weigh and still be read. It holds a mode and
/// a path.
pub const MAX_OPTIMIZE_FILE_BYTES: usize = 8 * 1024;

/// The most a weights file may weigh. The reader refuses more than this as
/// well, and stopping here keeps a large file off the heap first.
#[cfg(feature = "optimize")]
const MAX_MODEL_BYTES: u64 = 2_000_000;

/// The environment variable that skips the optimizer, set to any value.
pub const NO_OPTIMIZE_ENV: &str = "SPIDER_AGENT_NO_OPTIMIZE";

/// The modes the file may name.
pub const MODES: &[&str] = &["off", "shadow", "apply"];

/// What a run does with the optimizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Build no optimizer. The request goes out as the router left it.
    Off,
    /// Score and record, and send the request unchanged.
    Shadow,
    /// Write the chosen edits onto the fields the caller left unset.
    Apply,
}

impl Mode {
    /// The name the flag and the file use.
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Shadow => "shadow",
            Mode::Apply => "apply",
        }
    }

    /// The mode a stored name asks for, or `None` when it is not one.
    fn parse(name: &str) -> Option<Mode> {
        match name {
            "off" => Some(Mode::Off),
            "shadow" => Some(Mode::Shadow),
            "apply" => Some(Mode::Apply),
            _ => None,
        }
    }
}

/// The mode a `--mode` value names, when it names one of these.
///
/// The fetch modes and the router modes share the type and never parse on
/// either optimizer flag.
pub fn chosen(mode: Option<CliMode>) -> Option<Mode> {
    mode.and_then(|mode| match mode {
        CliMode::Off => Some(Mode::Off),
        CliMode::Shadow => Some(Mode::Shadow),
        CliMode::Apply => Some(Mode::Apply),
        CliMode::Http | CliMode::Smart | CliMode::Browser | CliMode::Fallback | CliMode::First => {
            None
        }
    })
}

/// The settings kept on this machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredOptimize {
    /// off, shadow or apply. Absent means off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The weights file to score with, when one was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl StoredOptimize {
    /// Refuse a mode this version does not have, before it is written or used.
    pub fn validate(&self) -> Result<(), String> {
        match &self.mode {
            Some(name) if Mode::parse(name).is_none() => Err(format!(
                "{name} is not an optimizer mode. It is one of {}",
                MODES.join(", ")
            )),
            _ => Ok(()),
        }
    }

    /// The mode it names, or off.
    pub fn mode(&self) -> Mode {
        self.mode
            .as_deref()
            .and_then(Mode::parse)
            .unwrap_or(Mode::Off)
    }
}

/// `~/.spider/optimize.json`, or `None` when there is no home directory.
pub fn path() -> Option<std::path::PathBuf> {
    store::spider_dir().map(|dir| dir.join(OPTIMIZE_FILE))
}

/// The stored settings, or `None` when there is no file.
///
/// A file that is there and cannot be read is a failure rather than no
/// settings, for the reason the stored router is: an optimizer the caller
/// thinks is on and is not changes what every page costs.
pub fn load() -> Run<Option<StoredOptimize>> {
    match path() {
        Some(path) => load_from(&path),
        None => Ok(None),
    }
}

/// The settings stored at a path, or `None` when there is no file.
pub fn load_from(path: &std::path::Path) -> Run<Option<StoredOptimize>> {
    use std::io::Read;

    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unusable(Code::Output, format!("could not open it: {e}"))),
    };
    let mut bytes = Vec::new();
    file.take(MAX_OPTIMIZE_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| unusable(Code::Output, format!("could not read it: {e}")))?;
    if bytes.len() > MAX_OPTIMIZE_FILE_BYTES {
        return Err(unusable(
            Code::Usage,
            format!("it is larger than {MAX_OPTIMIZE_FILE_BYTES} bytes"),
        ));
    }
    let stored: StoredOptimize = serde_json::from_slice(&bytes)
        .map_err(|e| unusable(Code::Usage, format!("it is not JSON this tool wrote: {e}")))?;
    stored
        .validate()
        .map_err(|message| unusable(Code::Usage, message))?;
    Ok(Some(stored))
}

/// Write the settings, owner only, replacing what was there. Returns the path.
pub fn store_settings(stored: &StoredOptimize) -> Run<std::path::PathBuf> {
    stored
        .validate()
        .map_err(|message| Failure::usage(format!("{message}. Nothing was stored.")))?;
    let bytes = serde_json::to_vec_pretty(stored)
        .map_err(|error| Failure::output(format!("the settings could not be encoded: {error}")))?;
    store::write_private_file(OPTIMIZE_FILE, &bytes)
        .map_err(|error| Failure::output(format!("could not write ~/{OPTIMIZE_PATH}: {error}")))
}

/// Delete the settings. True when there were some to delete.
pub fn clear() -> Run<bool> {
    let Some(path) = path() else {
        return Ok(false);
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Failure::output(format!(
            "could not delete ~/{OPTIMIZE_PATH}: {e}"
        ))),
    }
}

/// A stored file that cannot be used, under the code its failure belongs to and
/// the way past it.
fn unusable(code: Code, why: String) -> Failure {
    Failure::new(
        code,
        format!(
            "~/{OPTIMIZE_PATH} cannot be used: {why}. Replace it with spider-agent optimize set, remove it with spider-agent optimize clear, or pass --no-optimize"
        ),
    )
}

/// Whether this run leaves the optimizer out.
pub fn skipped(global: &Global) -> bool {
    global.no_optimize || std::env::var_os(NO_OPTIMIZE_ENV).is_some_and(|v| !v.is_empty())
}

/// What one run does with the optimizer, flags over file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// What to do with a pick.
    pub mode: Mode,
    /// The weights file to score with, when one was named.
    pub model: Option<String>,
}

impl Settings {
    /// The settings a run with no optimizer has.
    pub const fn off() -> Settings {
        Settings {
            mode: Mode::Off,
            model: None,
        }
    }
}

/// The settings for this run: the flags, then the file, then off.
///
/// `--no-optimize` and the environment variable win over both, so a run can be
/// taken back to what it does with no optimizer without editing the file.
pub fn settings(global: &Global) -> Run<Settings> {
    if skipped(global) {
        return Ok(Settings::off());
    }
    let stored = if global.optimize.is_some() && global.optimize_model.is_some() {
        // Both flags were given, so the file changes nothing about this run and
        // a damaged one has no say in it.
        None
    } else {
        load()?
    };
    Ok(resolve(
        chosen(global.optimize),
        global.optimize_model.clone(),
        stored,
    ))
}

/// The flags over the file, with off for what neither names.
fn resolve(mode: Option<Mode>, model: Option<String>, stored: Option<StoredOptimize>) -> Settings {
    Settings {
        mode: mode
            .or_else(|| stored.as_ref().map(StoredOptimize::mode))
            .unwrap_or(Mode::Off),
        model: model.or_else(|| stored.and_then(|stored| stored.model)),
    }
}

/// An optimizer built from the settings, and the line that says so.
#[cfg(feature = "optimize")]
#[derive(Debug)]
pub struct Ready {
    /// Handed to the client.
    pub optimizer: spider_cloud_agent::optimize::Optimizer,
    /// One line for stderr, naming the mode and the weights.
    pub note: String,
}

/// The optimizer these settings ask for, or `None` for a run without one.
///
/// The weights are read here rather than at the first page, so a file that
/// cannot be used fails the run before anything is spent.
#[cfg(feature = "optimize")]
pub fn build(settings: &Settings) -> Run<Option<Ready>> {
    use spider_cloud_agent::optimize::{ApplyMode, Gate, NoModel, Optimizer};

    let apply = match settings.mode {
        Mode::Off => return Ok(None),
        Mode::Shadow => ApplyMode::Shadow,
        Mode::Apply => ApplyMode::Apply,
    };
    let mode = settings.mode.as_str();
    let Some(path) = &settings.model else {
        return Ok(Some(Ready {
            optimizer: Optimizer::new(NoModel, Gate::default(), apply),
            note: format!(
                "optimizer {mode}: no weights, so it abstains and every request goes out as it is. Name a weights file with --optimize-model."
            ),
        }));
    };
    let weights = weights(path)?;
    let version = weights.version().0;
    Ok(Some(Ready {
        optimizer: Optimizer::new(weights, Gate::default(), apply),
        note: format!("optimizer {mode}: weights version {version} from {path}"),
    }))
}

/// The weights at a path, checked before they score anything.
#[cfg(feature = "optimize")]
fn weights(path: &str) -> Run<spider_optimize::Compact> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .map_err(|e| Failure::usage(format!("could not open the weights in {path}: {e}")))?;
    let mut bytes = Vec::new();
    file.take(MAX_MODEL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::usage(format!("could not read the weights in {path}: {e}")))?;
    spider_optimize::Compact::from_bytes(&bytes)
        .map_err(|e| Failure::usage(format!("the weights in {path} were refused: {e}")))
}

/// Said when the binary was built without the optimizer.
#[cfg(not(feature = "optimize"))]
pub fn build(settings: &Settings) -> Run<Option<()>> {
    match settings.mode {
        Mode::Off => Ok(None),
        mode => Err(Failure::usage(format!(
            "--optimize {} needs the optimizer, and this binary was built without it. Build spider-agent-cli with the optimize feature, or run with --optimize off.",
            mode.as_str()
        ))),
    }
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    use std::path::PathBuf;

    /// A directory of our own, so no test here reads or writes the
    /// developer's `~/.spider`.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("spider-optimize-{tag}-{unique}"));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn write(&self, contents: &[u8]) -> PathBuf {
            let path = self.0.join(OPTIMIZE_FILE);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn stored(mode: &str, model: Option<&str>) -> StoredOptimize {
        StoredOptimize {
            mode: Some(mode.to_string()),
            model: model.map(str::to_string),
        }
    }

    #[test]
    fn a_flag_beats_the_file_and_the_file_beats_off() {
        // Neither: off, and no weights.
        assert_eq!(resolve(None, None, None), Settings::off());
        // The file alone.
        let from_file = resolve(None, None, Some(stored("shadow", Some("a.bin"))));
        assert_eq!(from_file.mode, Mode::Shadow);
        assert_eq!(from_file.model.as_deref(), Some("a.bin"));
        // The flags, over the same file.
        let from_flags = resolve(
            Some(Mode::Apply),
            Some("b.bin".to_string()),
            Some(stored("shadow", Some("a.bin"))),
        );
        assert_eq!(from_flags.mode, Mode::Apply);
        assert_eq!(from_flags.model.as_deref(), Some("b.bin"));
        // One flag leaves the other field to the file.
        let mixed = resolve(Some(Mode::Off), None, Some(stored("apply", Some("a.bin"))));
        assert_eq!(mixed.mode, Mode::Off);
        assert_eq!(mixed.model.as_deref(), Some("a.bin"));
    }

    #[test]
    fn a_stored_file_that_cannot_be_used_names_the_way_past_it() {
        let dir = TempDir::new("damaged");
        let path = dir.write(b"{ not json");
        let failed = load_from(&path).expect_err("a failure");
        assert_eq!(failed.code, Code::Usage);
        assert!(failed.message.contains("optimize clear"), "{failed}");

        let path = dir.write(br#"{"mode": "on"}"#);
        let failed = load_from(&path).expect_err("a failure");
        assert_eq!(failed.code, Code::Usage);
        assert!(failed.message.contains("shadow"), "{failed}");

        let path = dir.write(&vec![b' '; MAX_OPTIMIZE_FILE_BYTES + 1]);
        let failed = load_from(&path).expect_err("a failure");
        assert_eq!(failed.code, Code::Usage);
        assert!(failed.message.contains("larger than"), "{failed}");
    }

    #[test]
    fn a_file_that_is_not_there_is_no_settings_rather_than_a_failure() {
        let dir = TempDir::new("missing");
        let path = dir.0.join(OPTIMIZE_FILE);
        assert_eq!(load_from(&path).unwrap(), None);
    }

    #[test]
    fn a_file_written_by_this_tool_reads_back() {
        let dir = TempDir::new("round-trip");
        let written = stored("apply", Some("weights.bin"));
        let path = dir.write(&serde_json::to_vec_pretty(&written).unwrap());
        assert_eq!(load_from(&path).unwrap(), Some(written));
    }

    #[cfg(feature = "optimize")]
    #[test]
    fn off_builds_no_optimizer() {
        assert!(build(&Settings::off()).unwrap().is_none());
    }

    #[cfg(feature = "optimize")]
    #[test]
    fn apply_without_weights_abstains_and_says_so() {
        use spider_cloud_agent::optimize::ApplyMode;

        let settings = Settings {
            mode: Mode::Apply,
            model: None,
        };
        let ready = build(&settings).unwrap().expect("an optimizer");
        assert_eq!(ready.optimizer.mode(), ApplyMode::Apply);
        // No weights means the scorer abstains, and the line says the request
        // goes out as it is rather than leaving the caller to work it out.
        assert!(ready.note.contains("no weights"), "{}", ready.note);
        assert!(ready.note.contains("goes out as it is"), "{}", ready.note);

        let shadow = build(&Settings {
            mode: Mode::Shadow,
            model: None,
        })
        .unwrap()
        .expect("an optimizer");
        assert_eq!(shadow.optimizer.mode(), ApplyMode::Shadow);
    }

    #[cfg(feature = "optimize")]
    #[test]
    fn weights_that_cannot_be_used_stop_the_run_before_a_page_is_paid_for() {
        let dir = TempDir::new("weights");
        let path = dir.0.join("weights.bin");
        std::fs::write(&path, b"not an artifact").unwrap();
        let settings = Settings {
            mode: Mode::Shadow,
            model: Some(path.display().to_string()),
        };
        let failed = build(&settings).expect_err("a failure");
        assert_eq!(failed.code, Code::Usage);
        assert!(failed.message.contains("refused"), "{failed}");

        let missing = Settings {
            mode: Mode::Shadow,
            model: Some(dir.0.join("nothing.bin").display().to_string()),
        };
        assert_eq!(build(&missing).expect_err("a failure").code, Code::Usage);
    }

    #[test]
    fn a_mode_survives_the_round_trip_through_its_name() {
        for mode in [Mode::Off, Mode::Shadow, Mode::Apply] {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
            assert!(MODES.contains(&mode.as_str()));
        }
        assert_eq!(Mode::parse("on"), None);
    }

    #[test]
    fn a_stored_mode_this_version_does_not_have_is_refused() {
        let stored = StoredOptimize {
            mode: Some("on".to_string()),
            model: None,
        };
        let message = stored.validate().expect_err("a refusal");
        assert!(message.contains("shadow"), "{message}");
        // And it is off rather than anything else until it is fixed.
        assert_eq!(stored.mode(), Mode::Off);
    }

    #[test]
    fn nothing_stored_is_off() {
        let stored = StoredOptimize::default();
        assert_eq!(stored.mode(), Mode::Off);
        assert_eq!(serde_json::to_string(&stored).unwrap(), "{}");
        assert!(stored.validate().is_ok());
    }

    #[test]
    fn the_stored_file_holds_only_what_was_set() {
        let stored = StoredOptimize {
            mode: Some("shadow".to_string()),
            model: None,
        };
        let text = serde_json::to_string(&stored).unwrap();
        assert_eq!(text, r#"{"mode":"shadow"}"#);
    }
}
