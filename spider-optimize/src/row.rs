//! The comparison row: one arm of one paired trial, as a line of JSON.
//!
//! A pair is the same request sent twice, once as the baseline and once with
//! a candidate edit, or once more in shadow. Each arm writes one row. The rows
//! are what a scorer is trained on, so what a row may hold is the same as what
//! a feature may hold: numbers, buckets and labels from a fixed vocabulary.
//! There is no url, host, pattern or body in [`ComparisonRow`], and no field
//! that could carry one.
//!
//! Written by hand rather than with a serializer, so a new column is a
//! deliberate edit to [`comparison_row`] and shows up in review as one. A
//! float that JSON cannot hold is written as `0`, and an absent value as
//! `null`.

use crate::edit::EditSet;
use crate::features::{value_bucket, EditFeatures, EDIT_FEATURE_VERSION};
use crate::observe::Observation;
use crate::params::Params;
use crate::schema::{Key, SCHEMA_VERSION};
use spider_route::{
    DeclaredNeed, ExtClass, FeatureVector, RouteDecision, RouteSource, SiteMemory, StatusClass,
    FEATURES_USED,
};
use std::fmt;

/// The version of the row's columns.
pub const CMP_ROW_VERSION: u32 = 1;

/// The version of the router feature layout a row's `base` columns follow.
///
/// The router crate does not version its layout, so this crate names the one
/// it was written against: 152 used slots. The assertion below fails the build
/// when that count moves, which is the moment this number has to move too.
pub const BASE_FEATURE_VERSION: u16 = 1;

const _: () = assert!(FEATURES_USED == 152);

/// Which arm of a pair a row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arm {
    /// The request as it was.
    Baseline,
    /// The request with the candidate edit applied.
    Candidate,
    /// A copy scored but not acted on.
    Shadow,
}

/// How much the caller remembered about the site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryState {
    /// Nothing recorded.
    Cold,
    /// One or two attempts.
    Thin,
    /// Three to nine attempts.
    Warm,
    /// Ten or more.
    Steady,
}

impl MemoryState {
    /// The state a memory is in.
    pub const fn of(memory: Option<&SiteMemory>) -> MemoryState {
        match memory {
            None => MemoryState::Cold,
            Some(memory) => match memory.observations {
                0 => MemoryState::Cold,
                1..=2 => MemoryState::Thin,
                3..=9 => MemoryState::Warm,
                _ => MemoryState::Steady,
            },
        }
    }
}

/// An identifier, as buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdentDescriptor {
    /// The extension class slot. See [`crate::Identifier::class`].
    pub class: u8,
    /// See [`crate::Identifier::share_bucket`].
    pub share: u8,
    /// See [`crate::Identifier::count_bucket`].
    pub count: u8,
    /// See [`crate::Identifier::third_party`].
    pub third: bool,
}

/// An edit, as numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EditDescriptor {
    /// [`Key::index`].
    pub key: u8,
    /// [`crate::Op::code`].
    pub op: u8,
    /// The value's index within its kind, or 255 when the op has none.
    pub bucket: u8,
    /// The identifier an append names, when it names an observed one.
    pub ident: Option<IdentDescriptor>,
}

impl EditDescriptor {
    /// Describe an edit set, or `None` for keep.
    ///
    /// A row holds one descriptor. For a pair that is the blacklist append
    /// when there is one, since the identifier is what the rest of the row
    /// cannot recover, and otherwise the first edit in key order. The whole
    /// set is in the row's edit features either way.
    pub fn describe(edits: &EditSet, observation: &Observation) -> Option<EditDescriptor> {
        let edit = edits
            .get(Key::NetworkBlacklist)
            .or_else(|| edits.edits().first())?;
        let (bucket, ident) = match value_bucket(edit, observation) {
            Some((bucket, ident)) => (bucket.min(254) as u8, ident),
            None => (u8::MAX, None),
        };
        Some(EditDescriptor {
            key: edit.key.index() as u8,
            op: edit.op.code(),
            bucket,
            ident: ident.map(|ident| IdentDescriptor {
                class: crate::features::ext_index(ident.class) as u8,
                share: ident.share_bucket,
                count: ident.count_bucket,
                third: ident.third_party,
            }),
        })
    }
}

