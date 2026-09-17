//! What is known about a site before an edit is chosen.
//!
//! An [`Observation`] carries the caller's memory of the site, the last status
//! seen, and optionally a [`ResourceSummary`] of what a previous rendered
//! attempt loaded. [`summarize`] builds that summary from the page's own
//! resource list.
//!
//! The summary is the one place in this crate that holds a host. It groups
//! resources by registrable domain and keeps the heaviest third party groups
//! as [`Identifier`]s, whose `pattern` is the text a blacklist edit writes.
//! Everything else about an identifier is a bucket, and only the buckets reach
//! a feature slot or a row.

use spider_route::features::extension_of;
use spider_route::{registrable_domain, ExtClass, SiteMemory, StatusClass};
use url::Url;

/// The most third party groups a summary keeps as candidates.
pub const MAX_IDENTIFIERS: usize = 4;

/// A third party resource group an edit could block.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identifier {
    /// The registrable domain of the group. Written into `network_blacklist`
    /// by an append edit, and never into a feature, a row or a log line this
    /// crate writes.
    pub pattern: String,
    /// The kind of file the largest resource in the group was.
    pub class: ExtClass,
    /// The group's share of all bytes the page loaded, bucketed: under 1%,
    /// under 5%, under 15%, under 40%, 40% or more (0 to 4).
    pub share_bucket: u8,
    /// How many requests the group made, bucketed: 1, 2 to 3, 4 to 9, 10 to
    /// 29, 30 or more (0 to 4).
    pub count_bucket: u8,
    /// Whether the group is outside the page's own registrable domain. Always
    /// true for an identifier [`summarize`] builds.
    pub third_party: bool,
}

/// What a previous attempt loaded, in aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceSummary {
    /// How many resources were loaded, capped at `u16` range.
    pub requests: u16,
    /// How many distinct third party registrable domains, capped.
    pub third_party_hosts: u16,
    /// Bytes from outside the page's registrable domain.
    pub third_party_bytes: u64,
    /// Bytes from the page's own registrable domain.
    pub first_party_bytes: u64,
    /// The heaviest third party groups, at most [`MAX_IDENTIFIERS`], heaviest
    /// first.
    pub candidates: Vec<Identifier>,
}

/// Everything an edit is chosen from, besides the request itself.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Observation {
    /// What the caller remembers about the site.
    pub memory: Option<SiteMemory>,
    /// What a previous attempt loaded, when one was rendered and reported.
    pub resources: Option<ResourceSummary>,
    /// How the last attempt ended.
    pub last_status: StatusClass,
}

impl Observation {
    /// Nothing known.
    pub const fn cold() -> Observation {
        Observation {
            memory: None,
            resources: None,
            last_status: StatusClass::Unknown,
        }
    }

    /// Whether the site asked for a slower pace last time, by this
    /// observation's own status or by the caller's memory of the site.
    pub fn rate_limited(&self) -> bool {
        self.last_status == StatusClass::RateLimited
            || self
                .memory
                .is_some_and(|memory| memory.last_status == StatusClass::RateLimited)
    }

    /// The observed identifier whose pattern is exactly this text.
    pub fn identifier(&self, pattern: &str) -> Option<(usize, &Identifier)> {
        self.resources
            .as_ref()?
            .candidates
            .iter()
            .enumerate()
            .find(|(_, ident)| ident.pattern == pattern)
    }
}

/// One registrable domain's resources while summarizing.
struct Group<'a> {
    domain: &'a str,
    bytes: u64,
    count: u32,
    largest: u64,
    class: ExtClass,
}

