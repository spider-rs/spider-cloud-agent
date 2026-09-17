//! Turning two arms into a pair: the nonce, the labels, and the rows.
//!
//! The client writes each arm's row with its pair-relative labels empty,
//! because one arm cannot know them. Here both bodies are still in memory, so
//! the candidate row gets `shingle_jaccard`, `byte_ratio`, `fields_ok` and
//! `content_ok` computed from them, the same way
//! `training/src/spider_optimize_train/labels.py` recomputes `content_ok` from
//! the stored scalars. No body, no url and no host reaches a row.

use serde_json::{Number, Value};
use spider_cloud_agent::{DeclaredNeed, Page};

/// The content tau a collected row is labelled with, the trainer's default.
pub const TAU: f64 = 0.80;

/// What one arm brought back, kept only until its pair is labelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// The text of a page, or its links one per line.
    Text(String),
    /// Extracted fields, each flattened to one string.
    Fields(Vec<(String, String)>),
}

impl Content {
    /// Read the part of a served page this need compares.
    pub fn of(page: &Page, need: DeclaredNeed) -> Content {
        match need {
            DeclaredNeed::Fields => Content::Fields(
                page.body
                    .fields()
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|(name, value)| (name.clone(), flatten(value)))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            DeclaredNeed::Links => Content::Text(
                page.links
                    .as_ref()
                    .map(|links| {
                        links
                            .iter()
                            .map(url::Url::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
            ),
            _ => Content::Text(page.text().unwrap_or_default().to_string()),
        }
    }

    /// The whole content as one text, for the shingle and byte comparisons.
    fn text(&self) -> String {
        match self {
            Content::Text(text) => text.clone(),
            Content::Fields(fields) => fields
                .iter()
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    fn fields(&self) -> Vec<(&str, &str)> {
        match self {
            Content::Text(_) => Vec::new(),
            Content::Fields(fields) => fields
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect(),
        }
    }
}

/// An extracted value as text: strings as they are, lists joined with spaces.
fn flatten(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(flatten)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}

/// Pair nonces, from a seed.
///
/// SplitMix64 is a bijection of its counter, so no two draws of one run repeat
/// before the mask. The mask keeps a nonce exact in a reader that holds JSON
/// numbers as doubles.
pub struct Nonces(u64);

impl Nonces {
    pub fn new(seed: u64) -> Nonces {
        Nonces(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A pair nonce, never zero, below 2^53.
    pub fn pair(&mut self) -> u64 {
        (self.next_u64() & ((1 << 53) - 1)).max(1)
    }

    /// A draw in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A float as a row writes it, rounded to five places.
fn rounded(value: f32) -> Value {
    Number::from_f64((f64::from(value) * 1e5).round() / 1e5).map_or(Value::Null, Value::Number)
}

/// Label the candidate row against its baseline and stamp both with the pair.
///
/// `requested` is the field names the need asked for, empty unless the need is
/// fields. `repeat` marks a second baseline run, which is written with arm
/// `candidate` and no edit so a pair keeps exactly one baseline arm, the shape
/// `labels.repeat_jaccards` reads.
pub fn label(
    pair: u64,
    baseline: &mut Value,
    candidate: &mut Value,
    base: Option<&Content>,
    cand: Option<&Content>,
    requested: &[String],
    repeat: bool,
) {
    for row in [&mut *baseline, &mut *candidate] {
        row["pair"] = Value::from(pair);
    }
    if repeat {
        candidate["arm"] = Value::from("candidate");
        candidate["edit"] = Value::Null;
    }

    let need = candidate["need"].as_str().unwrap_or_default().to_string();
    let (jaccard, ratio) = match (base, cand) {
        (Some(base), Some(cand)) => {
            let (a, b) = (base.text(), cand.text());
            (
                Some(spider_optimize::shingle_jaccard(&a, &b)),
                Some(spider_optimize::labels::byte_ratio(a.len(), b.len())),
            )
        }
        _ => (None, None),
    };
    let fields_ok = (need == "fields").then(|| match cand {
        Some(cand) => {
            let names: Vec<&str> = requested.iter().map(String::as_str).collect();
            let base_fields = base.map(Content::fields).unwrap_or_default();
            spider_optimize::fields_ok(&names, &base_fields, &cand.fields())
        }
        None => false,
    });

    candidate["shingle_jaccard"] = jaccard.map_or(Value::Null, rounded);
    candidate["byte_ratio"] = ratio.map_or(Value::Null, rounded);
    candidate["fields_ok"] = fields_ok.map_or(Value::Null, Value::from);
    candidate["content_ok"] = content_ok(candidate, TAU).map_or(Value::Null, Value::from);
}

/// Whether an arm's content matched its baseline's, or `None` when that cannot
/// be judged. The rule of `labels.content_ok` in the training package.
pub fn content_ok(row: &Value, tau: f64) -> Option<bool> {
    let need = row["need"].as_str().unwrap_or_default();
    if matches!(need, "screenshot" | "raw") {
        return None;
    }
    if row["status"].as_str() != Some("ok") {
        return Some(false);
    }
    match need {
        "fields" => row["fields_ok"].as_bool(),
        "links" | "metadata" => {
            let requested = row["fields_requested"].as_u64().unwrap_or(0);
            if requested == 0 {
                return None;
            }
            let present = row["fields_present"].as_u64().unwrap_or(0);
            Some(present as f64 / requested as f64 >= 0.9)
        }
        _ => {
            let jaccard = row["shingle_jaccard"].as_f64()?;
            let ratio = row["byte_ratio"].as_f64()?;
            Some(jaccard >= tau && (0.5..=2.0).contains(&ratio))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;

    fn row(need: &str, arm: &str) -> Value {
        json!({
            "pair": 0, "arm": arm, "need": need, "status": "ok", "edit": {"key": 1},
            "fields_requested": 0, "fields_present": 0, "content_ok": null,
            "fields_ok": null, "shingle_jaccard": null, "byte_ratio": null
        })
    }

    #[test]
    fn a_matching_body_is_content_ok_and_a_broken_one_is_not() {
        let text = "the harbour rebuilt the pier after the winter storms took it";
        let base = Content::Text(text.to_string());
        let (mut b, mut c) = (row("markdown", "baseline"), row("markdown", "candidate"));
        label(7, &mut b, &mut c, Some(&base), Some(&base), &[], false);
        assert_eq!(b["pair"], 7);
        assert_eq!(c["pair"], 7);
        assert_eq!(c["shingle_jaccard"], 1.0);
        assert_eq!(c["byte_ratio"], 1.0);
        assert_eq!(c["content_ok"], true);
        assert!(
            b["shingle_jaccard"].is_null(),
            "the baseline row is not labelled"
        );

        let broken = Content::Text("access denied".to_string());
        let (mut b, mut c) = (row("markdown", "baseline"), row("markdown", "candidate"));
        label(8, &mut b, &mut c, Some(&base), Some(&broken), &[], false);
        assert_eq!(c["content_ok"], false);

        // A candidate that came back with nothing cannot be compared, and its
        // status already says it failed.
        let (mut b, mut c) = (row("markdown", "baseline"), row("markdown", "candidate"));
        c["status"] = json!("blocked");
        label(9, &mut b, &mut c, Some(&base), None, &[], false);
        assert!(c["shingle_jaccard"].is_null());
        assert_eq!(c["content_ok"], false);
    }

    #[test]
    fn fields_are_judged_by_fields_ok() {
        let requested = vec!["title".to_string(), "price".to_string()];
        let base = Content::Fields(vec![
            ("title".into(), "Blue Kettle".into()),
            ("price".into(), "20 USD".into()),
        ]);
        let dropped = Content::Fields(vec![("title".into(), "Blue Kettle".into())]);
        let (mut b, mut c) = (row("fields", "baseline"), row("fields", "candidate"));
        label(
            1,
            &mut b,
            &mut c,
            Some(&base),
            Some(&base),
            &requested,
            false,
        );
        assert_eq!(c["fields_ok"], true);
        assert_eq!(c["content_ok"], true);
        let (mut b, mut c) = (row("fields", "baseline"), row("fields", "candidate"));
        label(
            2,
            &mut b,
            &mut c,
            Some(&base),
            Some(&dropped),
            &requested,
            false,
        );
        assert_eq!(c["fields_ok"], false);
        assert_eq!(c["content_ok"], false);
    }

    #[test]
    fn a_repeat_keeps_one_baseline_arm() {
        let base = Content::Text("same page".to_string());
        let (mut b, mut c) = (row("text", "baseline"), row("text", "baseline"));
        label(3, &mut b, &mut c, Some(&base), Some(&base), &[], true);
        assert_eq!(b["arm"], "baseline");
        assert_eq!(c["arm"], "candidate");
        assert!(c["edit"].is_null());
        assert_eq!(c["shingle_jaccard"], 1.0);
    }

    #[test]
    fn nonces_do_not_repeat_and_units_stay_in_range() {
        let mut nonces = Nonces::new(42);
        let mut seen: Vec<u64> = (0..10_000).map(|_| nonces.pair()).collect();
        assert!(seen.iter().all(|n| *n > 0 && *n < 1 << 53));
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 10_000);
        let mut draws = Nonces::new(1);
        assert!((0..1_000)
            .map(|_| draws.unit())
            .all(|u| (0.0..1.0).contains(&u)));
    }
}
