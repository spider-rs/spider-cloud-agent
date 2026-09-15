//! The word denylist used by `leakcheck`.
//!
//! This repo is going public, so the real denylist does not live here. The built-in
//! list below holds only terms that are safe to read in a public repo: host suffixes,
//! a shipped-default hostname, and obvious placeholders. The real list, with the
//! internal service, cluster, queue, engine and codename vocabulary spelled out,
//! lives in the private checkout and is applied in CI by pointing
//! `SPIDER_LEAKCHECK_WORDS` at it.
//!
//! Extra list format: one term per line. Blank lines and lines starting with `#` are
//! ignored. A term may be prefixed with `category:` to label it in the output, for
//! example `queue:some-internal-queue-name`.

use std::path::Path;

/// Where a term is worth failing on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// Anywhere in the packaged files, including docs and tests.
    Anywhere,
    /// Only in shipped library source, meaning a path under `src/`. Used for terms
    /// that are fine in a test or an example but wrong as a default the crate ships.
    ShippedSource,
}

impl Scope {
    /// True when a term with this scope should be checked in the given file.
    pub fn applies_to(self, path: &Path) -> bool {
        match self {
            Scope::Anywhere => true,
            Scope::ShippedSource => {
                let p = path.to_string_lossy().replace('\\', "/");
                p.contains("/src/") && !p.contains("/tests/")
            }
        }
    }
}

/// One denylist term with the reason it is on the list.
#[derive(Clone, Debug)]
pub struct Word {
    pub term: String,
    pub category: String,
    pub scope: Scope,
}

/// A group of terms that share a reason.
pub struct Category {
    pub name: &'static str,
    /// What this group is for, printed with `leakcheck --explain`.
    pub note: &'static str,
    pub scope: Scope,
    pub terms: &'static [&'static str],
}

/// The categories the private list is expected to fill in. The terms stay empty here
/// on purpose: naming an internal service in a public repo is the leak this tool is
/// meant to stop.
pub const CATEGORIES: &[Category] = &[
    Category {
        name: "service",
        note: "internal service and cluster names, the things our own deploys talk to",
        scope: Scope::Anywhere,
        terms: &[],
    },
    Category {
        name: "queue",
        note: "internal queue, topic and stream names",
        scope: Scope::Anywhere,
        terms: &[],
    },
    Category {
        name: "engine",
        note: "internal engine and escalation vocabulary, including any name for the browser lane",
        scope: Scope::Anywhere,
        terms: &[],
    },
    Category {
        name: "codename",
        note: "internal project codenames",
        scope: Scope::Anywhere,
        terms: &[],
    },
    Category {
        name: "host",
        note: "host suffixes that only resolve inside our network",
        scope: Scope::Anywhere,
        terms: &[".internal", ".cluster.local", ".svc.cluster.local"],
    },
    Category {
        name: "default",
        note: "hostnames that are fine in a test but wrong as a value the crate ships",
        scope: Scope::ShippedSource,
        terms: &["localhost"],
    },
    Category {
        name: "placeholder",
        note: "placeholders left behind by a copy and paste, which usually means a real value was there",
        scope: Scope::Anywhere,
        terms: &[
            "changeme",
            "change_me",
            "replace_me",
            "replace-me",
            "your-api-key-here",
            "xxxxxxxx",
        ],
    },
];

/// The environment variable that points at the private denylist.
pub const EXTRA_LIST_ENV: &str = "SPIDER_LEAKCHECK_WORDS";

/// Built-in terms plus, when `SPIDER_LEAKCHECK_WORDS` is set, the terms in that file.
pub fn load() -> Result<Vec<Word>, String> {
    let mut words = builtin();
    if let Some(path) = std::env::var_os(EXTRA_LIST_ENV) {
        let path = std::path::PathBuf::from(path);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "cannot read the extra denylist at {}: {e}. Point {} at a readable file or unset it.",
                path.display(),
                EXTRA_LIST_ENV
            )
        })?;
        words.extend(parse_extra(&text));
    }
    Ok(words)
}