/// Which keys the caller set, as two bitmasks over [`Key::index`]: the first
/// 32 keys, then the rest.
///
/// The second mask is 64 bits because version one has 80 keys.
pub fn pinned_mask(caller: &dyn Params) -> (u32, u64) {
    let mut low = 0u32;
    let mut high = 0u64;
    for key in Key::ALL {
        if !caller.is_set(*key) {
            continue;
        }
        let at = key.index();
        if at < 32 {
            low |= 1 << at;
        } else if at < 96 {
            high |= 1 << (at - 32);
        }
    }
    (low, high)
}

/// One arm of one paired trial.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComparisonRow<'a> {
    /// Which pair this arm belongs to.
    pub pair: u64,
    /// Which arm.
    pub arm: Arm,
    /// The day the trial ran, as a count the caller chooses.
    pub day: u32,
    /// The caller's opaque key for the site, for splitting train and test by
    /// site. Never a hash this crate computes from a name.
    pub domain_key: u64,
    /// What the caller wanted back.
    pub need: DeclaredNeed,
    /// What kind of file the address names.
    pub ext: ExtClass,
    /// The label group slot of the host, as the router reads it.
    pub tld: u8,
    /// How much the caller remembered.
    pub memory: MemoryState,
    /// What the router decided.
    pub routed: &'a RouteDecision,
    /// The edit this arm carried, `None` for the baseline.
    pub edit: Option<EditDescriptor>,
    /// See [`pinned_mask`], the first 32 keys.
    pub pinned: u32,
    /// See [`pinned_mask`], keys 32 and up.
    pub pinned_hi: u64,
    /// Whether the arm got what the caller asked for.
    pub success: bool,
    /// How it ended.
    pub status: StatusClass,
    /// How long it took.
    pub millis: u32,
    /// How many bytes came back.
    pub bytes: u32,
    /// What it cost.
    pub credits: f64,
    /// How many attempts it took.
    pub attempts: u8,
    /// The arm's multiplier.
    pub multiplier: f32,
    /// How many fields the caller asked for.
    pub fields_requested: u8,
    /// How many came back non-empty.
    pub fields_present: u8,
    /// Whether the content matched the baseline closely enough.
    pub content_ok: Option<bool>,
    /// See [`crate::labels::fields_ok`].
    pub fields_ok: Option<bool>,
    /// See [`crate::labels::shingle_jaccard`].
    pub shingle_jaccard: Option<f32>,
    /// See [`crate::labels::byte_ratio`].
    pub byte_ratio: Option<f32>,
    /// The router's features for the request.
    pub base: &'a FeatureVector,
    /// The edit's features.
    pub edit_feats: &'a EditFeatures,
}

/// A string built with `write!`, without a trait import.
struct Out(String);

impl Out {
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) {
        // Writing into a `String` cannot fail.
        let _ = fmt::Write::write_fmt(&mut self.0, args);
    }

    fn raw(&mut self, text: &str) {
        self.0.push_str(text);
    }

    fn float(&mut self, value: f32) {
        if value.is_finite() {
            write!(self, "{value}");
        } else {
            self.raw("0");
        }
    }

    fn double(&mut self, value: f64) {
        if value.is_finite() {
            write!(self, "{value}");
        } else {
            self.raw("0");
        }
    }

    fn flag(&mut self, value: bool) {
        self.raw(if value { "true" } else { "false" });
    }

    fn label(&mut self, text: &str) {
        self.raw("\"");
        self.raw(text);
        self.raw("\"");
    }
}

