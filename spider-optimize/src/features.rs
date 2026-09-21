//! What a scorer is allowed to look at about an edit.
//!
//! [`featurize_edit`] turns a candidate in its context into
//! [`EditFeatures`], a fixed array of [`EDIT_DIM`] floats that sits beside the
//! router's own [`FeatureVector`] in an [`Input`]. Every block is named by a
//! constant here, every value is a one-hot or a bucket, and every slot is
//! finite and within minus one to one.
//!
//! The same rule holds as in the router: no slot is a function of which site
//! is being fetched or which third party it loads. An identifier contributes
//! its class, its byte share bucket and its party, and never its pattern.
//! Missing information is a set slot in the `MISSING` block, never a gap.

use crate::candidates::{
    current_mode, current_proxy, current_wait, effective_flag, Candidate, Context, PAIRS,
};
use crate::edit::{Edit, Op, Value};
use crate::observe::{Identifier, Observation};
use crate::schema::{learnable_slot, Key, MODES, PROXIES, WAIT_BUCKETS};
use spider_route::{DeclaredNeed, ExtClass, FeatureVector, RouteSource};

/// The version of the edit feature layout. Recorded on every row.
pub const EDIT_FEATURE_VERSION: u16 = 1;

/// How many slots an edit feature vector has.
pub const EDIT_DIM: usize = 96;

/// The edit feature vector.
///
/// Fixed width, `Copy`, no heap, so there is no field a host could be stored
/// in.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EditFeatures {
    bits: [f32; EDIT_DIM],
}

impl Default for EditFeatures {
    fn default() -> EditFeatures {
        EditFeatures::zeroed()
    }
}

impl EditFeatures {
    /// Every slot zero.
    pub const fn zeroed() -> EditFeatures {
        EditFeatures {
            bits: [0.0; EDIT_DIM],
        }
    }

    /// The slots, in order.
    pub fn as_slice(&self) -> &[f32] {
        &self.bits
    }

    /// One slot, or `None` past the end.
    pub fn get(&self, slot: usize) -> Option<f32> {
        self.bits.get(slot).copied()
    }

    /// Set one slot. Out of range does nothing, because a layout bug should
    /// not take down a caller's process.
    fn set(&mut self, slot: usize, value: f32) {
        if let Some(cell) = self.bits.get_mut(slot) {
            *cell = value;
        }
    }

    /// Set the slot at `base + offset`.
    fn one_hot(&mut self, base: usize, offset: usize) {
        self.set(base + offset, 1.0);
    }
}

/// What a scorer reads: the router's features for the request and the edit's
/// own.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct Input {
    /// The router's features for the request, with no caller pins.
    pub base: FeatureVector,
    /// This candidate's features.
    pub edit: EditFeatures,
}

// The layout. Each base is derived from the block before it, so a block
// cannot silently overlap its neighbour.

/// The current value of each learnable key: request 3, proxy 2, idle wait
/// bucket 4, then two slots (off, on) for each of disable intercept, full
/// resources, block ads, block analytics and block stylesheets, then one for a
/// non-empty blacklist, then 4 spare.
pub const CONFIG: usize = 0;
pub(crate) const CONFIG_W: usize = 24;

/// Which learnable keys the edit set touches, in key order. Multi-hot for a
/// pair. 9 used, 7 spare.
pub const EDIT_KEY: usize = CONFIG + CONFIG_W;
pub(crate) const EDIT_KEY_W: usize = 16;

/// Which operations the edit set uses: set, append, remove, replace, clear.
/// Keep sets none.
pub const EDIT_OP: usize = EDIT_KEY + EDIT_KEY_W;
pub(crate) const EDIT_OP_W: usize = 5;

/// The index of each new value within its kind: the mode or pool index, the
/// wait bucket index, off 0 and on 1, or an appended identifier's rank.
pub const VALUE_BUCKET: usize = EDIT_OP + EDIT_OP_W;
pub(crate) const VALUE_BUCKET_W: usize = 8;

/// Which compatibility pair the edit set is, in [`PAIRS`] order, then a slot
/// for not a pair. 5 used, 3 spare.
pub const PAIR: usize = VALUE_BUCKET + VALUE_BUCKET_W;
pub(crate) const PAIR_W: usize = 8;

/// What the caller wants back: a body, fields, links or metadata, a picture.
pub const NEED_BITS: usize = PAIR + PAIR_W;
pub(crate) const NEED_BITS_W: usize = 4;

