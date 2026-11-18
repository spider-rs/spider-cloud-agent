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

/// Sign in and keep the key.
#[cfg(feature = "oauth")]
pub async fn login(global: &Global, args: &crate::cli::LoginArgs, log: Log) -> Run<Code> {
    use spider_cloud_agent::auth::oauth;

    if args.device {
        return device_login(global, args, log).await;
    }
    log.say("opening a browser to sign in");
    let key = oauth::login().await.map_err(Failure::from)?;
    keep(global, args, log, key)
}

/// Print the key or store it, whichever the caller asked for.
#[cfg(feature = "oauth")]
fn keep(global: &Global, args: &crate::cli::LoginArgs, log: Log, key: String) -> Run<Code> {
    use spider_cloud_agent::auth::{Credentials, Stored};

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

/// The file a sign in waits in between `--no-wait` and `--resume`. It holds
/// the device code, so it sits beside the key and is written the same way.
#[cfg(feature = "oauth")]
const PENDING_FILE: &str = "device-login.json";

/// The most of that file a resume reads. The real thing is a few hundred bytes.
#[cfg(feature = "oauth")]
const MAX_PENDING_BYTES: u64 = 16 * 1024;

/// How long `--resume` waits when the caller names no time. Short, because a
/// caller that split the sign in in two is one that cannot sit and wait.
#[cfg(feature = "oauth")]
const RESUME_WAIT_SECONDS: u64 = 30;

/// Sign in with a code a person types on another device.
#[cfg(feature = "oauth")]
async fn device_login(global: &Global, args: &crate::cli::LoginArgs, log: Log) -> Run<Code> {
    use spider_cloud_agent::auth::device;
    use std::time::Duration;

    let mut pending = if args.resume {
        read_pending()?
    } else {
        let pending = device::start().await.map_err(Failure::from)?;
        if args.no_wait {
            write_pending(&pending)?;
            let mut emitter = setup::emitter(global, Format::Text)?;
            emitter.write(pending_item(&pending))?;
            emitter.finish()?;
            return Ok(Code::Ok);
        }
        log.say(prompt(&pending));
        log.say("waiting for the code to be approved");
        pending
    };

    let seconds = match (args.wait, args.resume) {
        (Some(seconds), _) => seconds,
        (None, true) => RESUME_WAIT_SECONDS,
        (None, false) => pending.seconds_left(),
    };
    let outcome = device::wait(&mut pending, Duration::from_secs(seconds)).await;
    match outcome {
        Ok(Some(key)) => {
            forget_pending();
            let code = keep(global, args, log, key)?;
            if !args.print {
                let mut emitter = setup::emitter(global, Format::Text)?;
                emitter.write(
                    Item::structured(json!({ "type": "device_login", "status": "signed_in" }))
                        .with_text("signed in".to_string(), "txt"),
                )?;
                emitter.finish()?;
            }
            Ok(code)
        }
        Ok(None) => {
            // Kept, with whatever slower pace the server asked for, so the
            // next resume picks up where this one stopped.
            write_pending(&pending)?;
            Err(Failure::new(
                Code::Auth,
                format!(
                    "the code is not approved yet. {} Then run spider-agent login --device --resume",
                    prompt(&pending)
                ),
            ))
        }
        Err(error) => {
            // Declined or expired: the stored code is no use to anyone.
            forget_pending();
            Err(Failure::from(error))
        }
    }
}

/// What a person has to do, in one line.
#[cfg(feature = "oauth")]
fn prompt(pending: &spider_cloud_agent::auth::device::Pending) -> String {
    format!(
        "Open {} and enter the code {}.",
        pending.verification_uri, pending.user_code
    )
}

/// The code and the address as one item, for a caller that will show them to
/// a person and come back. Nothing in it is secret.
#[cfg(feature = "oauth")]
fn pending_item(pending: &spider_cloud_agent::auth::device::Pending) -> Item {
    let next = "spider-agent login --device --resume";
    Item::structured(json!({
        "type": "device_login",
        "status": "pending",
        "user_code": pending.user_code,
        "verification_uri": pending.verification_uri,
        "verification_uri_complete": pending.verification_uri_complete,
        "expires_in": pending.seconds_left(),
        "next": next,
    }))
    .with_text(format!("{} Then run {next}", prompt(pending)), "txt")
}

#[cfg(feature = "oauth")]
fn write_pending(pending: &spider_cloud_agent::auth::device::Pending) -> Run<()> {
    let bytes = serde_json::to_vec(pending)
        .map_err(|e| Failure::new(Code::Failed, format!("could not keep the sign in: {e}")))?;
    spider_cloud_agent::auth::store::write_private_file(PENDING_FILE, &bytes)
        .map(|_| ())
        .map_err(|e| Failure::new(Code::Output, format!("could not keep the sign in: {e}")))
}

#[cfg(feature = "oauth")]
fn read_pending() -> Run<spider_cloud_agent::auth::device::Pending> {
    use std::io::Read;
    let nothing = || {
        Failure::usage(
            "no sign in is waiting. Start one with spider-agent login --device --no-wait",
        )
    };
    let path = spider_cloud_agent::auth::store::spider_dir()
        .ok_or_else(nothing)?
        .join(PENDING_FILE);
    let file = std::fs::File::open(path).map_err(|_| nothing())?;
    let mut text = String::new();
    file.take(MAX_PENDING_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| nothing())?;
    serde_json::from_str(&text).map_err(|_| nothing())
}

/// Drop the stored code. Best effort: a file that will not go is an expired
/// code in an owner only directory, and not a reason to fail a sign in.
#[cfg(feature = "oauth")]
fn forget_pending() {
    if let Some(dir) = spider_cloud_agent::auth::store::spider_dir() {
        let _ = std::fs::remove_file(dir.join(PENDING_FILE));
    }
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
        "this build has no sign in. Set SPIDER_API_KEY, or install a build with the oauth feature.",
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