/// One row, as a single line of JSON with no trailing newline.
pub fn comparison_row(r: &ComparisonRow<'_>) -> String {
    let base = r.base.as_slice();
    let base = base.get(..FEATURES_USED).unwrap_or(base);
    let mut out = Out(String::with_capacity(4_096));

    write!(
        out,
        "{{\"v\":{CMP_ROW_VERSION},\"schema_v\":{SCHEMA_VERSION},\"feat_v\":{BASE_FEATURE_VERSION},\"edit_feat_v\":{EDIT_FEATURE_VERSION},\"pair\":{},\"arm\":",
        r.pair
    );
    out.label(arm_label(r.arm));
    write!(out, ",\"day\":{},\"dk\":{},\"need\":", r.day, r.domain_key);
    out.label(need_label(r.need));
    out.raw(",\"ext\":");
    out.label(ext_label(r.ext));
    write!(out, ",\"tld\":{},\"mem\":", r.tld);
    out.label(memory_label(r.memory));

    out.raw(",\"routed\":{\"mode\":");
    out.label(r.routed.mode().as_str());
    out.raw(",\"proxy\":");
    out.label(r.routed.proxy().as_str());
    write!(
        out,
        ",\"wait_ms\":{},\"start_rung\":{},\"source\":",
        r.routed.wait().millis(),
        r.routed.start_rung
    );
    out.label(source_label(r.routed.source));
    out.raw(",\"confidence\":");
    out.float(r.routed.confidence);
    out.raw("}");

    out.raw(",\"edit\":");
    match r.edit {
        None => out.raw("null"),
        Some(edit) => {
            write!(
                out,
                "{{\"key\":{},\"op\":{},\"bucket\":{},\"ident\":",
                edit.key, edit.op, edit.bucket
            );
            match edit.ident {
                None => out.raw("null"),
                Some(ident) => {
                    write!(
                        out,
                        "{{\"class\":{},\"share\":{},\"count\":{},\"third\":",
                        ident.class, ident.share, ident.count
                    );
                    out.flag(ident.third);
                    out.raw("}");
                }
            }
            out.raw("}");
        }
    }

    write!(
        out,
        ",\"pinned\":{},\"pinned_hi\":{},\"success\":",
        r.pinned, r.pinned_hi
    );
    out.flag(r.success);
    out.raw(",\"status\":");
    out.label(status_label(r.status));
    write!(
        out,
        ",\"millis\":{},\"bytes\":{},\"credits\":",
        r.millis, r.bytes
    );
    out.double(r.credits);
    write!(out, ",\"attempts\":{},\"multiplier\":", r.attempts);
    out.float(r.multiplier);
    write!(
        out,
        ",\"fields_requested\":{},\"fields_present\":{},\"content_ok\":",
        r.fields_requested, r.fields_present
    );
    optional_flag(&mut out, r.content_ok);
    out.raw(",\"fields_ok\":");
    optional_flag(&mut out, r.fields_ok);
    out.raw(",\"shingle_jaccard\":");
    optional_float(&mut out, r.shingle_jaccard);
    out.raw(",\"byte_ratio\":");
    optional_float(&mut out, r.byte_ratio);

    out.raw(",\"base\":[");
    array(&mut out, base);
    out.raw("],\"edit_feats\":[");
    array(&mut out, r.edit_feats.as_slice());
    out.raw("]}");

    out.0
}

fn optional_flag(out: &mut Out, value: Option<bool>) {
    match value {
        Some(value) => out.flag(value),
        None => out.raw("null"),
    }
}

fn optional_float(out: &mut Out, value: Option<f32>) {
    match value {
        Some(value) => out.float(value),
        None => out.raw("null"),
    }
}

fn array(out: &mut Out, values: &[f32]) {
    for (at, value) in values.iter().enumerate() {
        if at > 0 {
            out.raw(",");
        }
        out.float(*value);
    }
}

/// The name an arm is written under.
pub(crate) const fn arm_label(arm: Arm) -> &'static str {
    match arm {
        Arm::Baseline => "baseline",
        Arm::Candidate => "candidate",
        Arm::Shadow => "shadow",
    }
}

/// The name a memory state is written under.
pub(crate) const fn memory_label(memory: MemoryState) -> &'static str {
    match memory {
        MemoryState::Cold => "cold",
        MemoryState::Thin => "thin",
        MemoryState::Warm => "warm",
        MemoryState::Steady => "steady",
    }
}