/// Third party hosts seen: none, 1 to 2, 3 to 5, 6 to 10, 11 or more.
pub const OBS_HOSTS: usize = NEED_BITS + NEED_BITS_W;
pub(crate) const OBS_HOSTS_W: usize = 5;

/// The appended identifier's byte share bucket. See
/// [`crate::Identifier::share_bucket`].
pub const OBS_SHARE: usize = OBS_HOSTS + OBS_HOSTS_W;
pub(crate) const OBS_SHARE_W: usize = 5;

/// The appended identifier's extension class, one-hot.
pub const OBS_CLASS: usize = OBS_SHARE + OBS_SHARE_W;
pub(crate) const OBS_CLASS_W: usize = 14;

/// The appended identifier's party: first, third.
pub const OBS_PARTY: usize = OBS_CLASS + OBS_CLASS_W;
pub(crate) const OBS_PARTY_W: usize = 2;

/// What is not known: no resource observation, no identifier on this
/// candidate, cold memory, no routed decision on the request.
pub const MISSING: usize = OBS_PARTY + OBS_PARTY_W;
pub(crate) const MISSING_W: usize = 4;

/// Always one.
pub const BIAS: usize = MISSING + MISSING_W;
pub(crate) const BIAS_W: usize = 1;

const _: () = assert!(BIAS + BIAS_W == EDIT_DIM);

/// Every block, as (name, base, width), in layout order.
pub const BLOCKS: [(&str, usize, usize); 12] = [
    ("config", CONFIG, CONFIG_W),
    ("edit_key", EDIT_KEY, EDIT_KEY_W),
    ("edit_op", EDIT_OP, EDIT_OP_W),
    ("value_bucket", VALUE_BUCKET, VALUE_BUCKET_W),
    ("pair", PAIR, PAIR_W),
    ("need_bits", NEED_BITS, NEED_BITS_W),
    ("obs_hosts", OBS_HOSTS, OBS_HOSTS_W),
    ("obs_share", OBS_SHARE, OBS_SHARE_W),
    ("obs_class", OBS_CLASS, OBS_CLASS_W),
    ("obs_party", OBS_PARTY, OBS_PARTY_W),
    ("missing", MISSING, MISSING_W),
    ("bias", BIAS, BIAS_W),
];

/// The switches in the config block, in key order.
const SWITCHES: [Key; 5] = [
    Key::DisableIntercept,
    Key::FullResources,
    Key::BlockAds,
    Key::BlockAnalytics,
    Key::BlockStylesheets,
];

/// Turn a candidate in its context into the edit features.
///
/// Allocates nothing, reads no clock, and gives the same answer for the same
/// input.
pub fn featurize_edit(ctx: &Context<'_>, cand: &Candidate) -> EditFeatures {
    let mut out = EditFeatures::zeroed();
    let current = ctx.current;

    // What the request carries now.
    out.one_hot(CONFIG, mode_index(current_mode(current)));
    out.one_hot(CONFIG + 3, proxy_index(current_proxy(current)));
    out.one_hot(CONFIG + 5, wait_index(current_wait(current)));
    for (at, key) in SWITCHES.iter().enumerate() {
        out.one_hot(
            CONFIG + 9 + at * 2,
            usize::from(effective_flag(current, *key)),
        );
    }
    if current
        .list(Key::NetworkBlacklist)
        .is_some_and(|list| !list.is_empty())
    {
        out.set(CONFIG + 19, 1.0);
    }

    // What the edit does.
    let resources = ctx.observation.resources.as_ref();
    let mut identifier = None;
    for edit in cand.edits.edits() {
        if let Some(slot) = learnable_slot(edit.key) {
            out.one_hot(EDIT_KEY, slot);
        }
        out.one_hot(EDIT_OP, usize::from(edit.op.code()));

        if let Some((bucket, ident)) = value_bucket(edit, ctx.observation) {
            out.one_hot(VALUE_BUCKET, bucket.min(VALUE_BUCKET_W - 1));
            if ident.is_some() {
                identifier = ident;
            }
        }
    }

    if !cand.edits.is_keep() {
        out.one_hot(PAIR, pair_index(cand));
    }

    // What the caller wants.
    match ctx.need {
        DeclaredNeed::Fields => out.one_hot(NEED_BITS, 1),
        DeclaredNeed::Links | DeclaredNeed::Metadata => out.one_hot(NEED_BITS, 2),
        DeclaredNeed::Screenshot => out.one_hot(NEED_BITS, 3),
        _ => out.one_hot(NEED_BITS, 0),
    }

    // What was seen.
    if let Some(resources) = resources {
        out.one_hot(OBS_HOSTS, hosts_bucket(resources.third_party_hosts));
    }
    if let Some(ident) = identifier {
        out.one_hot(
            OBS_SHARE,
            usize::from(ident.share_bucket).min(OBS_SHARE_W - 1),
        );
        out.one_hot(OBS_CLASS, ext_index(ident.class));
        out.one_hot(OBS_PARTY, usize::from(ident.third_party));
    }

    // What was not.
    if resources.is_none() {
        out.one_hot(MISSING, 0);
    }
    if identifier.is_none() {
        out.one_hot(MISSING, 1);
    }
    if ctx
        .observation
        .memory
        .is_none_or(|memory| memory.observations == 0)
    {
        out.one_hot(MISSING, 2);
    }
    if ctx.routed.source == RouteSource::Caller || ctx.current.request().is_none() {
        out.one_hot(MISSING, 3);
    }

    out.set(BIAS, 1.0);
    out
}