/// The terms compiled into this binary.
pub fn builtin() -> Vec<Word> {
    let mut out = Vec::new();
    for cat in CATEGORIES {
        for term in cat.terms {
            out.push(Word {
                term: term.to_ascii_lowercase(),
                category: cat.name.to_string(),
                scope: cat.scope,
            });
        }
    }
    out
}

/// Parse the extra denylist file.
pub fn parse_extra(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (category, term) = match line.split_once(':') {
            Some((c, t)) if !c.contains(char::is_whitespace) && !t.trim().is_empty() => {
                (c.trim().to_string(), t.trim())
            }
            _ => ("private".to_string(), line),
        };
        out.push(Word {
            term: term.to_ascii_lowercase(),
            category,
            scope: Scope::Anywhere,
        });
    }
    out
}

/// Byte offsets in `line` where `term` appears, matched without case and only on a
/// word boundary, so `cluster` does not hit inside `clusters` but does hit inside
/// `cache-cluster`.
pub fn find_term(line: &str, term: &str) -> Vec<usize> {
    let hay = line.to_ascii_lowercase();
    let mut hits = Vec::new();
    if term.is_empty() {
        return hits;
    }
    let bytes = hay.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = hay[from..].find(term) {
        let start = from + rel;
        let end = start + term.len();
        let left_ok = !starts_word(term) || start == 0 || !is_word_byte(bytes[start - 1]);
        let right_ok = !ends_word(term) || end >= bytes.len() || !is_word_byte(bytes[end]);
        if left_ok && right_ok {
            hits.push(start);
        }
        from = start + 1;
    }
    hits
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn starts_word(term: &str) -> bool {
    term.as_bytes().first().is_some_and(|b| is_word_byte(*b))
}

fn ends_word(term: &str) -> bool {
    term.as_bytes().last().is_some_and(|b| is_word_byte(*b))
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
    fn finds_a_planted_term_from_the_extra_list() {
        let words = parse_extra("queue:pluto-ingest\n# a comment\n\nlone-term\n");
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].category, "queue");
        assert_eq!(words[0].term, "pluto-ingest");
        assert_eq!(words[1].category, "private");

        let line = "const QUEUE: &str = \"pluto-ingest\";";
        assert_eq!(find_term(line, &words[0].term).len(), 1);
    }

    #[test]
    fn matching_ignores_case_and_respects_word_edges() {
        assert_eq!(find_term("Pluto-Ingest here", "pluto-ingest").len(), 1);
        assert!(find_term("the clusters moved", "cluster").is_empty());
        assert_eq!(find_term("the cache-cluster moved", "cluster").len(), 1);
    }

    #[test]
    fn a_dotted_suffix_matches_at_the_end_of_a_host() {
        assert_eq!(find_term("host = example.internal", ".internal").len(), 1);
        assert!(find_term("host = example.internalized", ".internal").is_empty());
    }

    #[test]
    fn shipped_source_scope_skips_tests_and_docs() {
        let shipped = Path::new("spider-cloud-agent/src/client.rs");
        let test = Path::new("spider-cloud-agent/tests/endpoints.rs");
        let doc = Path::new("README.md");
        assert!(Scope::ShippedSource.applies_to(shipped));
        assert!(!Scope::ShippedSource.applies_to(test));
        assert!(!Scope::ShippedSource.applies_to(doc));
        assert!(Scope::Anywhere.applies_to(doc));
    }

    #[test]
    fn the_builtin_list_names_no_internal_terms() {
        for cat in CATEGORIES {
            if matches!(cat.name, "service" | "queue" | "engine" | "codename") {
                assert!(
                    cat.terms.is_empty(),
                    "category {} must stay empty in the public repo",
                    cat.name
                );
            }
        }
    }
}