/// Every need, in the router's slot order.
#[cfg(test)]
pub(crate) const ALL_NEEDS: [DeclaredNeed; 8] = [
    DeclaredNeed::Text,
    DeclaredNeed::Markdown,
    DeclaredNeed::Html,
    DeclaredNeed::Links,
    DeclaredNeed::Metadata,
    DeclaredNeed::Fields,
    DeclaredNeed::Screenshot,
    DeclaredNeed::Raw,
];

/// The name a need is written under.
pub(crate) const fn need_label(need: DeclaredNeed) -> &'static str {
    match need {
        DeclaredNeed::Text => "text",
        DeclaredNeed::Markdown => "markdown",
        DeclaredNeed::Html => "html",
        DeclaredNeed::Links => "links",
        DeclaredNeed::Metadata => "metadata",
        DeclaredNeed::Fields => "fields",
        DeclaredNeed::Screenshot => "screenshot",
        DeclaredNeed::Raw => "raw",
        _ => "other",
    }
}

/// Every status class, in the router's slot order.
#[cfg(test)]
pub(crate) const ALL_STATUSES: [StatusClass; 9] = [
    StatusClass::Unknown,
    StatusClass::Ok,
    StatusClass::Empty,
    StatusClass::BadRequest,
    StatusClass::NeedsLogin,
    StatusClass::Blocked,
    StatusClass::NotFound,
    StatusClass::RateLimited,
    StatusClass::ServerError,
];

/// The name a status class is written under, as the client's route rows
/// write it.
pub(crate) const fn status_label(status: StatusClass) -> &'static str {
    match status {
        StatusClass::Unknown => "unknown",
        StatusClass::Ok => "ok",
        StatusClass::Empty => "empty",
        StatusClass::BadRequest => "bad_request",
        StatusClass::NeedsLogin => "needs_login",
        StatusClass::Blocked => "blocked",
        StatusClass::NotFound => "not_found",
        StatusClass::RateLimited => "rate_limited",
        StatusClass::ServerError => "server_error",
        _ => "other",
    }
}

/// The name an extension class is written under.
pub(crate) const fn ext_label(ext: ExtClass) -> &'static str {
    match ext {
        ExtClass::None => "none",
        ExtClass::Markup => "markup",
        ExtClass::Xml => "xml",
        ExtClass::Json => "json",
        ExtClass::Feed => "feed",
        ExtClass::Text => "text",
        ExtClass::Csv => "csv",
        ExtClass::Pdf => "pdf",
        ExtClass::Office => "office",
        ExtClass::Image => "image",
        ExtClass::Media => "media",
        ExtClass::Archive => "archive",
        ExtClass::Asset => "asset",
        _ => "other",
    }
}

