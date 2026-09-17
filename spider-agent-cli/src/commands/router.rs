//! The stored provider fallback: set, show and clear.
//!
//! None of these makes a call. The config lives in `~/.spider/router.json`,
//! and the library writes it owner only. Keys arrive on stdin or in
//! `SPIDER_ROUTER_TOKEN`, never as an argument, and nothing here prints one.

use std::io::Read;

use serde_json::Value;

use spider_cloud_agent::auth::router::{self, StoredRouter, MAX_ROUTER_FILE_BYTES};

use crate::cli::{Format, Funding, Global, Mode, RouterArgs, RouterCommand, RouterSetArgs};
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;
use crate::records;
use crate::setup;

/// The environment variable a token can come from instead of stdin.
pub const TOKEN_ENV: &str = "SPIDER_ROUTER_TOKEN";

/// Providers that take one token.
pub const TOKEN_PROVIDERS: &[&str] = &[
    "firecrawl",
    "zenrows",
    "scrapingbee",
    "scraperapi",
    "brightdata",
    "zyte",
    "apify",
    "crawlbase",
    "diffbot",
];

/// Providers that take a username and a password as credentials, and no token.
pub const CREDENTIAL_PROVIDERS: &[&str] = &["oxylabs", "dataforseo"];

/// Run the `router` subcommand the caller named.
pub fn command(global: &Global, args: &RouterArgs, log: Log) -> Run<Code> {
    match &args.command {
        RouterCommand::Set(set_args) => set(set_args, log),
        RouterCommand::Show => show(global, log),
        RouterCommand::Clear => clear(log),
    }
}

/// Where a credential's value comes from.
enum Source {
    Given(String),
    Stdin,
}

