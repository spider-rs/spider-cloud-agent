//! The edits worth scoring for one request.
//!
//! [`generate`] lists them in a fixed order: keep first, then one edit on
//! each learnable key the caller left alone, then the pairs in [`PAIRS`], then
//! one blacklist append per observed identifier. Every candidate has passed
//! [`crate::validate()`] before it is listed, duplicates are dropped, and the
//! list stops at [`MAX_CANDIDATES`]. The same context gives the same list.

use crate::edit::{Edit, EditSet, Op, Value};
use crate::observe::Observation;
use crate::params::Params;
use crate::schema::{service_default, Key, Schema, MODES, PROXIES, WAIT_BUCKETS};
use crate::validate::validate;
use spider_route::{DeclaredNeed, ProxyPool, RequestMode, RouteDecision};
use url::Url;

/// The most candidates [`generate`] returns, keep included.
pub const MAX_CANDIDATES: usize = 24;

/// What each (mode, proxy, idle wait) triple is expected to cost, as a
/// multiple of a plain fetch of the same page.
///
/// The same table the client's explorer prices its arms with, which is the
/// escalation ladder's own arithmetic, so an edit is refused on the same
/// numbers an escalation is. The schema mirror in `training/fixtures` carries
/// it for the trainer.
pub const MULTIPLIERS: [(RequestMode, ProxyPool, u32, f32); 6] = [
    (RequestMode::Http, ProxyPool::Isp, 0, 1.0),
    (RequestMode::Smart, ProxyPool::Isp, 0, 1.5),
    (RequestMode::Browser, ProxyPool::Isp, 0, 4.0),
    (RequestMode::Browser, ProxyPool::Isp, 10_000, 5.0),
    (RequestMode::Smart, ProxyPool::Residential, 0, 3.0),
    (RequestMode::Browser, ProxyPool::Residential, 10_000, 8.0),
];

/// The pairs of keys one candidate may edit together.
pub const PAIRS: [(Key, Key); 4] = [
    (Key::Request, Key::WaitIdleMillis),
    (Key::Request, Key::Proxy),
    (Key::BlockStylesheets, Key::FullResources),
    (Key::NetworkBlacklist, Key::Request),
];

/// One request, as the optimizer sees it.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    /// The address being fetched. Read for its shape only, through the
    /// router's featurizer.
    pub url: &'a Url,
    /// What the caller wants back.
    pub need: DeclaredNeed,
    /// The request as it is about to go out, after the router and the plan.
    pub current: &'a dyn Params,
    /// The request as the caller left it. A field set here is never edited.
    pub caller: &'a dyn Params,
    /// What the router decided for this request.
    pub routed: &'a RouteDecision,
    /// What is known about the site.
    pub observation: &'a Observation,
    /// The dearest multiplier the caller's budget allows.
    pub multiplier_cap: f32,
}

/// An edit set and what it is expected to cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The edits.
    pub edits: EditSet,
    /// The [`MULTIPLIERS`] price of the request with these edits applied.
    pub multiplier: f32,
}

impl Candidate {
    /// The candidate that changes nothing, priced for this context.
    pub fn keep(ctx: &Context<'_>) -> Candidate {
        Candidate {
            edits: EditSet::keep(),
            multiplier: multiplier(ctx, &EditSet::keep()),
        }
    }
}

/// List the candidates for one request. See the module docs for the order.
///
/// A pair joins every other value of one key with every other value of the
/// other, and a blacklist value is an append of one observed identifier. An
/// idle wait of zero is never proposed, because applying it writes nothing.
pub fn generate(schema: &Schema, ctx: &Context<'_>) -> Vec<Candidate> {
    let mut list = List {
        schema,
        ctx,
        out: Vec::with_capacity(MAX_CANDIDATES),
    };
    list.out.push(Candidate::keep(ctx));

    for spec in schema.learnable() {
        // Blacklist appends are listed last, after the pairs.
        if spec.key != Key::NetworkBlacklist && left_to_us(ctx, spec.key) {
            for edit in alternatives(ctx, spec.key) {
                list.offer(vec![edit]);
            }
        }
    }

    for (first, second) in PAIRS {
        if list.is_full() || !left_to_us(ctx, first) || !left_to_us(ctx, second) {
            continue;
        }
        let firsts = alternatives(ctx, first);
        let seconds = alternatives(ctx, second);
        for a in &firsts {
            for b in &seconds {
                if !list.is_full() {
                    list.offer(vec![a.clone(), b.clone()]);
                }
            }
        }
    }

    if let Some(resources) = &ctx.observation.resources {
        for ident in &resources.candidates {
            if !list.is_full() {
                list.offer(vec![append(&ident.pattern)]);
            }
        }
    }

    list.out
}

/// The list being built, and what it is checked against.
struct List<'s, 'c, 'a> {
    schema: &'s Schema,
    ctx: &'c Context<'a>,
    out: Vec<Candidate>,
}

