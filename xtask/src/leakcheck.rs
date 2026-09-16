//! The publish-time leak checker.
//!
//! It answers one question: if we published right now, what internal detail would go
//! with it. By default it checks the packaged file set, the list `cargo package`
//! would put in the `.crate` archive, because the working tree holds files that
//! `include = [...]` leaves out. `--tree` checks the working tree instead, which is
//! faster and does not need the workspace to compile.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fixtures;
use crate::leak_words::{self, Scope, Word};

/// Environment variables the published crates are allowed to read.
pub const ALLOWED_ENV_VARS: &[&str] = &[
    "SPIDER_API_KEY",
    "SPIDER_CLOUD_API_KEY",
    "SPIDER_API_URL",
    "SPIDER_MCP_SERVER",
];

/// Bytes at the front of a model artifact that may hold printable ASCII, for a magic
/// number and a version.
pub const ARTIFACT_HEADER_BYTES: usize = 16;

/// The longest printable ASCII run allowed in a model artifact after the header.
pub const ARTIFACT_MAX_ASCII_RUN: usize = 8;

/// One thing worth failing the build over.
pub struct Finding {
    pub path: String,
    pub line: usize,
    pub reason: String,
}

impl Finding {
    fn print(&self) {
        println!("{}:{}: {}", self.path, self.line, self.reason);
    }
}

struct Options {
    tree: bool,
    explain: bool,
    require_private: bool,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        tree: false,
        explain: false,
        require_private: false,
    };
    for arg in args {
        match arg.as_str() {
            "--require-private" => opts.require_private = true,
            "--tree" => opts.tree = true,
            "--packaged" => opts.tree = false,
            "--explain" => opts.explain = true,
            other => {
                return Err(format!(
                    "unknown option {other}. leakcheck takes --tree, --packaged, --explain or --require-private."
                ))
            }
        }
    }
    Ok(opts)
}

/// Run the check. Returns true when nothing was found.
pub fn run(args: &[String]) -> Result<bool, String> {
    let opts = parse_args(args)?;
    let root = repo_root();
    let words = leak_words::load(opts.require_private)?;

    if opts.explain {
        print_explain(&words);
    }

    let crates = publishable_crates(&root)?;
    if crates.is_empty() {
        return Err(format!(
            "found no publishable crate under {}. Check the workspace members in Cargo.toml.",
            root.display()
        ));
    }

    let (files, source_label) = if opts.tree {
        (tree_files(&root, &crates), "working tree".to_string())
    } else {
        let mut all = Vec::new();
        for c in &crates {
            all.extend(packaged_files(&root, c)?);
        }
        (all, "packaged set".to_string())
    };

    let mut findings = Vec::new();
    let mut text_files = 0usize;
    let mut binary_files = 0usize;
    let mut fixture_files = 0usize;

    for path in &files {
        let rel = relative(&root, path);
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\u{0}') => {
                text_files += 1;
                let is_fixture = is_fixture_path(&rel);
                if is_fixture {
                    fixture_files += 1;
                }
                check_text(&rel, &text, &words, path, is_fixture, &mut findings);
            }
            _ => {
                binary_files += 1;
            }
        }
    }

    let artifacts: Vec<&PathBuf> = files
        .iter()
        .filter(|p| is_model_artifact(&relative(&root, p)))
        .collect();
    let mut artifact_note = String::from("no model artifact found, nothing to audit");
    if !artifacts.is_empty() {
        let baselines = load_baselines(&root)?;
        artifact_note = format!("{} model artifact(s) audited", artifacts.len());
        for path in artifacts {
            let rel = relative(&root, path);
            let bytes = std::fs::read(path)
                .map_err(|e| format!("cannot read the model artifact {rel}: {e}"))?;
            findings.extend(audit_artifact(&rel, &bytes, baselines.get(&rel).copied()));
        }
    }

    findings.sort_by(|a, b| (a.path.as_str(), a.line).cmp(&(b.path.as_str(), b.line)));
    for f in &findings {
        f.print();
    }

    let names: Vec<&str> = crates.iter().map(|c| c.name.as_str()).collect();
    println!(
        "leakcheck: checked {} files ({} text, {} binary, {} fixture) from the {} of {} ({}), {} denylist terms, {}. {} finding(s).",
        files.len(),
        text_files,
        binary_files,
        fixture_files,
        source_label,
        crates.len(),
        names.join(", "),
        words.len(),
        artifact_note,
        findings.len()
    );
    if files.is_empty() {
        return Err(
            "the file set was empty, so nothing was checked. That is a failure, not a pass."
                .to_string(),
        );
    }
    if !findings.is_empty() {
        println!(
            "Fix each line above, or if a term is a false positive, narrow the pattern rather than deleting the rule."
        );
    }
    Ok(findings.is_empty())
}