/// The index of an edit's new value within its kind, and the observed
/// identifier an append names. `None` for an op with no single value, and for
/// an append whose pattern was not observed.
pub(crate) fn value_bucket<'o>(
    edit: &Edit,
    observation: &'o Observation,
) -> Option<(usize, Option<&'o Identifier>)> {
    match &edit.op {
        Op::Set(Value::Mode(mode)) => Some((mode_index(*mode), None)),
        Op::Set(Value::Proxy(pool)) => Some((proxy_index(*pool), None)),
        Op::Set(Value::Millis(millis)) => Some((wait_index(*millis), None)),
        Op::Set(Value::Bool(value)) => Some((usize::from(*value), None)),
        Op::Set(Value::Int(_)) => None,
        Op::Append(entries) => {
            let (rank, ident) = observation.identifier(entries.first()?)?;
            Some((rank, Some(ident)))
        }
        Op::Remove(_) | Op::Replace(_) | Op::Clear => None,
    }
}

/// A mode's index in the request kind. An unknown mode reads as the last.
fn mode_index(mode: spider_route::RequestMode) -> usize {
    MODES
        .iter()
        .position(|m| *m == mode)
        .unwrap_or(MODES.len() - 1)
}

/// A pool's index in the proxy kind. An unknown pool reads as the last.
fn proxy_index(pool: spider_route::ProxyPool) -> usize {
    PROXIES
        .iter()
        .position(|p| *p == pool)
        .unwrap_or(PROXIES.len() - 1)
}

/// The largest wait bucket at or below this many milliseconds.
fn wait_index(millis: u32) -> usize {
    WAIT_BUCKETS
        .iter()
        .rposition(|bucket| *bucket <= millis)
        .unwrap_or(0)
}

/// Which pair an edit set is, or the slot after the pairs for none.
fn pair_index(cand: &Candidate) -> usize {
    let edits = cand.edits.edits();
    if let [a, b] = edits {
        for (at, (first, second)) in PAIRS.iter().enumerate() {
            if (a.key == *first && b.key == *second) || (a.key == *second && b.key == *first) {
                return at;
            }
        }
    }
    PAIRS.len()
}

/// Third party hosts, bucketed.
const fn hosts_bucket(hosts: u16) -> usize {
    match hosts {
        0 => 0,
        1..=2 => 1,
        3..=5 => 2,
        6..=10 => 3,
        _ => 4,
    }
}