impl List<'_, '_, '_> {
    /// Whether the cap is reached, so nothing more is built.
    fn is_full(&self) -> bool {
        self.out.len() >= MAX_CANDIDATES
    }

    /// Add a candidate if it is valid, new, and there is room.
    fn offer(&mut self, edits: Vec<Edit>) {
        if self.is_full() {
            return;
        }
        let Ok(edits) = EditSet::new(edits) else {
            return;
        };
        let candidate = Candidate {
            multiplier: multiplier(self.ctx, &edits),
            edits,
        };
        if validate(self.schema, self.ctx, &candidate).is_ok()
            && !self.out.iter().any(|seen| seen.edits == candidate.edits)
        {
            self.out.push(candidate);
        }
    }
}

/// Whether the caller left this key for an edit to set.
fn left_to_us(ctx: &Context<'_>, key: Key) -> bool {
    !ctx.caller.is_set(key)
        && !(key == Key::NetworkBlacklist && ctx.caller.is_set(Key::NetworkWhitelist))
}

/// Every edit on one key that changes its current value.
fn alternatives(ctx: &Context<'_>, key: Key) -> Vec<Edit> {
    let current = ctx.current;
    match key {
        Key::Request => MODES
            .iter()
            .filter(|mode| **mode != current_mode(current))
            .map(|mode| Edit::set(key, Value::Mode(*mode)))
            .collect(),
        Key::Proxy => PROXIES
            .iter()
            .filter(|pool| **pool != current_proxy(current))
            .map(|pool| Edit::set(key, Value::Proxy(*pool)))
            .collect(),
        Key::WaitIdleMillis => WAIT_BUCKETS
            .iter()
            .filter(|millis| **millis > 0 && **millis != current_wait(current))
            .map(|millis| Edit::set(key, Value::Millis(*millis)))
            .collect(),
        Key::NetworkBlacklist => ctx
            .observation
            .resources
            .as_ref()
            .map(|resources| {
                resources
                    .candidates
                    .iter()
                    .map(|ident| append(&ident.pattern))
                    .collect()
            })
            .unwrap_or_default(),
        Key::DisableIntercept
        | Key::FullResources
        | Key::BlockAds
        | Key::BlockAnalytics
        | Key::BlockStylesheets => {
            vec![Edit::set(key, Value::Bool(!effective_flag(current, key)))]
        }
        _ => Vec::new(),
    }
}

/// An append of one pattern to the blacklist.
fn append(pattern: &str) -> Edit {
    Edit {
        key: Key::NetworkBlacklist,
        op: Op::Append(vec![pattern.to_string()]),
    }
}

/// The mode a request goes out with. Unset is the service default.
pub(crate) fn current_mode(params: &dyn Params) -> RequestMode {
    params.request().unwrap_or_default()
}

/// The pool a request goes out from. Unset is the service default.
pub(crate) fn current_proxy(params: &dyn Params) -> ProxyPool {
    params.proxy().unwrap_or_default()
}

/// The idle wait a request carries, zero when it carries none.
pub(crate) fn current_wait(params: &dyn Params) -> u32 {
    params.idle_wait_millis().unwrap_or(0)
}

/// A switch's value as the service will read it.
pub(crate) fn effective_flag(params: &dyn Params, key: Key) -> bool {
    params.flag(key).unwrap_or(service_default(key))
}

/// The (mode, proxy, idle wait) a request has once these edits are applied.
///
/// Mirrors [`EditSet::apply`]: a zero wait writes nothing, so the current wait
/// stands.
pub(crate) fn resulting(ctx: &Context<'_>, edits: &EditSet) -> (RequestMode, ProxyPool, u32) {
    let mut mode = current_mode(ctx.current);
    let mut proxy = current_proxy(ctx.current);
    let mut wait = current_wait(ctx.current);

    for edit in edits.edits() {
        match (edit.key, &edit.op) {
            (Key::Request, Op::Set(Value::Mode(m))) => mode = *m,
            (Key::Proxy, Op::Set(Value::Proxy(p))) => proxy = *p,
            (Key::WaitIdleMillis, Op::Set(Value::Millis(n))) if *n > 0 => wait = *n,
            _ => {}
        }
    }

    (mode, proxy, wait)
}

/// How heavy a mode is, lightest first. A mode this version has not heard of
/// is priced as the heaviest.
pub(crate) const fn mode_rank(mode: RequestMode) -> u8 {
    match mode {
        RequestMode::Http => 0,
        RequestMode::Smart => 1,
        _ => 2,
    }
}

/// How dear a pool is, cheapest first. An unknown pool is the dearest.
pub(crate) const fn proxy_rank(pool: ProxyPool) -> u8 {
    match pool {
        ProxyPool::Isp => 0,
        _ => 1,
    }
}

