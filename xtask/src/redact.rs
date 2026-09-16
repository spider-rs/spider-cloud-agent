//! Turn a recorded API response into a fixture that can be published.
//!
//! This is the only sanctioned way to add a fixture. It rewrites every host to a
//! documentation host, in values and in object keys, whether the host sits in a
//! URL or stands bare in a `domain` field, drops authorization headers and
//! cookies, replaces anything shaped like an API key, and pretty prints the
//! result so a diff is readable.

use serde_json::{Map, Value};

use crate::fixtures;

/// The host every other host becomes.
pub const REPLACEMENT_HOST: &str = "example.com";

/// What a removed secret is replaced with.
pub const REDACTED: &str = "REDACTED";

/// Object keys whose value is dropped whatever it holds.
const SECRET_KEYS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api_key",
    "apikey",
    "access_token",
    "refresh_token",
    "id_token",
    "spider_api_key",
    "token",
    "secret",
    "password",
];

/// Run `redact <file> [-o <out>]`. Returns true, because redaction either works or
/// reports an error.
pub fn run(args: &[String]) -> Result<bool, String> {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                i += 1;
                output = Some(args.get(i).cloned().ok_or_else(|| {
                    "-o needs a path to write to. Leave it off to print to stdout.".to_string()
                })?);
            }
            "--in-place" => output = Some(String::new()),
            other if other.starts_with('-') => {
                return Err(format!(
                    "unknown option {other}. redact takes -o <path> or --in-place."
                ))
            }
            other => {
                if input.is_some() {
                    return Err("redact takes one file at a time.".to_string());
                }
                input = Some(other.to_string());
            }
        }
        i += 1;
    }
    let input = input.ok_or_else(|| {
        "redact needs a file to read. Usage: cargo run -p xtask -- redact <file> [-o <out>]"
            .to_string()
    })?;
    if output.as_deref() == Some("") {
        output = Some(input.clone());
    }

    let text = std::fs::read_to_string(&input).map_err(|e| {
        format!("cannot read {input}: {e}. Give redact a path to a recorded json response.")
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|e| {
        format!("{input} is not valid json: {e}. Save the recording as json first.")
    })?;
    let cleaned = redact_value(&value);
    let pretty = serde_json::to_string_pretty(&cleaned)
        .map_err(|e| format!("cannot write the redacted json: {e}"))?;

    match output {
        Some(path) => {
            std::fs::write(&path, format!("{pretty}\n"))
                .map_err(|e| format!("cannot write {path}: {e}"))?;
            eprintln!("redact: wrote {path}. Run leakcheck before committing it.");
        }
        None => println!("{pretty}"),
    }
    Ok(true)
}