fn print_explain(words: &[Word]) {
    println!("leakcheck denylist categories:");
    for cat in leak_words::CATEGORIES {
        println!(
            "  {:12} {} ({} built-in terms)",
            cat.name,
            cat.note,
            cat.terms.len()
        );
    }
    println!(
        "  the real list is private. Point {} at it from a private checkout.",
        leak_words::EXTRA_LIST_ENV
    );
    println!("  loaded {} terms in total", words.len());
}

// ---------------------------------------------------------------------------
// file sets
// ---------------------------------------------------------------------------

/// A workspace member that is published.
pub struct CrateInfo {
    pub name: String,
    pub dir: PathBuf,
}

/// The repo root, which is the parent of this crate. `SPIDER_AGENT_ROOT` overrides
/// it, which is how the checker runs against a checkout it was not built inside.
pub fn repo_root() -> PathBuf {
    if let Some(root) = std::env::var_os("SPIDER_AGENT_ROOT") {
        return PathBuf::from(root);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask always sits one level under the workspace root")
        .to_path_buf()
}

/// Workspace members whose manifest does not say `publish = false`.
pub fn publishable_crates(root: &Path) -> Result<Vec<CrateInfo>, String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("cannot read {}: {e}", root.join("Cargo.toml").display()))?;
    let mut out = Vec::new();
    for member in workspace_members(&manifest) {
        let dir = root.join(&member);
        let Ok(member_manifest) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        if manifest_says_unpublished(&member_manifest) {
            continue;
        }
        let name = manifest_name(&member_manifest).unwrap_or(member.clone());
        out.push(CrateInfo { name, dir });
    }
    Ok(out)
}