/// Merge the flags into the stored config and write it back.
fn set(args: &RouterSetArgs, log: Log) -> Run<Code> {
    if args.token.is_some() {
        return Err(Failure::usage(
            "--token is refused: a key on the command line lands in shell history and in the process list. Pipe it to --token-stdin, or set SPIDER_ROUTER_TOKEN.",
        ));
    }
    let credentials = args
        .credential
        .iter()
        .map(|spec| credential(spec))
        .collect::<Run<Vec<_>>>()?;
    let options = args
        .option
        .iter()
        .map(|spec| option(spec))
        .collect::<Run<Vec<_>>>()?;
    let removed_options = args
        .no_option
        .iter()
        .map(|spec| {
            option_path(spec)
                .ok_or_else(|| Failure::usage(format!("--no-option {spec} is not PROVIDER.KEY.")))
        })
        .collect::<Run<Vec<_>>>()?;

    let from_stdin = usize::from(args.token_stdin)
        + credentials
            .iter()
            .filter(|(_, source)| matches!(source, Source::Stdin))
            .count();
    if from_stdin > 1 {
        return Err(Failure::usage(
            "only one value can be read from stdin per run. Store the others with another router set.",
        ));
    }
    let env_token = std::env::var("SPIDER_ROUTER_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let changes = args.provider.is_some()
        || args.mode.is_some()
        || args.funding.is_some()
        || args.token_stdin
        || args.no_token
        || env_token.is_some()
        || !credentials.is_empty()
        || !args.no_credential.is_empty()
        || !options.is_empty()
        || !removed_options.is_empty();
    if !changes {
        return Err(Failure::usage(
            "nothing to set. Name at least one of --provider, --mode, --funding, --token-stdin, --credential or --option.",
        ));
    }

    // Read before stdin, so a file that cannot be used stops the run before
    // a key is taken off the pipe.
    let mut config = setup::stored_router()?.unwrap_or_default();
    let piped = if from_stdin == 1 {
        Some(read_secret()?)
    } else {
        None
    };
    apply(
        &mut config,
        args,
        credentials,
        options,
        removed_options,
        piped,
        env_token,
    );

    config
        .validate()
        .map_err(|error| Failure::usage(format!("{error}. Nothing was stored.")))?;
    let path = router::store(&config)
        .map_err(|error| Failure::output(format!("the router could not be stored: {error}")))?;
    for note in notes(&config) {
        log.say(note);
    }
    log.say(format!("stored the router in {}", path.display()));
    Ok(Code::Ok)
}

/// Write the parsed flags over the stored config. A flag that was not passed
/// leaves its field as it was.
fn apply(
    config: &mut StoredRouter,
    args: &RouterSetArgs,
    credentials: Vec<(String, Source)>,
    options: Vec<(String, String, Value)>,
    removed_options: Vec<(String, String)>,
    mut piped: Option<String>,
    env_token: Option<String>,
) {
    let router = &mut config.router;
    if let Some(provider) = &args.provider {
        router.provider = Some(provider.trim().to_string());
    }
    // The fetch modes share the type and never parse on this flag.
    match args.mode {
        Some(Mode::Fallback) => router.mode = Some("fallback".to_string()),
        Some(Mode::First) => router.mode = Some("first".to_string()),
        Some(Mode::Off) => router.mode = Some("off".to_string()),
        Some(Mode::Http | Mode::Smart | Mode::Browser | Mode::Shadow | Mode::Apply) | None => {}
    }
    if let Some(funding) = args.funding {
        router.funding = Some(
            match funding {
                Funding::Any => "any",
                Funding::Own => "own",
            }
            .to_string(),
        );
    }
    if args.no_token {
        router.token = None;
    } else if args.token_stdin {
        router.token = piped.take();
    } else if let Some(token) = env_token {
        router.token = Some(token.trim().to_string());
    }

    let mut held = router.credentials.take().unwrap_or_default();
    for name in &args.no_credential {
        held.remove(name);
    }
    for (name, source) in credentials {
        let value = match source {
            Source::Given(value) => value,
            Source::Stdin => piped.take().unwrap_or_default(),
        };
        held.insert(name, value);
    }
    router.credentials = (!held.is_empty()).then_some(held);

    let mut settings = config.provider_options.take().unwrap_or_default();
    for (provider, key) in removed_options {
        if let Some(Value::Object(map)) = settings.get_mut(&provider) {
            map.remove(&key);
        }
    }
    for (provider, key, value) in options {
        let entry = settings
            .entry(provider)
            .or_insert_with(|| Value::Object(Default::default()));
        if !entry.is_object() {
            *entry = Value::Object(Default::default());
        }
        if let Value::Object(map) = entry {
            map.insert(key, value);
        }
    }
    settings.retain(|_, value| !matches!(value, Value::Object(map) if map.is_empty()));
    config.provider_options = (!settings.is_empty()).then_some(settings);
}

/// Things worth saying about a config that validated. None is a failure.
fn notes(config: &StoredRouter) -> Vec<String> {
    let router = &config.router;
    let mut out = Vec::new();
    if let Some(provider) = router.provider.as_deref() {
        let known = TOKEN_PROVIDERS.contains(&provider) || CREDENTIAL_PROVIDERS.contains(&provider);
        if !known {
            out.push(format!(
                "note: {provider} is not a provider this version knows. It was stored anyway, because providers are added without a release."
            ));
        }
        if CREDENTIAL_PROVIDERS.contains(&provider) && router.token.is_some() {
            out.push(format!(
                "note: {provider} takes a username and password as credentials and no token, so the stored token is not used for it."
            ));
        }
    }
    out
}

/// `NAME=VALUE` or `NAME-stdin`.
///
/// A value that looks like a secret by its name is refused inline, for the
/// reason `--token` is.
fn credential(spec: &str) -> Run<(String, Source)> {
    if let Some((name, value)) = spec.split_once('=') {
        let name = plain_name(name, "--credential")?;
        let lowered = name.to_ascii_lowercase();
        if ["password", "secret", "token", "key"]
            .iter()
            .any(|word| lowered.contains(word))
        {
            return Err(Failure::usage(format!(
                "--credential {name}=... is refused: a key on the command line lands in shell history and in the process list. Pipe it to --credential {name}-stdin."
            )));
        }
        return Ok((name, Source::Given(value.to_string())));
    }
    if let Some(name) = spec.strip_suffix("-stdin") {
        return Ok((plain_name(name, "--credential")?, Source::Stdin));
    }
    Err(Failure::usage(
        "--credential takes NAME=VALUE for a value that is not secret, or NAME-stdin to read the value from stdin.",
    ))
}

/// `PROVIDER.KEY=VALUE`. A value that reads as JSON is kept as JSON.
fn option(spec: &str) -> Run<(String, String, Value)> {
    let parsed = spec
        .split_once('=')
        .and_then(|(path, value)| option_path(path).map(|(p, k)| (p, k, value)));
    let Some((provider, key, value)) = parsed else {
        return Err(Failure::usage(
            "--option takes PROVIDER.KEY=VALUE, for example zyte.geolocation=US.",
        ));
    };
    let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
    Ok((provider, key, value))
}

/// `PROVIDER.KEY`, both parts non-empty.
fn option_path(spec: &str) -> Option<(String, String)> {
    let (provider, key) = spec.split_once('.')?;
    let (provider, key) = (provider.trim(), key.trim());
    (!provider.is_empty() && !key.is_empty()).then(|| (provider.to_string(), key.to_string()))
}

/// A credential name, trimmed and non-empty.
fn plain_name(name: &str, flag: &str) -> Run<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Failure::usage(format!("{flag} needs a name.")));
    }
    Ok(name.to_string())
}

