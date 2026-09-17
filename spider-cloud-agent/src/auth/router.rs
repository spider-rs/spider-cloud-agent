//! A provider fallback kept on this machine, for every request that did not
//! name one.
//!
//! The request parameter `router` puts an outside scraping provider behind the
//! service's own fetch (`fallback`), in front of it (`first`), or out of the
//! request (`off`), and `provider_options` carries vendor settings for routes
//! your own key pays for. Setting both on every operation is repetitive, and
//! the token in it is a key. This module keeps one copy in
//! `~/.spider/router.json`, written the way the credentials file is written:
//! owner only from the moment it exists, and replaced whole.
//!
//! A client built with [`crate::SpiderBuilder::stored_router`] reads the file
//! once, and every page operation then fills in `router` and
//! `provider_options` where the caller left them unset. A caller who set
//! `router` gets exactly what they set, `mode: off` included.
//!
//! Nothing here prints a key. `Debug` on [`StoredRouter`] is written by hand,
//! and no error names a value, only the field it came from.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::error::{AuthCause, Error};
use crate::params::Router;
use crate::Result;

use super::store;

/// Where the stored router lives, relative to the home directory.
pub const ROUTER_PATH: &str = ".spider/router.json";

/// The file name inside `~/.spider`.
const ROUTER_FILE: &str = "router.json";

/// The most a router file may hold and still be read. A router with every
/// provider's keys and options is a few kilobytes.
pub const MAX_ROUTER_FILE_BYTES: usize = 64 * 1024;

/// The modes the service reads.
pub const MODES: &[&str] = &["fallback", "first", "off"];

/// The funding rules the service reads.
pub const FUNDINGS: &[&str] = &["any", "own"];

/// Shown wherever a key would otherwise be printed.
const REDACTED: &str = "<redacted>";

/// Once per process, as for the credentials file.
static WARNED_ABOUT_PERMISSIONS: AtomicBool = AtomicBool::new(false);

/// A `router` parameter and its `provider_options`, as stored.
///
/// `Debug` prints the mode, the provider, the funding rule, the credential
/// names and the provider names under the options. The token and every
/// credential value print as `<redacted>`, and no option value is printed.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredRouter {
    /// The `router` parameter every request without one receives.
    #[serde(default)]
    pub router: Router,
    /// The `provider_options` every request without them receives, keyed by
    /// provider name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<BTreeMap<String, serde_json::Value>>,
}

impl fmt::Debug for StoredRouter {
    /// Written by hand so a token cannot reach a log line through a derived
    /// `Debug` on something that holds one of these.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let credentials = self.router.credentials.as_ref().map(|held| {
            held.keys()
                .map(|name| (name.as_str(), REDACTED))
                .collect::<BTreeMap<_, _>>()
        });
        let options = self
            .provider_options
            .as_ref()
            .map(|held| held.keys().map(String::as_str).collect::<Vec<_>>());
        f.debug_struct("StoredRouter")
            .field("mode", &self.router.mode)
            .field("provider", &self.router.provider)
            .field("token", &self.router.token.as_ref().map(|_| REDACTED))
            .field("credentials", &credentials)
            .field("funding", &self.router.funding)
            .field("provider_options", &options)
            .finish()
    }
}

/// Why a router cannot be stored or used.
///
/// Every message names the field and never the value in it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RouterConfigError {
    /// `mode` is not one of [`MODES`].
    #[error("router.mode has to be fallback, first or off")]
    Mode,
    /// `funding` is not one of [`FUNDINGS`].
    #[error("router.funding has to be any or own")]
    Funding,
    /// `mode` is `first` and no provider is named.
    #[error("router.mode first needs router.provider")]
    FirstNeedsProvider,
    /// `mode` is `first` and there is no key for the provider.
    #[error(
        "router.mode first needs router.token, or a router.credentials entry for the provider"
    )]
    FirstNeedsKey,
    /// `provider` is present and blank.
    #[error("router.provider is empty")]
    EmptyProvider,
    /// `token` is present and blank.
    #[error("router.token is empty")]
    EmptyToken,
    /// A credential is present and blank. Holds the credential's name.
    #[error("router.credentials.{0} is empty")]
    EmptyCredential(String),
}

impl From<RouterConfigError> for Error {
    fn from(error: RouterConfigError) -> Error {
        Error::Config(error.to_string())
    }
}