/// The `members = [...]` list of a workspace manifest, read without a toml parser
/// because this tool stays on the standard library.
pub fn workspace_members(manifest: &str) -> Vec<String> {
    let Some(start) = manifest.find("members") else {
        return Vec::new();
    };
    let Some(open) = manifest[start..].find('[') else {
        return Vec::new();
    };
    let Some(close) = manifest[start + open..].find(']') else {
        return Vec::new();
    };
    let body = &manifest[start + open + 1..start + open + close];
    body.split(',')
        .map(|s| s.trim().trim_matches('"').trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn manifest_says_unpublished(manifest: &str) -> bool {
    manifest
        .lines()
        .map(str::trim)
        .any(|l| l.starts_with("publish") && l.contains("false"))
}

fn manifest_name(manifest: &str) -> Option<String> {
    manifest
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("name"))
        .and_then(|l| l.split('=').nth(1))
        .map(|v| v.trim().trim_matches('"').to_string())
}

/// The files `cargo package` would ship for one crate.
fn packaged_files(root: &Path, krate: &CrateInfo) -> Result<Vec<PathBuf>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(&cargo)
        .current_dir(root)
        .args([
            "package",
            "--list",
            "--quiet",
            "--allow-dirty",
            "-p",
            &krate.name,
        ])
        .output()
        .map_err(|e| format!("cannot run {cargo} package --list: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo package --list -p {} failed, so the packaged file set is unknown:\n{}\nRun it by hand to see why. Do not fall back to --tree for a publish check, because --tree can miss a file that include = [...] adds.",
            krate.name,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let mut files = Vec::new();
    for line in listing.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // cargo generates these at package time, so they are not on disk.
        if line == "Cargo.toml.orig" || line == ".cargo_vcs_info.json" || line == "Cargo.lock" {
            continue;
        }
        let path = krate.dir.join(line);
        if path.is_file() {
            files.push(path);
        }
    }
    Ok(files)
}

/// Every file under the publishable crates, plus the root level text files that end
/// up in a package or on the repo page.
fn tree_files(root: &Path, crates: &[CrateInfo]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for krate in crates {
        walk(&krate.dir, &mut files);
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") || name == "Cargo.toml" || name == "LICENSE" {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if matches!(
                name.as_str(),
                "target" | ".git" | "node_modules" | "graphify-out"
            ) {
                continue;
            }
            walk(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_fixture_path(rel: &str) -> bool {
    rel.contains("/tests/fixtures/") && rel.ends_with(".json")
}

fn is_model_artifact(rel: &str) -> bool {
    rel.starts_with("spider-route/assets/") && rel.ends_with(".bin")
}

// ---------------------------------------------------------------------------
// text checks
// ---------------------------------------------------------------------------

fn check_text(
    rel: &str,
    text: &str,
    words: &[Word],
    path: &Path,
    is_fixture: bool,
    findings: &mut Vec<Finding>,
) {
    let is_rust = rel.ends_with(".rs");
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        for word in words {
            if !word.scope.applies_to(path) {
                continue;
            }
            if !leak_words::find_term(line, &word.term).is_empty() {
                findings.push(Finding {
                    path: rel.to_string(),
                    line: line_no,
                    reason: format!(
                        "denylist term \"{}\" ({}). Remove it or replace it with a public equivalent.",
                        word.term, word.category
                    ),
                });
            }
        }
        for reason in host_findings(line, Scope::ShippedSource.applies_to(path)) {
            findings.push(Finding {
                path: rel.to_string(),
                line: line_no,
                reason,
            });
        }
        if is_rust {
            for reason in env_findings(line) {
                findings.push(Finding {
                    path: rel.to_string(),
                    line: line_no,
                    reason,
                });
            }
        }
        if is_fixture {
            for host in fixtures::offending_hosts(line) {
                findings.push(Finding {
                    path: rel.to_string(),
                    line: line_no,
                    reason: format!(
                        "fixture host \"{host}\" is not on the allowlist ({}). Re-record against a documentation host, or run cargo run -p xtask -- redact on the fixture.",
                        fixtures::ALLOWED_HOSTS.join(", ")
                    ),
                });
            }
        }
    }
    if is_fixture {
        if let Err(e) = serde_json::from_str::<serde_json::Value>(text) {
            findings.push(Finding {
                path: rel.to_string(),
                line: 1,
                reason: format!("fixture is not valid json: {e}. Fix the file or remove it."),
            });
        }
    }
}

/// Host and address patterns that should never ship. `shipped_source` is true for a
/// file under `src/`, where a loopback address is a hard coded default rather than a
/// line in a document telling someone what to run locally.
pub fn host_findings(line: &str, shipped_source: bool) -> Vec<String> {
    let mut out = Vec::new();
    let lower = line.to_ascii_lowercase();
    for suffix in [".internal", ".local"] {
        if !leak_words::find_term(&lower, suffix).is_empty() {
            out.push(format!(
                "host suffix \"{suffix}\" only resolves inside our network. Use a documentation host instead."
            ));
        }
    }
    if lower.contains(".amazonaws.com") {
        out.push(
            "an amazonaws.com host names our infrastructure. Use a documentation host or read the value from configuration."
                .to_string(),
        );
    }
    for (literal, kind) in ipv4_literals(line) {
        match kind {
            IpKind::Rfc1918 => out.push(format!(
                "private address literal {literal}. A published crate must not carry an address from our network."
            )),
            IpKind::Loopback if shipped_source => out.push(format!(
                "loopback address literal {literal} in shipped source. Take the address from configuration instead of hard coding it."
            )),
            IpKind::Loopback => {}
            IpKind::Bare => out.push(format!(
                "bare address literal {literal}. Use a documentation address such as 192.0.2.1, or take it from configuration."
            )),
            IpKind::Documentation => {}
        }
    }
    out
}

/// How an IPv4 literal is classified.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IpKind {
    /// RFC1918: 10/8, 172.16/12, 192.168/16.
    Rfc1918,
    /// 127/8 and 0.0.0.0.
    Loopback,
    /// RFC5737 documentation ranges and the broadcast address, which are fine.
    Documentation,
    /// Any other literal.
    Bare,
}

/// Classify one set of octets.
pub fn classify_ipv4(o: [u8; 4]) -> IpKind {
    match o {
        [10, ..] => IpKind::Rfc1918,
        [172, b, ..] if (16..=31).contains(&b) => IpKind::Rfc1918,
        [192, 168, ..] => IpKind::Rfc1918,
        [127, ..] | [0, 0, 0, 0] => IpKind::Loopback,
        [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _] | [255, 255, 255, 255] => {
            IpKind::Documentation
        }
        _ => IpKind::Bare,
    }
}

/// Every IPv4 literal in a line, with its class. Lines that look like a version
/// declaration are skipped, because `1.2.3.4` there is a crate version.
pub fn ipv4_literals(line: &str) -> Vec<(String, IpKind)> {
    let mut out = Vec::new();
    if looks_like_version_line(line) {
        return out;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let left_ok = i == 0
            || !matches!(bytes[i - 1], b'.' | b'-' | b'_') && !bytes[i - 1].is_ascii_alphanumeric();
        if !left_ok {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            continue;
        }
        match parse_ipv4_at(bytes, i) {
            Some((octets, end)) => {
                let right_ok = end >= bytes.len()
                    || !(bytes[end].is_ascii_alphanumeric()
                        || matches!(bytes[end], b'.' | b'-' | b'_'));
                if right_ok {
                    out.push((line[i..end].to_string(), classify_ipv4(octets)));
                }
                i = end;
            }
            None => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }
    }
    out
}

fn parse_ipv4_at(bytes: &[u8], start: usize) -> Option<([u8; 4], usize)> {
    let mut octets = [0u8; 4];
    let mut i = start;
    for (n, slot) in octets.iter_mut().enumerate() {
        if n > 0 {
            if i >= bytes.len() || bytes[i] != b'.' {
                return None;
            }
            i += 1;
        }
        let digits_start = i;
        let mut value: u32 = 0;
        while i < bytes.len() && bytes[i].is_ascii_digit() && i - digits_start < 3 {
            value = value * 10 + u32::from(bytes[i] - b'0');
            i += 1;
        }
        if i == digits_start || value > 255 {
            return None;
        }
        *slot = value as u8;
    }
    Some((octets, i))
}

fn looks_like_version_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("version") || lower.contains("edition") || lower.trim_start().starts_with("//!")
}