/// Group a page's resources by registrable domain and keep the heaviest third
/// party groups.
///
/// `resources` is each loaded resource with its size in bytes. The page's own
/// registrable domain is first party. A resource with no registrable domain,
/// such as a literal address, counts toward the totals and is never a
/// candidate, because there is no name to block it by. The class of a group is
/// read from the path of its largest resource with the router's public
/// extension classifier. Ranking is by bytes, then request count, then the
/// domain text, so the result is the same for the same input.
pub fn summarize(page: &Url, resources: &[(Url, u64)]) -> ResourceSummary {
    let own = registrable_domain(page);
    let mut groups: Vec<Group<'_>> = Vec::new();
    let mut first_party_bytes = 0u64;
    let mut third_party_bytes = 0u64;

    for (url, bytes) in resources {
        let domain = registrable_domain(url);
        let first_party = match (domain, own) {
            (Some(domain), Some(own)) => domain.eq_ignore_ascii_case(own),
            (None, None) => true,
            _ => false,
        };

        if first_party {
            first_party_bytes = first_party_bytes.saturating_add(*bytes);
            continue;
        }
        third_party_bytes = third_party_bytes.saturating_add(*bytes);

        let Some(domain) = domain else {
            continue;
        };
        match groups.iter_mut().find(|group| group.domain == domain) {
            Some(group) => {
                group.bytes = group.bytes.saturating_add(*bytes);
                group.count = group.count.saturating_add(1);
                if *bytes > group.largest {
                    group.largest = *bytes;
                    group.class = extension_of(url);
                }
            }
            None => groups.push(Group {
                domain,
                bytes: *bytes,
                count: 1,
                largest: *bytes,
                class: extension_of(url),
            }),
        }
    }

    groups.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then(b.count.cmp(&a.count))
            .then(a.domain.cmp(b.domain))
    });

    let total = first_party_bytes.saturating_add(third_party_bytes);
    let candidates = groups
        .iter()
        .take(MAX_IDENTIFIERS)
        .map(|group| Identifier {
            pattern: group.domain.to_string(),
            class: group.class,
            share_bucket: share_bucket(group.bytes, total),
            count_bucket: count_bucket(group.count),
            third_party: true,
        })
        .collect();

    ResourceSummary {
        requests: resources.len().min(65_535) as u16,
        third_party_hosts: groups.len().min(65_535) as u16,
        third_party_bytes,
        first_party_bytes,
        candidates,
    }
}

/// A group's share of all bytes, bucketed at 1%, 5%, 15% and 40%.
///
/// Compared in integers so a byte count past what a float holds exactly
/// cannot move a group across a line.
fn share_bucket(bytes: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let scaled = bytes as u128 * 100;
    let total = total as u128;
    if scaled < total {
        0
    } else if scaled < total * 5 {
        1
    } else if scaled < total * 15 {
        2
    } else if scaled < total * 40 {
        3
    } else {
        4
    }
}