impl StoredRouter {
    /// Check the values the service reads as a closed set, and that `first`
    /// has something to call.
    ///
    /// An unknown provider name is allowed, because the service adds providers
    /// without a release of this crate.
    pub fn validate(&self) -> std::result::Result<(), RouterConfigError> {
        let router = &self.router;
        if let Some(mode) = &router.mode {
            if !MODES.contains(&mode.as_str()) {
                return Err(RouterConfigError::Mode);
            }
        }
        if let Some(funding) = &router.funding {
            if !FUNDINGS.contains(&funding.as_str()) {
                return Err(RouterConfigError::Funding);
            }
        }
        if router.provider.as_deref().is_some_and(blank) {
            return Err(RouterConfigError::EmptyProvider);
        }
        if router.token.as_deref().is_some_and(blank) {
            return Err(RouterConfigError::EmptyToken);
        }
        for (name, value) in router.credentials.iter().flatten() {
            if blank(value) {
                return Err(RouterConfigError::EmptyCredential(name.clone()));
            }
        }
        if router.mode.as_deref() == Some("first") {
            let Some(provider) = router.provider.as_deref() else {
                return Err(RouterConfigError::FirstNeedsProvider);
            };
            let credential = router.credentials.iter().flatten().any(|(name, _)| {
                name == provider
                    || name
                        .strip_prefix(provider)
                        .is_some_and(|rest| rest.starts_with('_'))
            });
            if router.token.is_none() && !credential {
                return Err(RouterConfigError::FirstNeedsKey);
            }
        }
        Ok(())
    }
}

fn blank(value: &str) -> bool {
    value.trim().is_empty()
}

/// `~/.spider/router.json`, or `None` when there is no home directory.
pub fn path() -> Option<PathBuf> {
    store::spider_dir().map(|dir| dir.join(ROUTER_FILE))
}

/// The stored router, or `None` when no file is there.
///
/// A file that is there and cannot be used is an error rather than no router,
/// because a fallback the caller thinks is on and is not costs pages.
pub fn load() -> Result<Option<StoredRouter>> {
    match path() {
        Some(path) => load_from(&path),
        None => Ok(None),
    }
}

/// The router stored at a path, or `None` when no file is there.
///
/// The read stops at [`MAX_ROUTER_FILE_BYTES`]. A file readable by anyone but
/// its owner is reported once per process on Unix and left as it is.
pub fn load_from(path: &Path) -> Result<Option<StoredRouter>> {
    use std::io::Read;

    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(local(format!("could not open ~/{ROUTER_PATH}: {e}"))),
    };
    let mut bytes = Vec::new();
    file.take(MAX_ROUTER_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| local(format!("could not read ~/{ROUTER_PATH}: {e}")))?;
    if bytes.len() > MAX_ROUTER_FILE_BYTES {
        return Err(Error::Config(format!(
            "~/{ROUTER_PATH} is larger than {MAX_ROUTER_FILE_BYTES} bytes, so it was not read"
        )));
    }
    if let Some(mode) = store::wide_permissions(path) {
        let fix = "store the router again";
        store::warn_wide_permissions(path, mode, &WARNED_ABOUT_PERMISSIONS, fix);
    }
    // A serde message quotes the value it choked on, and that value can be a
    // key. Only the position is kept.
    let config: StoredRouter = serde_json::from_slice(&bytes).map_err(|e| {
        Error::Config(format!(
            "~/{ROUTER_PATH} is not a stored router (line {}, column {})",
            e.line(),
            e.column()
        ))
    })?;
    config.validate()?;
    Ok(Some(config))
}

/// Write the router to `~/.spider/router.json`, owner only, replacing what was
/// there. Returns the path.
pub fn store(config: &StoredRouter) -> Result<PathBuf> {
    config.validate()?;
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|_| Error::Config("the router could not be encoded".to_string()))?;
    if bytes.len() > MAX_ROUTER_FILE_BYTES {
        return Err(Error::Config(format!(
            "the router is larger than {MAX_ROUTER_FILE_BYTES} bytes, so it was not stored"
        )));
    }
    // The directory is created at its owner only mode if it is missing, and
    // the file is created owner only beside the path and renamed over it.
    store::write_private_file(ROUTER_FILE, &bytes)
        .map_err(|e| local(format!("could not write ~/{ROUTER_PATH}: {e}")))
}

/// Delete the stored router. True when there was one to delete.
pub fn clear() -> Result<bool> {
    let Some(path) = path() else {
        return Ok(false);
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(local(format!("could not delete ~/{ROUTER_PATH}: {e}"))),
    }
}

