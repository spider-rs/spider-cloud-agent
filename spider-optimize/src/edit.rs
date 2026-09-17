//! A change to a request, and writing it.
//!
//! An [`EditSet`] is up to [`MAX_EDITS`] edits on distinct keys, kept in key
//! order so two sets holding the same edits compare equal. The empty set is
//! [`EditSet::keep`], which leaves the request exactly as it is.
//!
//! [`EditSet::apply`] holds the precedence rule the client's router already
//! follows: the caller beats everything. A field set on the caller's own
//! snapshot of the request is never written, whatever the router or the plan
//! wrote into the request since.

use crate::params::Params;
use crate::schema::Key;
use spider_route::{ProxyPool, RequestMode};
use std::fmt;

/// A value an edit sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Value {
    /// For a `Bool` field.
    Bool(bool),
    /// For `request`.
    Mode(RequestMode),
    /// For `proxy`.
    Proxy(ProxyPool),
    /// For an idle network wait, in milliseconds.
    Millis(u32),
    /// For an `IntRange` field. No learnable key takes one in version one, so
    /// [`EditSet::apply`] writes nothing for it.
    Int(i64),
}

/// What an edit does to its field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Op {
    /// Set a single value.
    Set(Value),
    /// Add these entries to the end of a list, skipping any already there.
    Append(Vec<String>),
    /// Take these entries out of a list.
    Remove(Vec<String>),
    /// Replace the list with these entries.
    Replace(Vec<String>),
    /// Unset the list.
    Clear,
}

impl Op {
    /// The code a row records for this op: set 0, append 1, remove 2,
    /// replace 3, clear 4.
    pub const fn code(&self) -> u8 {
        match self {
            Op::Set(_) => 0,
            Op::Append(_) => 1,
            Op::Remove(_) => 2,
            Op::Replace(_) => 3,
            Op::Clear => 4,
        }
    }

    /// The name of each op, in code order.
    pub const NAMES: [&'static str; 5] = ["set", "append", "remove", "replace", "clear"];

    const fn is_list(&self) -> bool {
        !matches!(self, Op::Set(_))
    }
}

/// One change to one field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edit {
    /// The field.
    pub key: Key,
    /// The change.
    pub op: Op,
}

impl Edit {
    /// Set a field to a value.
    pub const fn set(key: Key, value: Value) -> Edit {
        Edit {
            key,
            op: Op::Set(value),
        }
    }
}

/// The most edits one set holds.
pub const MAX_EDITS: usize = 3;

/// Why a set of edits could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// Two edits name the same key.
    Duplicate(Key),
    /// More than [`MAX_EDITS`] edits.
    TooMany,
    /// A list operation on a field that is not the network blacklist.
    Unsupported(Key),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Duplicate(key) => write!(f, "two edits name {}", key.wire()),
            EditError::TooMany => write!(f, "an edit set holds at most {MAX_EDITS} edits"),
            EditError::Unsupported(key) => {
                write!(f, "{} does not take a list operation", key.wire())
            }
        }
    }
}

impl std::error::Error for EditError {}

/// Up to [`MAX_EDITS`] edits on distinct keys, in key order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct EditSet {
    edits: Vec<Edit>,
}

/// What [`EditSet::apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    /// Edits written onto the request.
    pub written: u8,
    /// Edits left out because the caller set the field.
    pub skipped_pinned: u8,
}

impl EditSet {
    /// No edits: the request goes out as it is.
    pub const fn keep() -> EditSet {
        EditSet { edits: Vec::new() }
    }

    /// Whether this is the set that changes nothing.
    pub fn is_keep(&self) -> bool {
        self.edits.is_empty()
    }

    /// Build a set, sorted by key.
    ///
    /// Refuses two edits on one key, more than [`MAX_EDITS`] edits, and a
    /// list operation on anything but [`Key::NetworkBlacklist`].
    pub fn new(mut edits: Vec<Edit>) -> Result<EditSet, EditError> {
        if edits.len() > MAX_EDITS {
            return Err(EditError::TooMany);
        }
        for edit in &edits {
            if edit.op.is_list() && edit.key != Key::NetworkBlacklist {
                return Err(EditError::Unsupported(edit.key));
            }
        }
        edits.sort_by_key(|edit| edit.key.index());
        for pair in edits.windows(2) {
            if let [first, second] = pair {
                if first.key == second.key {
                    return Err(EditError::Duplicate(first.key));
                }
            }
        }
        Ok(EditSet { edits })
    }

    /// The edits, in key order.
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    /// The edit on this key, if there is one.
    pub fn get(&self, key: Key) -> Option<&Edit> {
        self.edits.iter().find(|edit| edit.key == key)
    }

