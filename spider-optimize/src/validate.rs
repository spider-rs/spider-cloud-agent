//! Whether a candidate may be applied at all, before anything scores it.
//!
//! The checks run in a fixed order and the first failure is the answer, so a
//! rejection names the most basic thing wrong with a candidate: a key the
//! caller set, then a key that changes content, then a key or operation this
//! version does not learn, then a value outside its kind, then a setting that
//! does nothing on the request it lands on, then the service's own hints,
//! then a rate limit, then the budget. Keep is never rejected.
//!
//! # The content exception
//!
//! Version one refuses every edit on a `content_changing` key except two:
//! `block_stylesheets` and `network_blacklist`. Both are learnable, and both
//! can remove what a page shows, which is exactly why the label pipeline
//! compares a candidate's content against its baseline with
//! [`crate::labels`] before a row counts as a success. Any other content
//! changing key would change what the caller asked for rather than how it is
//! fetched, and no label can say that was fine.

use crate::candidates::{
    current_mode, current_proxy, current_wait, effective_flag, mode_rank, multiplier, proxy_rank,
    resulting, Candidate, Context,
};
use crate::edit::{EditSet, Op, Value};
use crate::observe::MAX_IDENTIFIERS;
use crate::schema::{Dependency, Key, Kind, Schema, MODES, PROXIES};
use spider_route::RequestMode;
use std::fmt;

/// Why a candidate was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rejection {
    /// The caller set this field.
    PinnedByCaller(Key),
    /// This version does not edit this field, or not with this operation.
    Unsupported(Key),
    /// The value is outside what this field's kind allows.
    OutOfRange(Key),
    /// The field does nothing on the request as edited.
    Dependency(Key),
    /// The site asked for a slower pace, and this candidate is heavier than
    /// the request as it stands.
    RateLimited,
    /// The candidate costs more than the caller's budget allows.
    OverBudget,
    /// Changing this field can change what comes back.
    ContentChanging(Key),
    /// A blacklist edit while the service applies its own blocking hints, so
    /// what the edit did could not be told apart from what the service did.
    HintsActive,
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejection::PinnedByCaller(key) => write!(f, "the caller set {}", key.wire()),
            Rejection::Unsupported(key) => write!(f, "{} is not edited this way", key.wire()),
            Rejection::OutOfRange(key) => write!(f, "the value for {} is out of range", key.wire()),
            Rejection::Dependency(key) => {
                write!(f, "{} does nothing on this request", key.wire())
            }
            Rejection::RateLimited => f.write_str("heavier settings under a rate limit"),
            Rejection::OverBudget => f.write_str("dearer than the budget allows"),
            Rejection::ContentChanging(key) => {
                write!(f, "{} can change the content", key.wire())
            }
            Rejection::HintsActive => {
                f.write_str("a blacklist edit needs the service's hints turned off")
            }
        }
    }
}

/// The content changing keys version one still edits. See the module docs.
const CONTENT_EXCEPTIONS: [Key; 2] = [Key::BlockStylesheets, Key::NetworkBlacklist];