/// Compile time values cargo defines itself. They say nothing about our network, so
/// the two macros may read them. `env::var` may not, because at run time the name
/// would come from whatever the caller set.
fn cargo_compile_time_var(call: &str, name: &str) -> bool {
    matches!(call, "env!(" | "option_env!(") && name.starts_with("CARGO_")
}

/// Environment reads that are not on the allowlist.
pub fn env_findings(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for call in ["env::var_os(", "env::var(", "env!(", "option_env!("] {
        let mut from = 0usize;
        while let Some(rel) = line[from..].find(call) {
            let open = from + rel + call.len();
            from = open;
            match first_string_literal(&line[open..]) {
                Some(name) => {
                    if !ALLOWED_ENV_VARS.contains(&name.as_str())
                        && !cargo_compile_time_var(call, &name)
                    {
                        out.push(format!(
                            "reads the environment variable {name}, which is not one of {}. Add it to the allowlist in xtask only if a published crate really should read it.",
                            ALLOWED_ENV_VARS.join(", ")
                        ));
                    }
                }
                None => out.push(format!(
                    "{call} is called with a name this checker cannot read. Pass a string literal so the name can be audited."
                )),
            }
        }
    }
    out
}

/// The first `"..."` immediately inside a call, or None when the argument is not a
/// plain literal.
fn first_string_literal(rest: &str) -> Option<String> {
    let mut chars = rest.char_indices();
    let start = loop {
        let (i, c) = chars.next()?;
        if c == '"' {
            break i + 1;
        }
        if !c.is_whitespace() {
            return None;
        }
    };
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_string())
}