/// One secret off stdin, trimmed. The read stops at the size a router file
/// may be, and an empty read is refused rather than stored.
fn read_secret() -> Run<String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_ROUTER_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::usage(format!("could not read stdin: {e}")))?;
    if bytes.len() > MAX_ROUTER_FILE_BYTES {
        return Err(Failure::usage(
            "stdin held more than any key, so nothing was stored.",
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| Failure::usage("stdin was not text, so nothing was stored."))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(Failure::usage("stdin was empty, so nothing was stored."));
    }
    Ok(text.to_string())
}

/// Print the stored config with every secret replaced.
fn show(global: &Global, log: Log) -> Run<Code> {
    let stored = setup::stored_router()?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    emitter.write_sole(records::router(stored.as_ref()))?;
    emitter.finish()?;
    if stored.is_none() {
        log.say("no router is stored. Store one with spider-agent router set.");
    }
    Ok(Code::Ok)
}

/// Delete the file and say so.
fn clear(log: Log) -> Run<Code> {
    let path = router::path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| format!("~/{}", router::ROUTER_PATH));
    match router::clear() {
        Ok(true) => log.say(format!("deleted the stored router at {path}")),
        Ok(false) => log.say(format!(
            "no router was stored at {path}, so nothing was deleted"
        )),
        Err(error) => {
            return Err(Failure::output(format!(
                "the router could not be deleted: {error}"
            )))
        }
    }
    Ok(Code::Ok)
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    /// The variables are read as literals so a reader can grep for them. The
    /// constants the schema prints have to name the same ones.
    #[test]
    fn the_variable_names_match_the_literals_that_are_read() {
        assert_eq!(TOKEN_ENV, "SPIDER_ROUTER_TOKEN");
        assert_eq!(setup::NO_ROUTER_ENV, "SPIDER_AGENT_NO_ROUTER");
    }

    #[test]
    fn a_secret_looking_credential_is_refused_inline_and_a_name_is_not() {
        assert!(credential("oxylabs_password=synthetic").is_err());
        assert!(credential("zyte_api_key=synthetic").is_err());
        assert!(matches!(
            credential("oxylabs_username=someone"),
            Ok((name, Source::Given(value))) if name == "oxylabs_username" && value == "someone"
        ));
        assert!(matches!(
            credential("oxylabs_password-stdin"),
            Ok((name, Source::Stdin)) if name == "oxylabs_password"
        ));
        assert!(credential("oxylabs_password").is_err());
        assert!(credential("=value").is_err());
    }

    #[test]
    fn an_option_keeps_json_as_json_and_the_rest_as_text() {
        let (provider, key, value) = option("zyte.geolocation=US").unwrap();
        assert_eq!((provider.as_str(), key.as_str()), ("zyte", "geolocation"));
        assert_eq!(value, Value::String("US".to_string()));
        assert_eq!(option("zyte.retries=3").unwrap().2, serde_json::json!(3));
        assert!(option("zyte=US").is_err());
        assert!(option(".geolocation=US").is_err());
    }

    #[test]
    fn a_known_provider_gets_no_note_and_an_unknown_one_does() {
        let mut config = StoredRouter::default();
        config.router.provider = Some("zyte".to_string());
        assert!(notes(&config).is_empty());
        config.router.provider = Some("a-provider-added-later".to_string());
        assert_eq!(notes(&config).len(), 1);
    }
}