/// Check one candidate against the schema and the context.
///
/// Allocates nothing.
pub fn validate(schema: &Schema, ctx: &Context<'_>, cand: &Candidate) -> Result<(), Rejection> {
    let edits = &cand.edits;
    if edits.is_keep() {
        return Ok(());
    }

    for edit in edits.edits() {
        let key = edit.key;
        let spec = schema.spec(key);

        if ctx.caller.is_set(key) {
            return Err(Rejection::PinnedByCaller(key));
        }
        if key == Key::NetworkBlacklist && ctx.caller.is_set(Key::NetworkWhitelist) {
            return Err(Rejection::PinnedByCaller(Key::NetworkWhitelist));
        }
        if spec.content_changing && !CONTENT_EXCEPTIONS.contains(&key) {
            return Err(Rejection::ContentChanging(key));
        }
        if !spec.learnable {
            return Err(Rejection::Unsupported(key));
        }

        match (spec.kind, &edit.op) {
            (Kind::Enum(_), Op::Set(Value::Mode(mode))) if key == Key::Request => {
                if !MODES.contains(mode) {
                    return Err(Rejection::OutOfRange(key));
                }
            }
            (Kind::Enum(_), Op::Set(Value::Proxy(pool))) if key == Key::Proxy => {
                if !PROXIES.contains(pool) {
                    return Err(Rejection::OutOfRange(key));
                }
            }
            (Kind::MillisBucket(buckets), Op::Set(Value::Millis(millis))) => {
                if !buckets.contains(millis) {
                    return Err(Rejection::OutOfRange(key));
                }
            }
            (Kind::Bool, Op::Set(Value::Bool(_))) => {}
            // Appending is the one list edit in version one.
            (Kind::StringList, Op::Append(entries)) => {
                if entries.is_empty()
                    || entries.len() > MAX_IDENTIFIERS
                    || !entries.iter().all(|entry| is_pattern(entry))
                {
                    return Err(Rejection::OutOfRange(key));
                }
            }
            _ => return Err(Rejection::Unsupported(key)),
        }
    }

    let (mode, _, _) = resulting(ctx, edits);
    for edit in edits.edits() {
        for dependency in schema.spec(edit.key).requires {
            let met = match dependency {
                Dependency::Rendered => {
                    matches!(mode, RequestMode::Smart | RequestMode::Browser)
                }
                // An edit that switches its own field off cannot conflict
                // with the other one, so only an edit that leaves its field
                // on is held to this.
                Dependency::NotWith(other) => {
                    !active(ctx, edits, edit.key) || !active(ctx, edits, *other)
                }
            };
            if !met {
                return Err(Rejection::Dependency(edit.key));
            }
        }
    }

    if edits.get(Key::NetworkBlacklist).is_some()
        && ctx.current.flag(Key::DisableHints) != Some(true)
    {
        return Err(Rejection::HintsActive);
    }

    if ctx.observation.rate_limited() && heavier(ctx, edits) {
        return Err(Rejection::RateLimited);
    }

    // A cap that is not a number refuses rather than allows.
    if ctx.multiplier_cap.is_nan() || multiplier(ctx, edits) > ctx.multiplier_cap {
        return Err(Rejection::OverBudget);
    }

    Ok(())
}

/// Whether text can go into the blacklist as one pattern: not empty, no
/// whitespace or control characters, and no longer than a host name.
fn is_pattern(entry: &str) -> bool {
    !entry.is_empty()
        && entry.len() <= 253
        && !entry
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}

/// Whether a field is switched on in the request once these edits apply.
fn active(ctx: &Context<'_>, edits: &EditSet, key: Key) -> bool {
    match edits.get(key).map(|edit| &edit.op) {
        Some(Op::Set(Value::Bool(value))) => *value,
        Some(Op::Append(entries)) => !entries.is_empty(),
        Some(_) => true,
        None => match key {
            Key::NetworkBlacklist | Key::NetworkWhitelist => {
                ctx.current.list(key).is_some_and(|list| !list.is_empty())
            }
            _ => effective_flag(ctx.current, key),
        },
    }
}