/// An extension class's slot, in the router's own order. A class this version
/// has not heard of reads as other.
pub(crate) const fn ext_index(class: ExtClass) -> usize {
    match class {
        ExtClass::None => 0,
        ExtClass::Markup => 1,
        ExtClass::Xml => 2,
        ExtClass::Json => 3,
        ExtClass::Feed => 4,
        ExtClass::Text => 5,
        ExtClass::Csv => 6,
        ExtClass::Pdf => 7,
        ExtClass::Office => 8,
        ExtClass::Image => 9,
        ExtClass::Media => 10,
        ExtClass::Archive => 11,
        ExtClass::Asset => 12,
        _ => 13,
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
    use crate::candidates::{generate, MULTIPLIERS};
    use crate::observe::{Identifier, ResourceSummary};
    use crate::row::{need_label, status_label, ALL_NEEDS, ALL_STATUSES};
    use crate::schema::{Schema, SCHEMA_VERSION};
    use crate::testing::Fixture;
    use spider_route::{SiteMemory, StatusClass};

    #[test]
    fn edit_features_hold_no_text() {
        assert_eq!(
            std::mem::size_of::<EditFeatures>(),
            EDIT_DIM * std::mem::size_of::<f32>()
        );
    }

    #[test]
    fn blocks_are_contiguous_and_count_is_12() {
        let mut next = 0;
        for (name, base, width) in BLOCKS {
            assert_eq!(
                base, next,
                "{name} does not start where the last block ended"
            );
            next = base + width;
        }
        assert_eq!(next, EDIT_DIM);
        assert_eq!(BLOCKS.len(), 12);
        let widths: Vec<usize> = BLOCKS.iter().map(|(_, _, w)| *w).collect();
        assert_eq!(widths, [24, 16, 5, 8, 8, 4, 5, 5, 14, 2, 4, 1]);
        // The vocabularies fit their blocks.
        assert_eq!(ext_index(ExtClass::Other) + 1, OBS_CLASS_W);
        assert_eq!(PAIRS.len() + 1, 5);
        assert_eq!(9 + 10 + 1 + 4, CONFIG_W);
    }

    fn assert_unit(feats: &EditFeatures, what: &str) {
        for (slot, value) in feats.as_slice().iter().enumerate() {
            assert!(
                value.is_finite() && (-1.0..=1.0).contains(value),
                "slot {slot} is {value} for {what}"
            );
        }
    }

    #[test]
    fn all_slots_finite_within_unit_range() {
        let schema = Schema::v1();
        let mut fixtures = vec![Fixture::cold(), Fixture::observed(), Fixture::pinned()];

        let mut hostile = Fixture::observed();
        hostile.observation.resources = Some(ResourceSummary {
            requests: u16::MAX,
            third_party_hosts: u16::MAX,
            third_party_bytes: u64::MAX,
            first_party_bytes: u64::MAX,
            candidates: vec![Identifier {
                pattern: "tracker-alpha.example".into(),
                class: ExtClass::Other,
                share_bucket: u8::MAX,
                count_bucket: u8::MAX,
                third_party: true,
            }],
        });
        hostile.observation.memory = Some(SiteMemory {
            observations: u32::MAX,
            success_rate: f32::NAN,
            streak: i16::MIN,
            last_status: StatusClass::ServerError,
        });
        hostile.current.wait_for = Some(spider_cloud_agent::params::WaitFor::idle_network(
            spider_cloud_agent::params::Timeout::from_secs(u64::MAX / 1_000),
        ));
        hostile.current.network_blacklist = Some(Vec::new());
        hostile.cap = f32::INFINITY;
        fixtures.push(hostile);

        let mut empty = Fixture::observed();
        empty.observation.resources = Some(ResourceSummary::default());
        empty.current = Default::default();
        fixtures.push(empty);

        let mut checked = 0;
        for fixture in &fixtures {
            let ctx = fixture.ctx();
            for cand in generate(&schema, &ctx) {
                assert_unit(&featurize_edit(&ctx, &cand), &format!("{:?}", cand.edits));
                checked += 1;
            }
        }
        assert!(checked > 40, "only {checked} candidates checked");
    }

    #[test]
    fn keep_has_no_edit_bits() {
        let fixture = Fixture::observed();
        let ctx = fixture.ctx();
        let keep = featurize_edit(&ctx, &Candidate::keep(&ctx));

        for slot in EDIT_KEY..NEED_BITS {
            assert_eq!(keep.get(slot), Some(0.0), "slot {slot} is set for keep");
        }
        for slot in OBS_SHARE..MISSING {
            assert_eq!(keep.get(slot), Some(0.0), "slot {slot} is set for keep");
        }

        // Every other candidate sets at least one key and one op.
        for cand in generate(&Schema::v1(), &ctx).iter().skip(1) {
            let feats = featurize_edit(&ctx, cand);
            let keys: f32 = (EDIT_KEY..EDIT_OP).map(|s| feats.get(s).unwrap()).sum();
            let ops: f32 = (EDIT_OP..VALUE_BUCKET).map(|s| feats.get(s).unwrap()).sum();
            assert!(keys >= 1.0 && ops >= 1.0, "{:?}", cand.edits);
        }
    }

    #[test]
    fn missing_observation_is_a_value_not_a_gap() {
        let cold = Fixture::cold();
        let ctx = cold.ctx();
        let feats = featurize_edit(&ctx, &Candidate::keep(&ctx));
        assert_eq!(feats.get(MISSING), Some(1.0));
        assert_eq!(feats.get(MISSING + 1), Some(1.0));
        assert_eq!(feats.get(MISSING + 2), Some(1.0));
        assert_eq!(feats.get(MISSING + 3), Some(0.0));
        assert_eq!(feats.get(BIAS), Some(1.0));
        // Unset switches read as the service reads them.
        assert_eq!(
            feats.get(CONFIG + 9 + 2 * 2 + 1),
            Some(1.0),
            "block ads is on"
        );
        assert_eq!(
            feats.get(CONFIG + 9),
            Some(1.0),
            "interception is not disabled"
        );

        let mut observed = Fixture::observed();
        observed.observation.memory = Some(SiteMemory {
            observations: 4,
            ..SiteMemory::cold()
        });
        let ctx = observed.ctx();
        let append = generate(&Schema::v1(), &ctx)
            .into_iter()
            .find(|c| c.edits.edits().len() == 1 && c.edits.get(Key::NetworkBlacklist).is_some())
            .unwrap();
        let feats = featurize_edit(&ctx, &append);
        for slot in MISSING..BIAS {
            assert_eq!(feats.get(slot), Some(0.0), "missing slot {slot}");
        }
        assert_eq!(feats.get(OBS_PARTY + 1), Some(1.0));
        assert_eq!(feats.get(OBS_HOSTS + 2), Some(1.0), "five hosts");

        let mut caller_routed = Fixture::cold();
        caller_routed.current = Default::default();
        let ctx = caller_routed.ctx();
        let feats = featurize_edit(&ctx, &Candidate::keep(&ctx));
        assert_eq!(feats.get(MISSING + 3), Some(1.0));
    }

    /// The layout and vocabulary as a document, for the trainer.
    fn mirror() -> String {
        let schema = Schema::v1();
        let blocks: Vec<_> = BLOCKS
            .iter()
            .map(|(name, base, width)| serde_json::json!({"name": name, "base": base, "width": width}))
            .collect();
        let keys: Vec<_> = schema
            .specs()
            .iter()
            .map(|spec| {
                serde_json::json!({
                    "name": format!("{:?}", spec.key),
                    "index": spec.key.index(),
                    "wire": spec.key.wire(),
                    "learnable": spec.learnable,
                    "content_changing": spec.content_changing,
                })
            })
            .collect();
        let ops: Vec<_> = crate::edit::Op::NAMES
            .iter()
            .enumerate()
            .map(|(code, name)| serde_json::json!({"name": name, "code": code}))
            .collect();
        let needs: Vec<_> = ALL_NEEDS.iter().map(|need| need_label(*need)).collect();
        let status: Vec<_> = ALL_STATUSES.iter().map(|s| status_label(*s)).collect();
        let multipliers: Vec<_> = MULTIPLIERS
            .iter()
            .map(|(mode, proxy, wait, cost)| {
                serde_json::json!({
                    "mode": mode.as_str(),
                    "proxy": proxy.as_str(),
                    "wait_ms": wait,
                    "multiplier": cost,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "edit_feature_version": EDIT_FEATURE_VERSION,
            "edit_dim": EDIT_DIM,
            "blocks": blocks,
            "keys": keys,
            "ops": ops,
            "needs": needs,
            "status": status,
            "multipliers": multipliers,
        });
        let mut text = serde_json::to_string_pretty(&doc).unwrap();
        text.push('\n');
        text
    }

    fn mirror_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("training/fixtures/schema-v2.json")
    }

    #[test]
    fn the_schema_mirror_is_committed() {
        let expected = mirror();
        let committed = std::fs::read_to_string(mirror_path()).unwrap_or_default();
        assert!(
            committed == expected,
            "training/fixtures/schema-v2.json does not match the constants in spider-optimize. \
             Regenerate it with `cargo test -p spider-optimize write_schema_mirror -- --ignored` \
             and commit the result.",
        );
    }

    #[test]
    #[ignore = "writes training/fixtures/schema-v2.json"]
    fn write_schema_mirror() {
        let path = mirror_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, mirror()).unwrap();
    }
}
