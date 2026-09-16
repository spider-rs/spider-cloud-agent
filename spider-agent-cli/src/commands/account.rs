//! Reads against the account, and signing in.
//!
//! None of these fetches a page, so none of them escalates and none of them
//! costs credits.

use serde_json::json;

use crate::cli::{Format, Global, RowsArgs};
use crate::emit::Item;
use crate::exit::{Code, Failure, Run};
use crate::progress::Log;
use crate::records;
use crate::setup;

/// What is left on the account.
pub async fn credits(global: &Global, log: Log) -> Run<Code> {
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Text)?;
    let balance = spider.credits().await.map_err(Failure::from)?;
    let item = Item::structured(json!({
        "type": "credits",
        "credits": balance.get(),
        "usd": balance.to_usd(),
    }))
    .with_text(format!("{}", balance.get()), "txt");
    emitter.write(item)?;
    emitter.finish()?;
    log.note("the balance read costs nothing");
    Ok(Code::Ok)
}

/// The record of past crawls.
pub async fn logs(global: &Global, args: &RowsArgs, _log: Log) -> Run<Code> {
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Ndjson)?;
    let mut call = spider.crawl_logs();
    if let Some(limit) = args.limit {
        call = call.limit(limit);
    }
    if let Some(page) = args.page {
        call = call.page(page);
    }
    let rows = call.send().await.map_err(Failure::from)?;
    write_rows(&mut emitter, &rows)?;
    emitter.finish()?;
    Ok(Code::Ok)
}

/// The sites configured on the account.
pub async fn sites(global: &Global, args: &RowsArgs, _log: Log) -> Run<Code> {
    stored(global, args, WEBSITES).await
}

/// The API keys on the account, as metadata.
pub async fn keys(global: &Global, args: &RowsArgs, _log: Log) -> Run<Code> {
    stored(global, args, API_KEYS).await
}

/// The account itself.
pub async fn profile(global: &Global, _log: Log) -> Run<Code> {
    stored(global, &RowsArgs::none(), PROFILES).await
}

/// The names this tool reads, spelled once.
///
/// Each is a constant rather than an argument. The command a caller types
/// names the thing it wants, and nothing a caller types reaches the path.
const WEBSITES: &str = "websites";
const API_KEYS: &str = "api_keys";
const PROFILES: &str = "profiles";

/// Read one of the stored reads above.
async fn stored(global: &Global, args: &RowsArgs, name: &'static str) -> Run<Code> {
    let spider = setup::client(global)?;
    let mut emitter = setup::emitter(global, Format::Ndjson)?;
    let mut call = spider.table(name);
    if let Some(limit) = args.limit {
        call = call.limit(limit);
    }
    if let Some(page) = args.page {
        call = call.page(page);
    }
    let rows = call.send().await.map_err(Failure::from)?;
    write_rows(&mut emitter, &rows)?;
    emitter.finish()?;
    Ok(Code::Ok)
}

/// Write every row, whatever the columns turned out to be.
fn write_rows(emitter: &mut crate::emit::Emitter, rows: &[serde_json::Value]) -> Run<()> {
    for row in rows {
        emitter.write(records::row(row))?;
    }
    Ok(())
}

/// Sign in through a browser and keep the key.
#[cfg(feature = "oauth")]
pub async fn login(global: &Global, args: &crate::cli::LoginArgs, log: Log) -> Run<Code> {
    use spider_cloud_agent::auth::{oauth, Credentials, Stored};

    log.say("opening a browser to sign in");
    let key = oauth::login().await.map_err(Failure::from)?;

    if args.print {
        let mut emitter = setup::emitter(global, Format::Text)?;
        // The key goes to the payload stream and nowhere else. It is never
        // logged, never put in a record and never printed alongside anything
        // that would end up in a transcript.
        emitter.write(key_item(key))?;
        emitter.finish()?;
        return Ok(Code::Ok);
    }

    let stored = Credentials::store(&key).map_err(|e| {
        Failure::new(
            Code::Output,
            format!("signed in, but the key could not be stored: {e}"),
        )
    })?;
    log.say(match stored {
        Stored::File(path) => format!("signed in. The key is in {}", path.display()),
        Stored::Keyring => "signed in. The key is in the system keychain".to_string(),
        _ => "signed in".to_string(),
    });
    Ok(Code::Ok)
}

/// The key as one item for `login --print`, in every shape the emitter writes.
///
/// The structured form carries the key too. `--print` exists to hand the key
/// to whatever asked for it, and a caller who also passed `--json` would
/// otherwise get a record that names a key and holds none, while the key
/// itself, already redeemed from a code that cannot be redeemed twice, is gone.
#[cfg(feature = "oauth")]
fn key_item(key: String) -> Item {
    Item::structured(json!({ "type": "key", "key": key })).with_text(key, "txt")
}

/// Signing in needs the browser flow, which this build left out.
#[cfg(not(feature = "oauth"))]
pub async fn login(_global: &Global, _args: &crate::cli::LoginArgs, _log: Log) -> Run<Code> {
    Err(Failure::usage(
        "this build has no browser sign in. Set SPIDER_API_KEY, or install a build with the oauth feature.",
    ))
}

#[cfg(all(test, feature = "oauth"))]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    const KEY: &str = "sk-live-not-a-real-key-0123456789";

    #[test]
    fn the_printed_key_is_in_the_structured_form_as_well_as_the_text() {
        let item = key_item(KEY.to_string());
        assert_eq!(item.text.as_deref(), Some(KEY));
        assert_eq!(item.value.get("type").and_then(|v| v.as_str()), Some("key"));
        let encoded = serde_json::to_string(&item.value).unwrap();
        assert!(
            encoded.contains(KEY),
            "--print --json loses the key: {encoded}"
        );
    }
}