    /// Write the edits onto `params`, leaving every field the caller set.
    ///
    /// `caller` is the request as the caller left it, before the router or
    /// the plan wrote anything. A `Set` is written only where the caller's
    /// field is unset. An idle wait of zero writes nothing, because there is
    /// no wait to send. A list operation on the blacklist is left out when
    /// the caller set either network list, since the caller's whitelist wins
    /// over any blacklist entry and an edit must not argue with it.
    pub fn apply(&self, params: &mut dyn Params, caller: &dyn Params) -> Applied {
        let mut applied = Applied::default();

        for edit in &self.edits {
            let pinned = match edit.key {
                Key::NetworkBlacklist => {
                    caller.is_set(Key::NetworkBlacklist) || caller.is_set(Key::NetworkWhitelist)
                }
                key => caller.is_set(key),
            };
            if pinned {
                applied.skipped_pinned = applied.skipped_pinned.saturating_add(1);
                continue;
            }

            let wrote = match (&edit.op, edit.key) {
                (Op::Set(Value::Mode(mode)), Key::Request) => {
                    params.set_request(*mode);
                    true
                }
                (Op::Set(Value::Proxy(pool)), Key::Proxy) => {
                    params.set_proxy(*pool);
                    true
                }
                (Op::Set(Value::Millis(millis)), Key::WaitIdleMillis) => {
                    if *millis > 0 {
                        params.set_idle_wait(*millis);
                    }
                    *millis > 0
                }
                (Op::Set(Value::Bool(value)), key) => {
                    params.set_flag(key, *value);
                    true
                }
                (Op::Append(entries), Key::NetworkBlacklist) => {
                    let mut list: Vec<String> = params
                        .list(Key::NetworkBlacklist)
                        .map(<[String]>::to_vec)
                        .unwrap_or_default();
                    for entry in entries {
                        if !list.contains(entry) {
                            list.push(entry.clone());
                        }
                    }
                    params.set_list(Key::NetworkBlacklist, Some(list));
                    true
                }
                (Op::Remove(entries), Key::NetworkBlacklist) => {
                    let list: Vec<String> = params
                        .list(Key::NetworkBlacklist)
                        .unwrap_or_default()
                        .iter()
                        .filter(|entry| !entries.contains(entry))
                        .cloned()
                        .collect();
                    params.set_list(Key::NetworkBlacklist, non_empty(list));
                    true
                }
                (Op::Replace(entries), Key::NetworkBlacklist) => {
                    let mut list: Vec<String> = Vec::with_capacity(entries.len());
                    for entry in entries {
                        if !list.contains(entry) {
                            list.push(entry.clone());
                        }
                    }
                    params.set_list(Key::NetworkBlacklist, non_empty(list));
                    true
                }
                (Op::Clear, Key::NetworkBlacklist) => {
                    params.set_list(Key::NetworkBlacklist, None);
                    true
                }
                // A value of the wrong type for its key, or an integer, which
                // no learnable key takes. Validation refuses both before an
                // edit gets here, so nothing is written.
                _ => false,
            };

            if wrote {
                applied.written = applied.written.saturating_add(1);
            }
        }

        applied
    }
}