/// The price of a (mode, proxy, idle wait) triple.
///
/// A triple in [`MULTIPLIERS`] is its own price. Any other is priced as the
/// cheapest listed triple at least as heavy on all three counts, so a price is
/// never an underestimate: a residential HTTP fetch costs what a residential
/// smart fetch does. A triple heavier than every row, such as a wait past ten
/// seconds, is priced as the dearest row.
pub fn price(mode: RequestMode, proxy: ProxyPool, wait: u32) -> f32 {
    let mut best: Option<f32> = None;
    let mut dearest = 0.0f32;

    for (arm_mode, arm_proxy, arm_wait, cost) in MULTIPLIERS {
        dearest = dearest.max(cost);
        if mode_rank(arm_mode) >= mode_rank(mode)
            && proxy_rank(arm_proxy) >= proxy_rank(proxy)
            && arm_wait >= wait
        {
            best = Some(best.map_or(cost, |seen| seen.min(cost)));
        }
    }

    best.unwrap_or(dearest)
}

/// The price of a request once these edits are applied.
pub(crate) fn multiplier(ctx: &Context<'_>, edits: &EditSet) -> f32 {
    let (mode, proxy, wait) = resulting(ctx, edits);
    price(mode, proxy, wait)
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
    use crate::testing::Fixture;
    use spider_cloud_agent::params::{Timeout, WaitFor};
    use spider_route::StatusClass;

    #[test]
    fn generation_is_deterministic() {
        let fixture = Fixture::observed();
        let schema = Schema::v1();
        let first = generate(&schema, &fixture.ctx());

        assert!(first.len() > 10, "only {} candidates", first.len());
        for _ in 0..5 {
            assert_eq!(generate(&schema, &fixture.ctx()), first);
        }
        // A copy of the inputs, not the same inputs, gives the same list.
        assert_eq!(generate(&schema, &Fixture::observed().ctx()), first);
    }

    #[test]
    fn keep_is_always_first() {
        let schema = Schema::v1();
        let mut fixtures = vec![Fixture::cold(), Fixture::observed(), Fixture::pinned()];
        let mut limited = Fixture::observed();
        limited.observation.last_status = StatusClass::RateLimited;
        fixtures.push(limited);
        let mut broke = Fixture::observed();
        broke.cap = 0.0;
        fixtures.push(broke);

        for fixture in &fixtures {
            let list = generate(&schema, &fixture.ctx());
            assert!(list[0].edits.is_keep());
            assert_eq!(list.iter().filter(|c| c.edits.is_keep()).count(), 1);
        }
    }

    #[test]
    fn no_candidate_survives_validation_with_a_pinned_key() {
        let schema = Schema::v1();
        let fixture = Fixture::pinned();
        let ctx = fixture.ctx();
        let list = generate(&schema, &ctx);

        for cand in &list {
            for edit in cand.edits.edits() {
                assert!(
                    !ctx.caller.is_set(edit.key),
                    "{:?} edits a key the caller set",
                    edit.key
                );
            }
        }
        // The caller pinned mode, proxy and wait, and left the switches.
        assert!(list.len() > 1);

        // Everything pinned leaves only keep.
        let mut all = Fixture::observed();
        all.caller = crate::schema::tests::populated();
        assert_eq!(generate(&schema, &all.ctx()).len(), 1);
    }

    #[test]
    fn never_more_than_max_candidates() {
        let schema = Schema::v1();
        // A smart fetch with four identifiers offers more valid candidates
        // than the cap, so the cap is what stops the list.
        let fixture = Fixture::observed();
        let list = generate(&schema, &fixture.ctx());

        assert_eq!(list.len(), MAX_CANDIDATES, "the cap was not reached");
        // What the cap cut is the tail: the last appends.
        let appends = list
            .iter()
            .filter(|c| c.edits.edits().len() == 1 && c.edits.get(Key::NetworkBlacklist).is_some())
            .count();
        assert!(appends < 4, "{appends} single appends survived the cap");
    }

    #[test]
    fn a_candidate_is_priced_by_what_it_sends() {
        assert_eq!(price(RequestMode::Http, ProxyPool::Isp, 0), 1.0);
        assert_eq!(price(RequestMode::Browser, ProxyPool::Isp, 10_000), 5.0);
        assert_eq!(price(RequestMode::Browser, ProxyPool::Isp, 2_000), 5.0);
        assert_eq!(price(RequestMode::Http, ProxyPool::Residential, 0), 3.0);
        assert_eq!(price(RequestMode::Browser, ProxyPool::Residential, 0), 8.0);
        assert_eq!(price(RequestMode::Smart, ProxyPool::Isp, 60_000), 8.0);

        let mut fixture = Fixture::cold();
        fixture.current.request = Some(RequestMode::Smart);
        fixture.current.wait_for = Some(WaitFor::idle_network(Timeout::from_millis(0)));
        let ctx = fixture.ctx();
        let wait = EditSet::new(vec![
            Edit::set(Key::Request, Value::Mode(RequestMode::Browser)),
            Edit::set(Key::WaitIdleMillis, Value::Millis(10_000)),
        ])
        .unwrap();
        assert_eq!(multiplier(&ctx, &wait), 5.0);
        assert_eq!(Candidate::keep(&ctx).multiplier, 1.5);
    }
}