/// Redact a whole json document.
pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_string(s)),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                // A key can be an address too: the page's request and response
                // maps are keyed by the URL each event was for.
                let key = redact_string(k);
                if SECRET_KEYS.contains(&k.to_ascii_lowercase().as_str()) {
                    out.insert(key, Value::String(REDACTED.to_string()));
                } else {
                    out.insert(key, redact_value(v));
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Redact one string: hosts first, then bearer tokens, then a bare key.
pub fn redact_string(s: &str) -> String {
    let s = replace_hosts(s);
    let s = replace_bare_hosts(&s);
    let s = strip_bearer(&s);
    if looks_like_api_key(&s) {
        return REDACTED.to_string();
    }
    s
}

/// Rewrite the host of every URL in a string, leaving the scheme, path and query.
pub fn replace_hosts(s: &str) -> String {
    let mut out = s.to_string();
    for url in fixtures::urls_in_line(s) {
        let Some(host) = fixtures::host_of_url(&url) else {
            continue;
        };
        if fixtures::host_allowed(&host) {
            continue;
        }
        let Some((scheme, rest)) = url.split_once("://") else {
            continue;
        };
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let tail = &rest[authority_end..];
        let replaced = format!("{scheme}://{REPLACEMENT_HOST}{tail}");
        out = out.replace(&url, &replaced);
    }
    out
}

/// Rewrite every host that stands on its own, outside a URL, the way a
/// `domain` field or a cookie scope names one. Longer hosts go first so that
/// `shop.acme.example` is rewritten whole rather than leaving `shop.` behind.
pub fn replace_bare_hosts(s: &str) -> String {
    let mut hosts: Vec<String> = fixtures::hosts_in_line(s)
        .into_iter()
        .filter(|host| !fixtures::host_allowed(host))
        .collect();
    hosts.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    let mut out = s.to_string();
    for host in hosts {
        out = replace_ignoring_case(&out, &host, REPLACEMENT_HOST);
    }
    out
}

/// Replace every occurrence of `needle` in `haystack`, matching ASCII case
/// insensitively, since a host is case insensitive and a recording may carry
/// it either way.
fn replace_ignoring_case(haystack: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0usize;
    for (at, _) in lower.match_indices(&needle) {
        // Byte offsets in the lowercase copy are offsets in the original:
        // ASCII lowercasing never changes a byte's width.
        out.push_str(haystack.get(last..at).unwrap_or_default());
        out.push_str(with);
        last = at + needle.len();
    }
    out.push_str(haystack.get(last..).unwrap_or_default());
    out
}

/// Drop the value after `Bearer`, in a header line or on its own.
pub fn strip_bearer(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let Some(at) = lower.find("bearer ") else {
        return s.to_string();
    };
    let head = &s[..at + "bearer ".len()];
    format!("{head}{REDACTED}")
}

/// True when a string looks like a credential rather than prose: one long token of
/// key characters carrying both letters and digits.
pub fn looks_like_api_key(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 20 || t.len() > 200 {
        return false;
    }
    if t.contains("://") || t.contains('.') || t.contains(' ') {
        return false;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let has_alpha = t.chars().any(|c| c.is_ascii_alphabetic());
    has_digit && has_alpha
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

    #[test]
    fn hosts_become_the_documentation_host() {
        assert_eq!(
            replace_hosts(r#"{"url":"https://shop.acme-retail.com/p/1?q=2"}"#),
            r#"{"url":"https://example.com/p/1?q=2"}"#
        );
        assert_eq!(
            replace_hosts("see https://example.org/a"),
            "see https://example.org/a"
        );
    }

    #[test]
    fn a_bare_host_is_rewritten_and_a_documentation_host_is_left() {
        assert_eq!(replace_bare_hosts("shop.acme-retail.com"), "example.com");
        assert_eq!(
            replace_bare_hosts("Domain=Shop.Acme-Retail.com; Path=/"),
            "Domain=example.com; Path=/"
        );
        assert_eq!(replace_bare_hosts("httpbin.org"), "httpbin.org");
        assert_eq!(
            replace_bare_hosts("Cargo.toml and main.rs"),
            "Cargo.toml and main.rs"
        );
    }

    #[test]
    fn a_key_that_is_an_address_is_rewritten_too() {
        let raw = serde_json::json!({
            "request_map": {"https://cdn.acme-retail.com/app.js": 41.5},
            "metadata": {"domain": "shop.acme-retail.com"}
        });
        let out = redact_value(&raw);
        assert_eq!(out["request_map"]["https://example.com/app.js"], 41.5);
        assert_eq!(out["metadata"]["domain"], "example.com");
        assert!(!serde_json::to_string(&out)
            .expect("serializes")
            .contains("acme-retail"));
    }

    #[test]
    fn a_bearer_token_is_dropped() {
        assert_eq!(strip_bearer("Bearer sk-live-6d9a2f11c0"), "Bearer REDACTED");
        assert_eq!(strip_bearer("no header here"), "no header here");
    }

    #[test]
    fn a_key_is_replaced_but_prose_is_not() {
        assert!(looks_like_api_key("sk-live-6d9a2f11c04b8e7fa93d21"));
        assert!(!looks_like_api_key("this is a normal sentence about keys"));
        assert!(!looks_like_api_key("short1"));
        assert!(!looks_like_api_key("https://example.com/a/b/c/d/e/f/g"));
    }

    #[test]
    fn a_whole_document_is_cleaned() {
        let raw = serde_json::json!({
            "url": "https://portal.acme-retail.com/account",
            "headers": {
                "Authorization": "Bearer sk-live-6d9a2f11c04b8e7fa93d21",
                "Set-Cookie": "session=abc; Path=/",
                "Content-Type": "text/html"
            },
            "api_key": "sk-live-6d9a2f11c04b8e7fa93d21",
            "content": "<a href=\"https://portal.acme-retail.com/login\">in</a>",
            "status": 200,
            "links": ["https://portal.acme-retail.com/a", "https://example.com/b"]
        });
        let out = redact_value(&raw);
        let text = serde_json::to_string(&out).expect("serializes");
        assert!(!text.contains("acme-retail"), "host survived: {text}");
        assert!(!text.contains("sk-live"), "key survived: {text}");
        assert_eq!(
            out["headers"]["Authorization"],
            serde_json::json!("REDACTED")
        );
        assert_eq!(
            out["headers"]["Content-Type"],
            serde_json::json!("text/html")
        );
        assert_eq!(out["status"], serde_json::json!(200));
        assert_eq!(out["links"][1], serde_json::json!("https://example.com/b"));
    }
}