/// An empty list is sent as no list at all.
fn non_empty(list: Vec<String>) -> Option<Vec<String>> {
    if list.is_empty() {
        None
    } else {
        Some(list)
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
    use crate::schema::{Schema, MODES, PROXIES, WAIT_BUCKETS};
    use spider_cloud_agent::params::{RequestParams, Timeout, WaitFor};

    fn append(entries: &[&str]) -> EditSet {
        EditSet::new(vec![Edit {
            key: Key::NetworkBlacklist,
            op: Op::Append(entries.iter().map(|e| e.to_string()).collect()),
        }])
        .unwrap()
    }

    #[test]
    fn append_keeps_existing_entries_and_dedupes() {
        let caller = RequestParams::default();
        // Written by the router or the plan, not the caller.
        let mut params = RequestParams {
            network_blacklist: Some(vec!["a.example".into(), "b.example".into()]),
            ..RequestParams::default()
        };

        let applied = append(&["b.example", "c.example", "c.example", "a.example"])
            .apply(&mut params, &caller);

        assert_eq!(applied.written, 1);
        assert_eq!(
            params.network_blacklist.as_deref().unwrap(),
            ["a.example", "b.example", "c.example"]
        );
    }

    #[test]
    fn a_whitelist_on_the_caller_blocks_a_blacklist_edit() {
        let caller = RequestParams {
            network_whitelist: Some(vec!["example.com".into()]),
            ..RequestParams::default()
        };
        let mut params = caller.clone();

        let applied = append(&["c.example"]).apply(&mut params, &caller);

        assert_eq!(applied.skipped_pinned, 1);
        assert_eq!(params, caller);
    }

    /// Every value of every learnable key, one edit per set.
    fn every_learnable_edit() -> Vec<EditSet> {
        let mut sets = Vec::new();
        for spec in Schema::v1().learnable() {
            let edits: Vec<Edit> = match spec.key {
                Key::Request => MODES
                    .iter()
                    .map(|m| Edit::set(spec.key, Value::Mode(*m)))
                    .collect(),
                Key::Proxy => PROXIES
                    .iter()
                    .map(|p| Edit::set(spec.key, Value::Proxy(*p)))
                    .collect(),
                Key::WaitIdleMillis => WAIT_BUCKETS
                    .iter()
                    .map(|n| Edit::set(spec.key, Value::Millis(*n)))
                    .collect(),
                Key::NetworkBlacklist => vec![
                    Edit {
                        key: spec.key,
                        op: Op::Append(vec!["c.example".into()]),
                    },
                    Edit {
                        key: spec.key,
                        op: Op::Remove(vec!["example.net".into()]),
                    },
                    Edit {
                        key: spec.key,
                        op: Op::Replace(vec!["c.example".into()]),
                    },
                    Edit {
                        key: spec.key,
                        op: Op::Clear,
                    },
                ],
                key => vec![
                    Edit::set(key, Value::Bool(true)),
                    Edit::set(key, Value::Bool(false)),
                ],
            };
            for edit in edits {
                sets.push(EditSet::new(vec![edit]).unwrap());
            }
        }
        sets
    }

    #[test]
    fn an_edit_never_touches_a_field_the_caller_set() {
        let caller = crate::schema::tests::populated();

        let sets = every_learnable_edit();
        assert!(sets.len() >= 20, "only {} edits were tried", sets.len());

        for set in sets {
            let mut params = caller.clone();
            let applied = set.apply(&mut params, &caller);
            assert_eq!(params, caller, "{set:?} wrote over the caller");
            assert_eq!(applied.written, 0, "{set:?}");
            assert_eq!(applied.skipped_pinned, 1, "{set:?}");
        }
    }

    #[test]
    fn an_edit_writes_where_the_caller_left_a_gap() {
        // The counterpart to the test above, so it cannot pass by writing
        // nothing anywhere.
        let caller = RequestParams::default();
        let mut params = RequestParams::default();
        let set = EditSet::new(vec![
            Edit::set(Key::Request, Value::Mode(RequestMode::Browser)),
            Edit::set(Key::WaitIdleMillis, Value::Millis(5_000)),
            Edit::set(Key::BlockAds, Value::Bool(false)),
        ])
        .unwrap();

        let applied = set.apply(&mut params, &caller);

        assert_eq!(applied.written, 3);
        assert_eq!(params.request, Some(RequestMode::Browser));
        assert_eq!(
            params.wait_for,
            Some(WaitFor::idle_network(Timeout::from_millis(5_000)))
        );
        assert_eq!(params.block_ads, Some(false));
    }

    #[test]
    fn an_edit_set_rejects_duplicates_and_too_many() {
        let mode = Edit::set(Key::Request, Value::Mode(RequestMode::Browser));
        assert_eq!(
            EditSet::new(vec![mode.clone(), mode.clone()]),
            Err(EditError::Duplicate(Key::Request))
        );
        assert_eq!(
            EditSet::new(vec![
                mode.clone(),
                Edit::set(Key::Proxy, Value::Proxy(ProxyPool::Residential)),
                Edit::set(Key::BlockAds, Value::Bool(false)),
                Edit::set(Key::FullResources, Value::Bool(true)),
            ]),
            Err(EditError::TooMany)
        );
        assert_eq!(
            EditSet::new(vec![Edit {
                key: Key::Blacklist,
                op: Op::Append(vec!["/a".into()]),
            }]),
            Err(EditError::Unsupported(Key::Blacklist))
        );

        // Order does not matter to what the set is.
        let a = EditSet::new(vec![
            Edit::set(Key::BlockAds, Value::Bool(false)),
            mode.clone(),
        ])
        .unwrap();
        let b = EditSet::new(vec![mode, Edit::set(Key::BlockAds, Value::Bool(false))]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.edits()[0].key, Key::Request);
        assert!(EditSet::new(Vec::new()).unwrap().is_keep());
    }

    #[test]
    fn a_zero_wait_writes_nothing() {
        let caller = RequestParams::default();
        let mut params = RequestParams::default();
        let set = EditSet::new(vec![Edit::set(Key::WaitIdleMillis, Value::Millis(0))]).unwrap();

        let applied = set.apply(&mut params, &caller);

        assert_eq!(applied, Applied::default());
        assert_eq!(params.wait_for, None);
    }
}
