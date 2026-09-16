//! Host handling for recorded fixtures.
//!
//! A fixture is a recorded API response checked into the repo. The only hosts allowed
//! to appear in one are the documentation hosts below. Everything else is a customer
//! domain or one of ours, and neither belongs in a public package.

/// Hosts a fixture may name.
pub const ALLOWED_HOSTS: &[&str] = &[
    "example.com",
    "example.org",
    "example.net",
    "httpbin.org",
    "spider.cloud",
];

/// Top level labels the bare-host scanner trusts. A bare token is only treated as a
/// host when its last label is one of these, so `Cargo.toml` and `main.rs` are not
/// mistaken for hostnames. Hosts written as a full URL are picked up regardless.
const KNOWN_TLDS: &[&str] = &[
    "com",
    "org",
    "net",
    "io",
    "cloud",
    "dev",
    "co",
    "uk",
    "ai",
    "app",
    "edu",
    "gov",
    "info",
    "us",
    "de",
    "fr",
    "jp",
    "cn",
    "ru",
    "br",
    "in",
    "ca",
    "au",
    "nl",
    "se",
    "it",
    "es",
    "xyz",
    "shop",
    "store",
    "site",
    "online",
    "internal",
    "local",
    "lan",
    "corp",
    "amazonaws",
];

/// True when `host`, or a parent of it, is on the allowlist. `cdn.example.com` passes
/// because `example.com` does.
pub fn host_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ALLOWED_HOSTS
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

/// Every host named in one line of text, from full URLs and from bare hostnames.
/// Returned lowercase, without userinfo, port or trailing dot, in the order found.
pub fn hosts_in_line(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for host in urls_in_line(line).iter().filter_map(|u| host_of_url(u)) {
        push_unique(&mut out, host);
    }
    for token in host_tokens(line) {
        if let Some(host) = normalize_host(&token) {
            if token_is_hostish(&host) {
                push_unique(&mut out, host);
            }
        }
    }
    out
}

fn push_unique(out: &mut Vec<String>, host: String) {
    if !out.contains(&host) {
        out.push(host);
    }
}

/// Every `scheme://...` substring in a line, cut at the first character that cannot
/// be part of a URL in JSON or prose.
pub fn urls_in_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Compare bytes, not a string slice: `line[i..]` panics when i lands
        // inside a multibyte character, and fixtures carry things like a pound
        // sign in a price. A checker that panics is a checker that hides findings.
        if bytes[i..].starts_with(b"://") {
            let mut start = i;
            while start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
                start -= 1;
            }
            if start == i {
                i += 3;
                continue;
            }
            let mut end = i + 3;
            while end < bytes.len() && !is_url_terminator(bytes[end]) {
                end += 1;
            }
            out.push(line[start..end].to_string());
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn is_url_terminator(b: u8) -> bool {
    matches!(
        b,
        b'"' | b'\'' | b'<' | b'>' | b'`' | b' ' | b'\t' | b'\\' | b')' | b']' | b'}' | b',' | b';'
    )
}

/// The host part of a URL, lowercase, without userinfo or port.
pub fn host_of_url(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    normalize_host(authority)
}

fn normalize_host(authority: &str) -> Option<String> {
    let host = authority.split(':').next().unwrap_or_default();
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(host)
}

/// Candidate bare hostnames in a line: dotted runs of host characters.
fn host_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
            current.push(ch);
        } else {
            if current.contains('.') {
                out.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if current.contains('.') {
        out.push(current);
    }
    out
}

/// True when a dotted token looks like a hostname rather than a filename, a version
/// or a decimal number.
fn token_is_hostish(token: &str) -> bool {
    let token = token.trim_matches('.');
    if token.is_empty() || token.contains("..") || token.contains('_') {
        return false;
    }
    let labels: Vec<&str> = token.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
        return false;
    }
    if labels.iter().all(|l| l.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    let tld = labels[labels.len() - 1];
    KNOWN_TLDS.contains(&tld)
}

/// Any host in a line that is not on the allowlist.
pub fn offending_hosts(line: &str) -> Vec<String> {
    hosts_in_line(line)
        .into_iter()
        .filter(|h| !host_allowed(h))
        .collect()
}

#[cfg(test)]
mod utf8_tests {
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
    use super::urls_in_line;

    #[test]
    fn a_multibyte_character_does_not_panic_the_scanner() {
        // A pound sign before the url puts the "://" at a byte offset that is not
        // a character boundary. Slicing the string there used to panic, which took
        // the whole check down and reported nothing at all.
        let line = "{\"price\": \"\u{a3}12.99\", \"url\": \"https://example.com/a\"}";
        assert_eq!(
            urls_in_line(line),
            vec!["https://example.com/a".to_string()]
        );
    }

    #[test]
    fn many_multibyte_characters_around_the_url() {
        let line = "\u{a3}\u{20ac}\u{5186} see https://example.org/x then \u{4e2d}\u{6587}";
        assert_eq!(
            urls_in_line(line),
            vec!["https://example.org/x".to_string()]
        );
    }
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
    fn the_allowlist_rejects_an_outside_host() {
        let line = r#"  "url": "https://shop.acme-retail.com/p/1?q=2","#;
        assert_eq!(
            offending_hosts(line),
            vec!["shop.acme-retail.com".to_string()]
        );
    }

    #[test]
    fn the_allowlist_accepts_the_documentation_hosts_and_their_subdomains() {
        assert!(offending_hosts(r#"{"url":"https://example.com/a"}"#).is_empty());
        assert!(offending_hosts(r#"{"url":"https://cdn.example.org/a"}"#).is_empty());
        assert!(offending_hosts(r#"{"url":"http://httpbin.org/get"}"#).is_empty());
        assert!(offending_hosts(r#"{"api":"https://api.spider.cloud/scrape"}"#).is_empty());
    }

    #[test]
    fn a_bare_host_is_found_but_a_filename_is_not() {
        assert_eq!(
            hosts_in_line("visit news.acme.co for more"),
            vec!["news.acme.co"]
        );
        assert!(hosts_in_line("open Cargo.toml and main.rs").is_empty());
        assert!(hosts_in_line("version 1.2.3").is_empty());
    }

    #[test]
    fn userinfo_and_port_are_stripped() {
        assert_eq!(
            host_of_url("https://user:pass@Api.Example.com:8443/path"),
            Some("api.example.com".to_string())
        );
    }
}