/// Whether these edits ask the site for more than the request as it stands.
///
/// A dearer mode, pool or price, a longer wait, or letting the page load
/// something it would not have loaded. Blocking more is never heavier.
pub(crate) fn heavier(ctx: &Context<'_>, edits: &EditSet) -> bool {
    let (mode, proxy, wait) = resulting(ctx, edits);
    if mode_rank(mode) > mode_rank(current_mode(ctx.current))
        || proxy_rank(proxy) > proxy_rank(current_proxy(ctx.current))
        || wait > current_wait(ctx.current)
        || multiplier(ctx, edits) > multiplier(ctx, &EditSet::keep())
    {
        return true;
    }

    edits.edits().iter().any(|edit| match (&edit.op, edit.key) {
        (Op::Set(Value::Bool(value)), Key::FullResources | Key::DisableIntercept) => {
            *value && !effective_flag(ctx.current, edit.key)
        }
        (
            Op::Set(Value::Bool(value)),
            Key::BlockAds | Key::BlockAnalytics | Key::BlockStylesheets,
        ) => !*value && effective_flag(ctx.current, edit.key),
        _ => false,
    })
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
    use crate::candidates::generate;
    use crate::edit::Edit;
    use crate::testing::Fixture;
    use spider_route::{ProxyPool, StatusClass};

    fn cand(ctx: &Context<'_>, edits: Vec<Edit>) -> Candidate {
        let edits = EditSet::new(edits).unwrap();
        Candidate {
            multiplier: multiplier(ctx, &edits),
            edits,
        }
    }

    fn check(fixture: &Fixture, edits: Vec<Edit>) -> Result<(), Rejection> {
        let ctx = fixture.ctx();
        validate(&Schema::v1(), &ctx, &cand(&ctx, edits))
    }

    fn append(pattern: &str) -> Edit {
        Edit {
            key: Key::NetworkBlacklist,
            op: Op::Append(vec![pattern.into()]),
        }
    }

    #[test]
    fn a_pinned_key_is_rejected_by_name() {
        let fixture = Fixture::pinned();
        assert_eq!(
            check(
                &fixture,
                vec![Edit::set(Key::Request, Value::Mode(RequestMode::Http))]
            ),
            Err(Rejection::PinnedByCaller(Key::Request))
        );
        let mut whitelisted = Fixture::observed();
        whitelisted.caller.network_whitelist = Some(vec!["example.com".into()]);
        assert_eq!(
            check(&whitelisted, vec![append("cdn-beta.example")]),
            Err(Rejection::PinnedByCaller(Key::NetworkWhitelist))
        );
    }

    #[test]
    fn rate_limit_rejects_every_heavier_candidate() {
        let schema = Schema::v1();
        let mut limited = Fixture::observed();
        limited.observation.last_status = StatusClass::RateLimited;
        let calm = Fixture::observed();

        // Every candidate the calm request would get, checked under a limit.
        let everything = generate(&schema, &calm.ctx());
        let ctx = limited.ctx();
        let mut heavier_seen = 0;
        let mut lighter_seen = 0;
        for candidate in &everything {
            if heavier(&ctx, &candidate.edits) {
                heavier_seen += 1;
                assert_eq!(
                    validate(&schema, &ctx, candidate),
                    Err(Rejection::RateLimited),
                    "{:?}",
                    candidate.edits
                );
            } else {
                lighter_seen += 1;
                assert_eq!(validate(&schema, &ctx, candidate), Ok(()));
            }
        }
        assert!(heavier_seen >= 8, "only {heavier_seen} heavier candidates");
        // Keep, a plain fetch, and blocking more survive.
        assert!(lighter_seen >= 3, "only {lighter_seen} lighter candidates");

        for candidate in generate(&schema, &ctx) {
            assert!(!heavier(&ctx, &candidate.edits), "{:?}", candidate.edits);
        }

        // The memory's last status counts the same as the observation's.
        let mut remembered = Fixture::observed();
        remembered.observation.memory = Some(spider_route::SiteMemory {
            last_status: StatusClass::RateLimited,
            ..spider_route::SiteMemory::cold()
        });
        assert_eq!(
            check(
                &remembered,
                vec![Edit::set(Key::Proxy, Value::Proxy(ProxyPool::Residential))]
            ),
            Err(Rejection::RateLimited)
        );
    }

    #[test]
    fn a_dependency_violation_is_rejected() {
        let mut fixture = Fixture::cold();
        fixture.caller.request = Some(RequestMode::Http);
        fixture.current.request = Some(RequestMode::Http);

        assert_eq!(
            check(
                &fixture,
                vec![Edit::set(Key::WaitIdleMillis, Value::Millis(5_000))]
            ),
            Err(Rejection::Dependency(Key::WaitIdleMillis))
        );
        assert_eq!(
            check(&fixture, vec![Edit::set(Key::BlockAds, Value::Bool(false))]),
            Err(Rejection::Dependency(Key::BlockAds))
        );

        // A blacklist append beside `disable_intercept` is allowed in both
        // directions.
        let mut observed = Fixture::observed();
        observed.current.disable_intercept = Some(true);
        assert_eq!(check(&observed, vec![append("cdn-beta.example")]), Ok(()));
        let mut listed = Fixture::observed();
        listed.current.network_blacklist = Some(vec!["cdn-beta.example".into()]);
        assert_eq!(
            check(
                &listed,
                vec![Edit::set(Key::DisableIntercept, Value::Bool(true))]
            ),
            Ok(())
        );
        let rendered = Fixture::cold();
        assert_eq!(
            check(
                &rendered,
                vec![Edit::set(Key::WaitIdleMillis, Value::Millis(5_000))]
            ),
            Ok(())
        );
    }

    #[test]
    fn blacklist_needs_disable_hints() {
        let mut fixture = Fixture::observed();
        assert_eq!(check(&fixture, vec![append("cdn-beta.example")]), Ok(()));

        fixture.current.disable_hints = None;
        assert_eq!(
            check(&fixture, vec![append("cdn-beta.example")]),
            Err(Rejection::HintsActive)
        );
        fixture.current.disable_hints = Some(false);
        assert_eq!(
            check(&fixture, vec![append("cdn-beta.example")]),
            Err(Rejection::HintsActive)
        );
        assert!(generate(&Schema::v1(), &fixture.ctx())
            .iter()
            .all(|c| c.edits.get(Key::NetworkBlacklist).is_none()));
    }

    #[test]
    fn over_budget_is_rejected() {
        let mut fixture = Fixture::cold();
        fixture.cap = 3.0;
        let browser = vec![Edit::set(Key::Request, Value::Mode(RequestMode::Browser))];
        assert_eq!(check(&fixture, browser.clone()), Err(Rejection::OverBudget));

        fixture.cap = 4.0;
        assert_eq!(check(&fixture, browser.clone()), Ok(()));

        fixture.cap = f32::NAN;
        assert_eq!(check(&fixture, browser), Err(Rejection::OverBudget));
        assert!(generate(&Schema::v1(), &fixture.ctx()).len() == 1);
    }

    #[test]
    fn content_values_and_operations_outside_version_one_are_refused() {
        let fixture = Fixture::observed();
        assert_eq!(
            check(
                &fixture,
                vec![Edit::set(Key::Readability, Value::Bool(true))]
            ),
            Err(Rejection::ContentChanging(Key::Readability))
        );
        assert_eq!(
            check(&fixture, vec![Edit::set(Key::Stealth, Value::Bool(true))]),
            Err(Rejection::Unsupported(Key::Stealth))
        );
        assert_eq!(
            check(
                &fixture,
                vec![Edit::set(Key::WaitIdleMillis, Value::Millis(3_000))]
            ),
            Err(Rejection::OutOfRange(Key::WaitIdleMillis))
        );
        assert_eq!(
            check(&fixture, vec![Edit::set(Key::Request, Value::Bool(true))]),
            Err(Rejection::Unsupported(Key::Request))
        );
        assert_eq!(
            check(
                &fixture,
                vec![Edit {
                    key: Key::NetworkBlacklist,
                    op: Op::Clear,
                }]
            ),
            Err(Rejection::Unsupported(Key::NetworkBlacklist))
        );
        assert_eq!(
            check(&fixture, vec![append("two words")]),
            Err(Rejection::OutOfRange(Key::NetworkBlacklist))
        );
        assert_eq!(
            check(
                &fixture,
                vec![Edit::set(Key::BlockStylesheets, Value::Bool(false))]
            ),
            Ok(())
        );
    }
}