/// A problem on this machine rather than with the router.
fn local(message: String) -> Error {
    Error::Auth {
        cause: AuthCause::Local,
        message,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;

    const TOKEN: &str = "synthetic-provider-token";
    const PASSWORD: &str = "synthetic-provider-password";

    fn router(mode: &str) -> StoredRouter {
        StoredRouter {
            router: Router {
                mode: Some(mode.to_string()),
                provider: Some("zyte".to_string()),
                token: Some(TOKEN.to_string()),
                credentials: None,
                funding: Some("own".to_string()),
            },
            provider_options: None,
        }
    }

    /// A directory of our own, removed when the test ends. Nothing here reads
    /// or writes the developer's `~/.spider`.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("spider-router-{tag}-{unique}"));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        /// Written owner only, so no test here sets off the permission
        /// warning that the credentials tests count.
        fn write(&self, contents: &[u8]) -> PathBuf {
            let path = self.0.join(ROUTER_FILE);
            std::fs::write(&path, contents).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_path_constant_names_the_file_inside_the_spider_directory() {
        assert_eq!(ROUTER_PATH, format!("{}/{ROUTER_FILE}", store::SPIDER_DIR));
    }

    #[test]
    fn every_mode_and_funding_the_service_reads_is_accepted() {
        for mode in MODES {
            for funding in FUNDINGS {
                let mut config = router(mode);
                config.router.funding = Some((*funding).to_string());
                assert_eq!(config.validate(), Ok(()), "{mode} {funding}");
            }
        }
        assert_eq!(StoredRouter::default().validate(), Ok(()));
    }

    #[test]
    fn a_mode_or_funding_outside_the_set_is_refused() {
        assert_eq!(router("Fallback").validate(), Err(RouterConfigError::Mode));
        let mut config = router("fallback");
        config.router.funding = Some("mine".to_string());
        assert_eq!(config.validate(), Err(RouterConfigError::Funding));
    }

    #[test]
    fn first_needs_a_provider_and_a_key_for_it() {
        let mut config = router("first");
        config.router.provider = None;
        assert_eq!(
            config.validate(),
            Err(RouterConfigError::FirstNeedsProvider)
        );

        let mut config = router("first");
        config.router.token = None;
        assert_eq!(config.validate(), Err(RouterConfigError::FirstNeedsKey));

        config.router.provider = Some("oxylabs".to_string());
        config.router.credentials = Some(BTreeMap::from([
            ("oxylabs_username".to_string(), "name".to_string()),
            ("oxylabs_password".to_string(), PASSWORD.to_string()),
        ]));
        assert_eq!(config.validate(), Ok(()));

        // A credential for another provider is not a key for this one.
        config.router.provider = Some("oxy".to_string());
        assert_eq!(config.validate(), Err(RouterConfigError::FirstNeedsKey));

        // Fallback needs neither: the provider may already be armed on the
        // account.
        config.router.mode = Some("fallback".to_string());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn a_blank_token_credential_or_provider_is_refused() {
        let mut config = router("fallback");
        config.router.token = Some("  ".to_string());
        assert_eq!(config.validate(), Err(RouterConfigError::EmptyToken));

        let mut config = router("fallback");
        config.router.credentials = Some(BTreeMap::from([(
            "oxylabs_password".to_string(),
            String::new(),
        )]));
        assert_eq!(
            config.validate(),
            Err(RouterConfigError::EmptyCredential(
                "oxylabs_password".to_string()
            ))
        );

        let mut config = router("fallback");
        config.router.provider = Some(String::new());
        assert_eq!(config.validate(), Err(RouterConfigError::EmptyProvider));
    }

    #[test]
    fn an_unknown_provider_is_allowed() {
        let mut config = router("first");
        config.router.provider = Some("a-provider-added-later".to_string());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn debug_prints_names_and_no_secret() {
        let mut config = router("fallback");
        config.router.credentials = Some(BTreeMap::from([(
            "oxylabs_password".to_string(),
            PASSWORD.to_string(),
        )]));
        config.provider_options = Some(BTreeMap::from([(
            "zyte".to_string(),
            serde_json::json!({"geolocation": "synthetic-option-value"}),
        )]));
        let printed = format!("{config:?} {config:#?}");
        for secret in [TOKEN, PASSWORD, "synthetic-option-value", "geolocation"] {
            assert!(!printed.contains(secret), "{printed}");
        }
        for name in ["oxylabs_password", "zyte", "fallback", "own", REDACTED] {
            assert!(printed.contains(name), "{name} missing from {printed}");
        }
    }

    #[test]
    fn a_missing_file_is_no_router() {
        let dir = TempDir::new("missing");
        assert_eq!(load_from(&dir.0.join(ROUTER_FILE)).unwrap(), None);
    }

    #[test]
    fn a_file_round_trips() {
        let dir = TempDir::new("round-trip");
        let mut config = router("first");
        config.provider_options = Some(BTreeMap::from([(
            "zyte".to_string(),
            serde_json::json!({"geolocation": "US"}),
        )]));
        let path = dir.write(&serde_json::to_vec(&config).unwrap());
        assert_eq!(load_from(&path).unwrap(), Some(config));
    }

    #[test]
    fn a_file_past_the_cap_is_not_read() {
        let dir = TempDir::new("oversized");
        let mut padded = b"{\"router\":{},\"provider_options\":{\"x\":\"".to_vec();
        padded.resize(MAX_ROUTER_FILE_BYTES + 1, b'a');
        let path = dir.write(&padded);
        let error = load_from(&path).unwrap_err().to_string();
        assert!(error.contains("larger than"), "{error}");
    }

    #[test]
    fn a_malformed_file_names_a_position_and_no_value() {
        let dir = TempDir::new("malformed");
        let text = format!(r#"{{"router":{{"token":["{TOKEN}"]}}}}"#);
        let path = dir.write(text.as_bytes());
        let error = load_from(&path).unwrap_err();
        let printed = format!("{error} {error:?}");
        assert!(printed.contains("line 1"), "{printed}");
        assert!(!printed.contains(TOKEN), "{printed}");
    }

    #[test]
    fn a_stored_file_that_does_not_validate_is_an_error() {
        let dir = TempDir::new("invalid");
        let text = format!(r#"{{"router":{{"mode":"sideways","token":"{TOKEN}"}}}}"#);
        let path = dir.write(text.as_bytes());
        let error = load_from(&path).unwrap_err();
        let printed = format!("{error} {error:?}");
        assert!(printed.contains("router.mode"), "{printed}");
        assert!(!printed.contains("sideways") && !printed.contains(TOKEN));
    }

    /// Every error this module makes, built the way it builds them.
    pub(crate) fn every_error(dir: &Path) -> Vec<Error> {
        let mut out: Vec<Error> = Vec::new();
        let invalid = [
            {
                let mut config = router("sideways");
                config.router.token = Some(TOKEN.to_string());
                config
            },
            {
                let mut config = router("fallback");
                config.router.funding = Some(TOKEN.to_string());
                config
            },
            {
                let mut config = router("first");
                config.router.provider = None;
                config
            },
            {
                let mut config = router("first");
                config.router.token = None;
                config
            },
            {
                let mut config = router("fallback");
                config.router.provider = Some(" ".to_string());
                config
            },
            {
                let mut config = router("fallback");
                config.router.token = Some(" ".to_string());
                config
            },
            {
                let mut config = router("fallback");
                config.router.credentials =
                    Some(BTreeMap::from([("zyte".to_string(), " ".to_string())]));
                config
            },
        ];
        for config in &invalid {
            out.push(config.validate().unwrap_err().into());
            out.push(store(config).unwrap_err());
        }
        let oversized = dir.join("oversized.json");
        let mut padded = format!(r#"{{"router":{{"token":"{TOKEN}"}},"x":""#).into_bytes();
        padded.resize(MAX_ROUTER_FILE_BYTES + 1, b'a');
        std::fs::write(&oversized, padded).unwrap();
        out.push(load_from(&oversized).unwrap_err());
        let malformed = dir.join("malformed.json");
        std::fs::write(&malformed, format!(r#"{{"router":{{"token":{TOKEN}}}}}"#)).unwrap();
        out.push(load_from(&malformed).unwrap_err());
        let wrong_type = dir.join("wrong-type.json");
        // serde quotes a string it did not expect, which is the value.
        std::fs::write(&wrong_type, format!(r#"{{"router":"{TOKEN}"}}"#)).unwrap();
        out.push(load_from(&wrong_type).unwrap_err());
        // A directory where the file should be cannot be read.
        let directory = dir.join("directory.json");
        std::fs::create_dir_all(&directory).unwrap();
        out.extend(load_from(&directory).err());
        out
    }

    #[test]
    fn every_error_here_names_a_field_and_no_value() {
        let dir = TempDir::new("errors");
        let errors = every_error(&dir.0);
        assert_eq!(errors.len(), 18, "an error path stopped being covered");
        for error in errors {
            let printed = format!("{error} {error:?}");
            assert!(!printed.contains(TOKEN), "{printed}");
        }
    }
}