/// A group's request count, bucketed: 1, 2 to 3, 4 to 9, 10 to 29, 30 or more.
const fn count_bucket(count: u32) -> u8 {
    match count {
        0..=1 => 0,
        2..=3 => 1,
        4..=9 => 2,
        10..=29 => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;
    use crate::candidates::{Candidate, Context};
    use crate::edit::{Edit, EditSet, Op};
    use crate::features::featurize_edit;
    use crate::row::{comparison_row, Arm, ComparisonRow, EditDescriptor, MemoryState};
    use spider_cloud_agent::params::RequestParams;
    use spider_route::{featurize, DeclaredNeed, RouteDecision, RouteInput};

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn resources() -> Vec<(Url, u64)> {
        vec![
            (url("https://www.example.com/app.js"), 50_000),
            (url("https://img.example.com/hero.png"), 400_000),
            (url("https://tracker-alpha.example/t.js"), 90_000),
            (url("https://px.tracker-alpha.example/p.gif"), 100),
            (url("https://cdn-beta.example/lib/big.js"), 300_000),
            (url("https://cdn-beta.example/lib/small.css"), 1_000),
            (url("https://cdn-beta.example/lib/other.js"), 1_000),
            (url("http://192.0.2.1/beacon"), 10),
        ]
    }

    #[test]
    fn first_party_is_never_a_candidate() {
        let page = url("https://shop.example.com/item");
        let summary = summarize(&page, &resources());

        assert_eq!(summary.requests, 8);
        assert_eq!(summary.first_party_bytes, 450_000);
        assert_eq!(summary.third_party_bytes, 392_110);
        assert_eq!(summary.third_party_hosts, 2);
        let patterns: Vec<&str> = summary
            .candidates
            .iter()
            .map(|c| c.pattern.as_str())
            .collect();
        assert_eq!(patterns, ["cdn-beta.example", "tracker-alpha.example"]);
        assert!(summary.candidates.iter().all(|c| c.third_party));
        assert!(!patterns.iter().any(|p| p.contains("example.com")));

        let cdn = &summary.candidates[0];
        assert_eq!(cdn.class, ExtClass::Asset);
        // 302000 of 842110 bytes is about 36%.
        assert_eq!(cdn.share_bucket, 3);
        assert_eq!(cdn.count_bucket, 1);
        assert_eq!(summary.candidates[1].share_bucket, 2);
    }

    #[test]
    fn no_more_than_the_cap_are_kept_heaviest_first() {
        let page = url("https://example.com/");
        let many: Vec<(Url, u64)> = (0..10u64)
            .map(|n| (url(&format!("https://t{n}.example/x.js")), n * 10))
            .collect();
        let summary = summarize(&page, &many);

        assert_eq!(summary.candidates.len(), MAX_IDENTIFIERS);
        assert_eq!(summary.third_party_hosts, 10);
        assert_eq!(summary.candidates[0].pattern, "t9.example");
        assert_eq!(summarize(&page, &[]), ResourceSummary::default());
    }

    #[test]
    fn a_host_never_reaches_features_or_rows() {
        let page = url("https://example.com/");
        let summary = summarize(&page, &resources());
        let hosts = ["tracker-alpha", "cdn-beta"];
        assert_eq!(summary.candidates.len(), 2);

        let observation = Observation {
            resources: Some(summary.clone()),
            ..Observation::cold()
        };
        let current = RequestParams {
            request: Some(spider_route::RequestMode::Browser),
            disable_hints: Some(true),
            ..RequestParams::default()
        };
        let caller = RequestParams::default();
        let routed = RouteDecision::default();
        let ctx = Context {
            url: &page,
            need: DeclaredNeed::Markdown,
            current: &current,
            caller: &caller,
            routed: &routed,
            observation: &observation,
            multiplier_cap: 8.0,
        };
        let base = featurize(&RouteInput::new(&page, DeclaredNeed::Markdown));

        for (rank, ident) in summary.candidates.iter().enumerate() {
            let edits = EditSet::new(vec![Edit {
                key: crate::schema::Key::NetworkBlacklist,
                op: Op::Append(vec![ident.pattern.clone()]),
            }])
            .unwrap();
            let cand = Candidate {
                edits,
                multiplier: 4.0,
            };
            let feats = featurize_edit(&ctx, &cand);
            let debug = format!("{feats:?}");
            let descriptor = EditDescriptor::describe(&cand.edits, &observation);
            assert!(descriptor.and_then(|d| d.ident).is_some(), "rank {rank}");

            let row = comparison_row(&ComparisonRow {
                pair: 1,
                arm: Arm::Candidate,
                day: 2,
                domain_key: 3,
                need: DeclaredNeed::Markdown,
                ext: ExtClass::None,
                tld: 0,
                memory: MemoryState::Cold,
                routed: &routed,
                edit: descriptor,
                pinned: 0,
                pinned_hi: 0,
                success: true,
                status: StatusClass::Ok,
                millis: 10,
                bytes: 10,
                credits: 1.0,
                attempts: 1,
                multiplier: cand.multiplier,
                fields_requested: 0,
                fields_present: 0,
                content_ok: None,
                fields_ok: None,
                shingle_jaccard: None,
                byte_ratio: None,
                base: &base,
                edit_feats: &feats,
            });

            // The identifier did reach the features, as buckets.
            assert_ne!(
                feats,
                featurize_edit(
                    &ctx,
                    &Candidate {
                        edits: EditSet::keep(),
                        multiplier: 4.0,
                    }
                )
            );
            for host in hosts {
                assert!(
                    !debug.contains(host),
                    "{host} reached the feature debug text"
                );
                assert!(!row.contains(host), "{host} reached the row: {row}");
            }
            assert!(!row.contains(".example"), "a suffix reached the row: {row}");
        }
    }
}
