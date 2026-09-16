//! What a crawl is allowed to reach, and what it does with the links it finds.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Per-resource caps for a crawl, keyed by path.
///
/// The key `"*"` caps the crawl as a whole and any other key caps one path, so
/// `{"*": 500, "/blog": 50}` visits five hundred pages and stops after fifty of
/// them are blog posts. This is the cap that keeps a crawl of an unfamiliar
/// site from finding a calendar and never coming back.
pub type CrawlBudget = BTreeMap<String, u32>;

/// Change a link before it is followed or returned.
///
/// One rule applies to every link found. Use it when the site publishes links
/// through a host you cannot fetch, such as a staging domain or a redirect
/// wrapper, and you want the real target instead.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum LinkRewriteRule {
    /// Swap one piece of text for another.
    #[serde(rename = "replace")]
    Replace {
        /// Only rewrite links on this host. Leave it out to rewrite them all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        /// The text to look for.
        find: String,
        /// What it becomes.
        replace_with: String,
    },
    /// Swap whatever a pattern matches.
    #[serde(rename = "regex")]
    Regex {
        /// Only rewrite links on this host. Leave it out to rewrite them all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        /// The pattern to match.
        pattern: String,
        /// What a match becomes. Capture groups can be referred to here.
        replace_with: String,
    },
}

// Payloads can contain login input, script literals or credential-bearing URLs.
impl std::fmt::Debug for LinkRewriteRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variant = match self {
            Self::Replace { .. } => "Replace",
            Self::Regex { .. } => "Regex",
        };
        f.debug_tuple(variant).field(&"<redacted>").finish()
    }
}

impl LinkRewriteRule {
    /// Swap `find` for `replace_with` in every link.
    pub fn replace(find: impl Into<String>, replace_with: impl Into<String>) -> LinkRewriteRule {
        LinkRewriteRule::Replace {
            host: None,
            find: find.into(),
            replace_with: replace_with.into(),
        }
    }

    /// Swap whatever `pattern` matches in every link.
    pub fn regex(pattern: impl Into<String>, replace_with: impl Into<String>) -> LinkRewriteRule {
        LinkRewriteRule::Regex {
            host: None,
            pattern: pattern.into(),
            replace_with: replace_with.into(),
        }
    }

    /// Narrow an existing rule to links on one host.
    pub fn on_host(self, host: impl Into<String>) -> LinkRewriteRule {
        match self {
            LinkRewriteRule::Replace {
                find, replace_with, ..
            } => LinkRewriteRule::Replace {
                host: Some(host.into()),
                find,
                replace_with,
            },
            LinkRewriteRule::Regex {
                pattern,
                replace_with,
                ..
            } => LinkRewriteRule::Regex {
                host: Some(host.into()),
                pattern,
                replace_with,
            },
        }
    }
}
