//! The candidate file: which edit sets to try against every url.
//!
//! A JSON array of entries. Each entry is an array of one edit, or of two that
//! form one of the pairs the optimizer lists, and each edit is an object naming
//! the field by its wire name, the operation and the value:
//!
//! ```json
//! [
//!   [{"key": "request", "op": "set", "value": "browser"}],
//!   [{"key": "block_stylesheets", "op": "set", "value": false}],
//!   [{"key": "request", "op": "set", "value": "browser"},
//!    {"key": "wait_for", "op": "set", "value": 5000}],
//!   [{"key": "network_blacklist", "op": "append", "value": {"observed": 0}}]
//! ]
//! ```
//!
//! `request` takes a mode name, `proxy` a pool name, `wait_for` an idle network
//! wait in milliseconds from the schema's buckets, and the five switches a
//! boolean. `network_blacklist` takes `append` only, with either a list of
//! patterns or `{"observed": N}`, which names the rank `N` identifier the
//! baseline arm of the same url observed. Every entry is checked here, before
//! any request goes out.

use serde::Deserialize;
use spider_cloud_agent::{ProxyPool, RequestMode};
use spider_optimize::schema::{Kind, MODES, PROXIES};
use spider_optimize::{Edit, EditSet, Key, Observation, Op, Schema, Value, MAX_IDENTIFIERS};

/// One edit as the file writes it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEdit {
    key: String,
    op: String,
    value: serde_json::Value,
}

/// What a blacklist append adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Patterns {
    /// These patterns, which must also be among the observed identifiers.
    Literal(Vec<String>),
    /// The identifier at this rank in the baseline's resource summary.
    Observed(usize),
}

/// One edit, before the observation is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecEdit {
    /// A single value, ready as it is.
    Fixed(Edit),
    /// A blacklist append resolved per url.
    Append(Patterns),
}

/// One entry of the candidate file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Where it sits in the file, from zero.
    pub index: usize,
    /// Its edits, in file order.
    pub edits: Vec<SpecEdit>,
}

impl Candidate {
    /// Whether any edit is a blacklist append, which needs hints switched off.
    pub fn blacklists(&self) -> bool {
        self.edits
            .iter()
            .any(|edit| matches!(edit, SpecEdit::Append(_)))
    }

    /// The edit set for one url, or why this entry cannot run there.
    ///
    /// Only an identifier the baseline observed can be appended, because the
    /// optimizer only lists appends of observed identifiers.
    pub fn resolve(&self, observation: &Observation) -> Result<EditSet, String> {
        let mut edits = Vec::with_capacity(self.edits.len());
        for edit in &self.edits {
            match edit {
                SpecEdit::Fixed(edit) => edits.push(edit.clone()),
                SpecEdit::Append(Patterns::Observed(rank)) => {
                    let pattern = observation
                        .resources
                        .as_ref()
                        .and_then(|resources| resources.candidates.get(*rank))
                        .ok_or_else(|| format!("no observed identifier at rank {rank}"))?;
                    edits.push(Edit {
                        key: Key::NetworkBlacklist,
                        op: Op::Append(vec![pattern.pattern.clone()]),
                    });
                }
                SpecEdit::Append(Patterns::Literal(patterns)) => {
                    if patterns
                        .iter()
                        .any(|pattern| observation.identifier(pattern).is_none())
                    {
                        return Err("a listed pattern was not observed".to_string());
                    }
                    edits.push(Edit {
                        key: Key::NetworkBlacklist,
                        op: Op::Append(patterns.clone()),
                    });
                }
            }
        }
        EditSet::new(edits).map_err(|error| error.to_string())
    }
}

/// Read and check the whole file. The error names the offending entry.
pub fn parse(text: &str) -> Result<Vec<Candidate>, String> {
    let raw: Vec<Vec<RawEdit>> =
        serde_json::from_str(text).map_err(|error| format!("not a list of edit lists: {error}"))?;
    if raw.is_empty() {
        return Err("the candidate file lists no entries".to_string());
    }
    let schema = Schema::v1();
    let mut out: Vec<Candidate> = Vec::with_capacity(raw.len());
    for (index, entry) in raw.into_iter().enumerate() {
        let candidate =
            entry_of(&schema, index, entry).map_err(|why| format!("entry {index}: {why}"))?;
        if let Some(seen) = out.iter().find(|seen| seen.edits == candidate.edits) {
            return Err(format!("entry {index}: repeats entry {}", seen.index));
        }
        out.push(candidate);
    }
    Ok(out)
}

fn entry_of(schema: &Schema, index: usize, entry: Vec<RawEdit>) -> Result<Candidate, String> {
    if entry.is_empty() {
        return Err("an empty edit set is the baseline, which always runs".to_string());
    }
    let mut edits = Vec::with_capacity(entry.len());
    for raw in entry {
        edits.push(edit_of(schema, raw)?);
    }

    // The same checks the optimizer's own builder makes, with a stand-in for
    // any pattern that is only known per url.
    let probe: Vec<Edit> = edits
        .iter()
        .map(|edit| match edit {
            SpecEdit::Fixed(edit) => edit.clone(),
            SpecEdit::Append(Patterns::Literal(patterns)) => Edit {
                key: Key::NetworkBlacklist,
                op: Op::Append(patterns.clone()),
            },
            SpecEdit::Append(Patterns::Observed(_)) => Edit {
                key: Key::NetworkBlacklist,
                op: Op::Append(vec!["observed".to_string()]),
            },
        })
        .collect();
    let set = EditSet::new(probe).map_err(|error| error.to_string())?;
    if let [first, second] = set.edits() {
        let paired = spider_optimize::candidates::PAIRS.iter().any(|(a, b)| {
            (first.key, second.key) == (*a, *b) || (first.key, second.key) == (*b, *a)
        });
        if !paired {
            return Err(format!(
                "{} and {} are not a pair the optimizer lists",
                first.key.wire(),
                second.key.wire()
            ));
        }
    } else if set.edits().len() > 2 {
        return Err("the optimizer lists at most two edits together".to_string());
    }
    Ok(Candidate { index, edits })
}