// ---------------------------------------------------------------------------
// model artifact audit
// ---------------------------------------------------------------------------

/// A run of printable ASCII in a binary blob.
#[derive(Debug, PartialEq, Eq)]
pub struct AsciiRun {
    pub offset: usize,
    pub text: String,
}

/// Printable ASCII runs of at least `min_len` bytes, starting the scan at `from`.
pub fn ascii_runs(blob: &[u8], min_len: usize, from: usize) -> Vec<AsciiRun> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = from.min(blob.len());
    while i <= blob.len() {
        let printable = i < blob.len() && (0x20..=0x7e).contains(&blob[i]);
        match (printable, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min_len {
                    out.push(AsciiRun {
                        offset: s,
                        text: String::from_utf8_lossy(&blob[s..i]).to_string(),
                    });
                }
                start = None;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Check one model artifact. `baseline` is the largest size recorded for it.
pub fn audit_artifact(rel: &str, blob: &[u8], baseline: Option<u64>) -> Vec<Finding> {
    let mut out = Vec::new();
    // Short runs still matter for the domain scan, so collect from four bytes up.
    for run in ascii_runs(blob, 4, ARTIFACT_HEADER_BYTES) {
        let hosts = fixtures::hosts_in_line(&run.text);
        if !hosts.is_empty() {
            out.push(Finding {
                path: rel.to_string(),
                line: 1,
                reason: format!(
                    "byte {}: the artifact contains a domain-like string {:?}. Weights must hold no host, so retrain without that feature.",
                    run.offset, hosts
                ),
            });
        }
        if run.text.len() > ARTIFACT_MAX_ASCII_RUN {
            out.push(Finding {
                path: rel.to_string(),
                line: 1,
                reason: format!(
                    "byte {}: printable ascii run of {} bytes, over the limit of {}: {:?}. Only the first {} header bytes may hold text.",
                    run.offset,
                    run.text.len(),
                    ARTIFACT_MAX_ASCII_RUN,
                    truncate(&run.text, 48),
                    ARTIFACT_HEADER_BYTES
                ),
            });
        }
    }
    match baseline {
        Some(max) if blob.len() as u64 > max => out.push(Finding {
            path: rel.to_string(),
            line: 1,
            reason: format!(
                "the artifact is {} bytes, past the recorded baseline of {max}. If the growth is expected, raise the baseline in xtask/artifact-baselines.json in the same commit.",
                blob.len()
            ),
        }),
        None => out.push(Finding {
            path: rel.to_string(),
            line: 1,
            reason: "no size baseline is recorded for this artifact. Add its path and its allowed maximum size in bytes to xtask/artifact-baselines.json.".to_string(),
        }),
        _ => {}
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut out = String::new();
        let _ = write!(out, "{}...", &s[..n]);
        out
    }
}

/// Recorded maximum sizes, keyed by path relative to the repo root.
pub fn load_baselines(root: &Path) -> Result<BTreeMap<String, u64>, String> {
    let path = root.join("xtask/artifact-baselines.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(BTreeMap::new());
    };
    parse_baselines(&text).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Parse the baseline file: a json object of path to maximum size in bytes.
pub fn parse_baselines(text: &str) -> Result<BTreeMap<String, u64>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid json: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "expected a json object of path to maximum size in bytes".to_string())?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if k.starts_with('_') {
            continue;
        }
        let size = v
            .get("max_bytes")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("entry {k} needs a max_bytes number"))?;
        out.insert(k.clone(), size);
    }
    Ok(out)
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
    fn rfc1918_is_caught_and_a_public_address_is_reported_separately() {
        assert_eq!(classify_ipv4([192, 168, 1, 1]), IpKind::Rfc1918);
        assert_eq!(classify_ipv4([10, 0, 0, 7]), IpKind::Rfc1918);
        assert_eq!(classify_ipv4([172, 20, 3, 4]), IpKind::Rfc1918);
        assert_eq!(classify_ipv4([172, 15, 3, 4]), IpKind::Bare);
        assert_eq!(classify_ipv4([172, 32, 3, 4]), IpKind::Bare);
        assert_eq!(classify_ipv4([1, 2, 3, 4]), IpKind::Bare);
        assert_eq!(classify_ipv4([192, 0, 2, 9]), IpKind::Documentation);
        assert_eq!(classify_ipv4([127, 0, 0, 1]), IpKind::Loopback);
    }

    #[test]
    fn literals_are_read_out_of_a_line_and_versions_are_left_alone() {
        let found = ipv4_literals("connect to 192.168.1.1 then 1.2.3.4");
        assert_eq!(
            found,
            vec![
                ("192.168.1.1".to_string(), IpKind::Rfc1918),
                ("1.2.3.4".to_string(), IpKind::Bare),
            ]
        );
        assert!(ipv4_literals("serde = { version = \"1.2.3.4\" }").is_empty());
        assert!(ipv4_literals("build v1.2.3.4 shipped").is_empty());
        assert!(ipv4_literals("not an address 1.2.3.4.5").is_empty());
        assert!(ipv4_literals("not an address 300.1.1.1").is_empty());
    }

    #[test]
    fn a_documentation_address_produces_no_finding_but_a_private_one_does() {
        assert!(host_findings("see 192.0.2.10 in the example", true).is_empty());
        assert_eq!(host_findings("the box at 10.1.2.3", true).len(), 1);
        // A loopback address is a finding in shipped source and fine in a document.
        assert_eq!(host_findings("bind 127.0.0.1:8080", true).len(), 1);
        assert!(host_findings("bind 127.0.0.1:8080", false).is_empty());
    }

    #[test]
    fn internal_host_suffixes_are_caught() {
        assert_eq!(
            host_findings("url = \"https://router.internal/health\"", true).len(),
            1
        );
        assert_eq!(
            host_findings("url = \"https://box.local/health\"", true).len(),
            1
        );
        assert_eq!(
            host_findings("url = \"https://queue.eu-west-1.amazonaws.com\"", true).len(),
            1
        );
        assert!(host_findings("url = \"https://example.com/health\"", true).is_empty());
    }

    #[test]
    fn env_reads_outside_the_allowlist_fail() {
        assert!(env_findings("let k = std::env::var(\"SPIDER_API_KEY\").ok();").is_empty());
        assert!(env_findings("let k = env!(\"SPIDER_API_URL\");").is_empty());
        let bad = env_findings("let h = std::env::var(\"INTERNAL_ROUTER_HOST\").ok();");
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("INTERNAL_ROUTER_HOST"));
        assert!(env_findings("const V: &str = env!(\"CARGO_PKG_VERSION\");").is_empty());
        assert_eq!(
            env_findings("let v = std::env::var(\"CARGO_PKG_VERSION\").ok();").len(),
            1
        );
        let dynamic = env_findings("let v = std::env::var(name).ok();");
        assert_eq!(dynamic.len(), 1);
        assert!(dynamic[0].contains("string literal"));
    }

    #[test]
    fn the_ascii_run_detector_finds_a_planted_domain_in_a_blob() {
        let mut blob: Vec<u8> = Vec::new();
        blob.extend_from_slice(b"SPDRT\x01\x00\x00");
        blob.extend_from_slice(&[0u8; 8]); // rest of the 16 byte header
        blob.extend_from_slice(&[0u8; 1024]); // stand in for weights
        let planted_at = blob.len();
        blob.extend_from_slice(b"shop.acme-retail.com");
        blob.extend_from_slice(&[0u8; 32]);

        let runs = ascii_runs(&blob, 4, ARTIFACT_HEADER_BYTES);
        let hit = runs
            .iter()
            .find(|r| r.offset == planted_at)
            .expect("the planted run should be found");
        assert_eq!(hit.text, "shop.acme-retail.com");

        let findings = audit_artifact(
            "spider-route/assets/test.bin",
            &blob,
            Some(blob.len() as u64),
        );
        assert!(findings.iter().any(|f| f.reason.contains("domain-like")));
        assert!(findings
            .iter()
            .any(|f| f.reason.contains("printable ascii run")));
    }

    #[test]
    fn a_clean_blob_passes_and_a_grown_one_does_not() {
        let mut blob: Vec<u8> = b"SPDRT\x01\x00\x00".to_vec();
        blob.extend_from_slice(&[0u8; 8]);
        blob.extend(
            (0..512u16)
                .flat_map(|i| i.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        let clean = audit_artifact("a.bin", &blob, Some(blob.len() as u64));
        assert!(
            clean.is_empty(),
            "unexpected findings: {:?}",
            clean.iter().map(|f| &f.reason).collect::<Vec<_>>()
        );

        let grown = audit_artifact("a.bin", &blob, Some(blob.len() as u64 - 1));
        assert_eq!(grown.len(), 1);
        assert!(grown[0].reason.contains("baseline"));

        let unrecorded = audit_artifact("a.bin", &blob, None);
        assert_eq!(unrecorded.len(), 1);
        assert!(unrecorded[0].reason.contains("no size baseline"));
    }

    #[test]
    fn header_bytes_are_allowed_to_hold_text() {
        let mut blob: Vec<u8> = b"SPIDER-ROUTE-V1\x00".to_vec();
        assert_eq!(blob.len(), ARTIFACT_HEADER_BYTES);
        blob.extend_from_slice(&[0u8, 1, 2, 3, 0, 4, 5]);
        assert!(audit_artifact("a.bin", &blob, Some(blob.len() as u64)).is_empty());
    }

    #[test]
    fn workspace_members_are_read_without_a_toml_parser() {
        let manifest = "[workspace]\nmembers = [\"spider-cloud-agent\", \"spider-route\", \"xtask\"]\nresolver = \"2\"\n";
        assert_eq!(
            workspace_members(manifest),
            vec!["spider-cloud-agent", "spider-route", "xtask"]
        );
        assert!(manifest_says_unpublished(
            "[package]\nname = \"xtask\"\npublish = false\n"
        ));
        assert!(!manifest_says_unpublished(
            "[package]\nname = \"spider-route\"\n"
        ));
        assert_eq!(
            manifest_name("[package]\nname = \"spider-route\"\n").as_deref(),
            Some("spider-route")
        );
    }

    #[test]
    fn baselines_parse_and_skip_comment_keys() {
        let text = r#"{"_note":"a comment","spider-route/assets/v1.bin":{"max_bytes":1048576}}"#;
        let map = parse_baselines(text).expect("valid");
        assert_eq!(map.len(), 1);
        assert_eq!(map["spider-route/assets/v1.bin"], 1048576);
    }
}