/// The name a decision's source is written under, as the client's route rows
/// write it.
pub(crate) const fn source_label(source: RouteSource) -> &'static str {
    match source {
        RouteSource::Heuristic => "heuristic",
        RouteSource::Model => "model",
        RouteSource::Caller => "caller",
        RouteSource::Memory => "memory",
        RouteSource::Explore => "explore",
        _ => "other",
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
    use crate::candidates::{generate, Candidate};
    use crate::features::featurize_edit;
    use crate::schema::Schema;
    use crate::testing::Fixture;
    use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
    use spider_route::{featurize, RouteInput};

    /// The top level keys of a JSON object, in the order they were written.
    struct KeyOrder(Vec<String>);

    impl<'de> serde::Deserialize<'de> for KeyOrder {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<KeyOrder, D::Error> {
            struct Keys;
            impl<'de> Visitor<'de> for Keys {
                type Value = KeyOrder;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an object")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<KeyOrder, A::Error> {
                    let mut keys = Vec::new();
                    while let Some(key) = map.next_key::<String>()? {
                        let _: IgnoredAny = map.next_value()?;
                        keys.push(key);
                    }
                    Ok(KeyOrder(keys))
                }
            }
            deserializer.deserialize_map(Keys)
        }
    }

    /// A row for the observed fixture's first append, with every option set.
    fn sample(fixture: &Fixture) -> String {
        let ctx = fixture.ctx();
        let cand = generate(&Schema::v1(), &ctx)
            .into_iter()
            .find(|c| c.edits.get(Key::NetworkBlacklist).is_some())
            .unwrap_or_else(|| Candidate::keep(&ctx));
        let feats = featurize_edit(&ctx, &cand);
        let base = featurize(&RouteInput::new(ctx.url, ctx.need));
        let (pinned, pinned_hi) = pinned_mask(ctx.caller);
        comparison_row(&ComparisonRow {
            pair: u64::MAX,
            arm: Arm::Candidate,
            day: 20_000,
            domain_key: 42,
            need: ctx.need,
            ext: ExtClass::Markup,
            tld: 0,
            memory: MemoryState::of(ctx.observation.memory.as_ref()),
            routed: ctx.routed,
            edit: EditDescriptor::describe(&cand.edits, ctx.observation),
            pinned,
            pinned_hi,
            success: true,
            status: StatusClass::Ok,
            millis: 1_234,
            bytes: 56_789,
            credits: f64::NAN,
            attempts: 1,
            multiplier: cand.multiplier,
            fields_requested: 2,
            fields_present: 2,
            content_ok: Some(true),
            fields_ok: None,
            shingle_jaccard: Some(0.97),
            byte_ratio: Some(f32::INFINITY),
            base: &base,
            edit_feats: &feats,
        })
    }

    #[test]
    fn rows_hold_no_url_host_or_body() {
        let row = sample(&Fixture::observed());

        assert!(row.contains("\"edit\":{\"key\":51,\"op\":1"), "{row}");
        for needle in [
            "http",
            "example",
            "articles",
            "tracker-alpha",
            "cdn-beta",
            ".com",
            "\n",
        ] {
            assert!(!row.contains(needle), "{needle:?} reached the row: {row}");
        }
    }

    #[test]
    fn a_row_parses_as_json_with_the_documented_keys() {
        let mut fixture = Fixture::observed();
        fixture.caller.stealth = Some(true);
        fixture.caller.skip_config_checks = Some(true);
        let row = sample(&fixture);

        let order: KeyOrder = serde_json::from_str(&row).unwrap();
        assert_eq!(
            order.0,
            [
                "v",
                "schema_v",
                "feat_v",
                "edit_feat_v",
                "pair",
                "arm",
                "day",
                "dk",
                "need",
                "ext",
                "tld",
                "mem",
                "routed",
                "edit",
                "pinned",
                "pinned_hi",
                "success",
                "status",
                "millis",
                "bytes",
                "credits",
                "attempts",
                "multiplier",
                "fields_requested",
                "fields_present",
                "content_ok",
                "fields_ok",
                "shingle_jaccard",
                "byte_ratio",
                "base",
                "edit_feats",
            ]
        );

        let value: serde_json::Value = serde_json::from_str(&row).unwrap();
        // A parsed value sorts its keys, so the nested order is read off the
        // text. The routed object holds no nested object.
        let from = row.find("\"routed\":").unwrap() + "\"routed\":".len();
        let to = from + row[from..].find('}').unwrap() + 1;
        let routed: KeyOrder = serde_json::from_str(&row[from..to]).unwrap();
        assert_eq!(
            routed.0,
            [
                "mode",
                "proxy",
                "wait_ms",
                "start_rung",
                "source",
                "confidence"
            ]
        );
        assert_eq!(value["pair"].as_u64(), Some(u64::MAX));
        assert_eq!(value["base"].as_array().unwrap().len(), FEATURES_USED);
        assert_eq!(value["edit_feats"].as_array().unwrap().len(), 96);
        assert_eq!(value["credits"], 0, "a NaN is written as zero");
        assert_eq!(value["byte_ratio"], 0, "an infinity is written as zero");
        assert!(value["fields_ok"].is_null());
        assert_eq!(value["pinned"], 1 << Key::Stealth.index());
        assert_eq!(
            value["pinned_hi"].as_u64(),
            Some(1 << (Key::SkipConfigChecks.index() - 32))
        );
        assert_eq!(value["edit"]["ident"]["third"], true);
        assert_eq!(value["status"], "ok");
    }
}