fn edit_of(schema: &Schema, raw: RawEdit) -> Result<SpecEdit, String> {
    let key = Key::ALL
        .iter()
        .copied()
        .find(|key| key.wire() == raw.key)
        .ok_or_else(|| format!("{:?} is not a request field", raw.key))?;
    let spec = schema.spec(key);
    if !spec.learnable {
        return Err(format!("{} is not a learnable field", key.wire()));
    }
    let wrong = || format!("{} does not take {} {}", key.wire(), raw.op, raw.value);

    match (spec.kind, raw.op.as_str()) {
        (Kind::StringList, "append") => {
            if let Some(rank) = raw
                .value
                .as_object()
                .filter(|object| object.len() == 1)
                .and_then(|object| object.get("observed"))
            {
                let rank = rank
                    .as_u64()
                    .filter(|rank| *rank < MAX_IDENTIFIERS as u64)
                    .ok_or_else(wrong)?;
                return Ok(SpecEdit::Append(Patterns::Observed(rank as usize)));
            }
            let patterns: Vec<String> =
                serde_json::from_value(raw.value.clone()).map_err(|_| wrong())?;
            let valid = |pattern: &String| {
                !pattern.is_empty()
                    && pattern.len() <= 253
                    && !pattern
                        .chars()
                        .any(|ch| ch.is_whitespace() || ch.is_control())
            };
            if patterns.is_empty()
                || patterns.len() > MAX_IDENTIFIERS
                || !patterns.iter().all(valid)
            {
                return Err(wrong());
            }
            Ok(SpecEdit::Append(Patterns::Literal(patterns)))
        }
        (_, "set") => {
            let value = match (key, &raw.value) {
                (Key::Request, serde_json::Value::String(name)) => RequestMode::from_wire(name)
                    .filter(|mode| MODES.contains(mode))
                    .map(Value::Mode),
                (Key::Proxy, serde_json::Value::String(name)) => PROXIES
                    .iter()
                    .copied()
                    .find(|pool: &ProxyPool| pool.as_str() == name)
                    .map(Value::Proxy),
                (_, serde_json::Value::Number(number)) => match spec.kind {
                    Kind::MillisBucket(buckets) => number
                        .as_u64()
                        .and_then(|millis| u32::try_from(millis).ok())
                        // A zero wait writes nothing, so it would run the baseline again.
                        .filter(|millis| *millis > 0 && buckets.contains(millis))
                        .map(Value::Millis),
                    _ => None,
                },
                (_, serde_json::Value::Bool(flag)) if spec.kind == Kind::Bool => {
                    Some(Value::Bool(*flag))
                }
                _ => None,
            };
            value
                .map(|value| SpecEdit::Fixed(Edit::set(key, value)))
                .ok_or_else(wrong)
        }
        _ => Err(wrong()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn the_documented_example_parses() {
        let text = r#"[
          [{"key": "request", "op": "set", "value": "browser"}],
          [{"key": "block_stylesheets", "op": "set", "value": false}],
          [{"key": "request", "op": "set", "value": "browser"},
           {"key": "wait_for", "op": "set", "value": 5000}],
          [{"key": "network_blacklist", "op": "append", "value": {"observed": 0}}],
          [{"key": "network_blacklist", "op": "append", "value": ["cdn.example"]}]
        ]"#;
        let parsed = parse(text).unwrap();
        assert_eq!(parsed.len(), 5);
        assert!(parsed[3].blacklists() && !parsed[0].blacklists());
    }

    #[test]
    fn a_bad_entry_is_named() {
        for (text, needle) in [
            (
                r#"[[{"key": "stealth", "op": "set", "value": true}]]"#,
                "entry 0",
            ),
            (
                r#"[[{"key": "request", "op": "set", "value": "http"}],
                   [{"key": "request", "op": "set", "value": "rocket"}]]"#,
                "entry 1",
            ),
            (
                r#"[[{"key": "wait_for", "op": "set", "value": 1234}]]"#,
                "entry 0",
            ),
            (
                r#"[[{"key": "wait_for", "op": "set", "value": 0}]]"#,
                "entry 0",
            ),
            (r#"[[]]"#, "entry 0"),
            (
                r#"[[{"key": "request", "op": "set", "value": "http"},
                    {"key": "request", "op": "set", "value": "browser"}]]"#,
                "entry 0",
            ),
            (
                r#"[[{"key": "block_ads", "op": "set", "value": true},
                    {"key": "request", "op": "set", "value": "browser"}]]"#,
                "entry 0",
            ),
            (
                r#"[[{"key": "block_ads", "op": "set", "value": true}],
                   [{"key": "block_ads", "op": "set", "value": true}]]"#,
                "entry 1",
            ),
            (
                r#"[[{"key": "proxy", "op": "append", "value": ["x"]}]]"#,
                "entry 0",
            ),
            (
                r#"[[{"key": "network_blacklist", "op": "append", "value": {"observed": 9}}]]"#,
                "entry 0",
            ),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.starts_with(needle), "{text}: {error}");
        }
        assert!(parse("[]").is_err());
        assert!(parse("{}").is_err());
    }
}
