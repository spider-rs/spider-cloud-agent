//! Whether any candidate is worth applying.
//!
//! [`choose`] scores keep and every valid candidate, ranks the candidates by
//! credits per successful result, and applies the best one only when it
//! clears every threshold in [`Gate`]. Any doubt keeps the request as it is:
//! no model, a NaN anywhere, too little evidence, too little gain, or a caller
//! who fixed the mode, pool or country.
//!
//! That last guard matches the client's explorer. A caller who pinned any of
//! the three has said how the request should be sent, and an edit to what is
//! left would be attributed to settings the caller chose.

use crate::candidates::{multiplier, Candidate, Context};
use crate::edit::EditSet;
use crate::features::{featurize_edit, Input};
use crate::model::{Score, Scorer};
use crate::schema::{Key, Schema};
use crate::validate::validate;
use spider_route::{featurize, RouteInput};
use std::cmp::Ordering;

/// The thresholds a candidate has to clear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gate {
    /// The lowest success chance a candidate may have.
    pub min_p_success: f32,
    /// How much lower, as a fraction of keep's, a candidate's credits per
    /// success must be.
    pub min_gain: f32,
    /// The least evidence a candidate's estimate may rest on.
    pub min_support: f32,
    /// The dearest multiplier a candidate may have.
    pub max_multiplier: f32,
}

impl Default for Gate {
    fn default() -> Gate {
        Gate {
            min_p_success: 0.9,
            min_gain: 0.05,
            min_support: 1.0,
            max_multiplier: 8.0,
        }
    }
}

/// Why the request was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The scorer has no weights.
    NoModel,
    /// A score was NaN, or no candidate had enough support.
    NoEvidence,
    /// There was nothing but keep to choose from.
    NoCandidates,
    /// No candidate beat keep by enough.
    NoGain,
    /// No candidate cleared the success floor.
    BelowFloor,
    /// Every candidate failed validation or was dearer than the gate allows.
    Unsupported,
    /// The caller fixed the mode, the pool or the country.
    Pinned,
}

/// What to do with the request.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// Send it as it is.
    Keep(Reason),
    /// Apply these edits.
    Apply {
        /// The edits.
        edits: EditSet,
        /// The candidate's score.
        expected: Score,
        /// Keep's score.
        baseline: Score,
    },
}

/// One scored candidate.
#[derive(Clone, Copy)]
struct Ranked<'c> {
    candidate: &'c Candidate,
    score: Score,
    objective: f32,
}

/// Credits per successful result. Lower is better; no chance of success is
/// infinitely expensive.
fn objective(score: &Score) -> f32 {
    if score.p_success > 0.0 {
        score.credits / score.p_success
    } else {
        f32::INFINITY
    }
}

/// Pick the edit to apply, or the reason to keep the request.
///
/// Deterministic, and allocates nothing unless it returns
/// [`Choice::Apply`], which owns a copy of the chosen edits.
pub fn choose(scorer: &dyn Scorer, ctx: &Context<'_>, cands: &[Candidate], gate: &Gate) -> Choice {
    if ctx.caller.is_set(Key::Request)
        || ctx.caller.is_set(Key::Proxy)
        || ctx.caller.is_set(Key::CountryCode)
    {
        return Choice::Keep(Reason::Pinned);
    }
    if scorer.version().is_none() {
        return Choice::Keep(Reason::NoModel);
    }

    let schema = Schema::v1();
    let offered = cands.iter().filter(|c| !c.edits.is_keep());
    if offered.clone().next().is_none() {
        return Choice::Keep(Reason::NoCandidates);
    }

    // The router's features for the request as the router saw it. The caller
    // pinned nothing, or this would have returned above.
    let mut route = RouteInput::new(ctx.url, ctx.need);
    if let Some(memory) = &ctx.observation.memory {
        route = route.with_memory(memory);
    }
    let base = featurize(&route);

    let keep = Candidate {
        edits: EditSet::keep(),
        multiplier: multiplier(ctx, &EditSet::keep()),
    };
    let baseline = scorer.score(&Input {
        base,
        edit: featurize_edit(ctx, &keep),
    });
    if baseline.has_nan() {
        return Choice::Keep(Reason::NoEvidence);
    }
    let baseline_objective = objective(&baseline);

    let mut best: Option<Ranked<'_>> = None;
    // Set when a candidate that would have beaten keep was held back, so the
    // reason names what stopped a gain rather than the gain that was left.
    let mut below_floor = false;
    let mut thin = false;

    for candidate in offered {
        if validate(&schema, ctx, candidate).is_err() {
            continue;
        }
        if multiplier(ctx, &candidate.edits) > gate.max_multiplier {
            continue;
        }

        let score = scorer.score(&Input {
            base,
            edit: featurize_edit(ctx, candidate),
        });
        if score.has_nan() {
            return Choice::Keep(Reason::NoEvidence);
        }
        let ranked = Ranked {
            candidate,
            score,
            objective: objective(&score),
        };
        if score.p_success < gate.min_p_success || score.p_success < baseline.p_success - 0.01 {
            below_floor |= beats(&ranked, &baseline, baseline_objective, gate);
            continue;
        }
        if score.support < gate.min_support {
            thin |= beats(&ranked, &baseline, baseline_objective, gate);
            continue;
        }

        // Strictly better only, so on a tie the earlier candidate stands.
        let better = match &best {
            None => true,
            Some(seen) => match ranked.objective.partial_cmp(&seen.objective) {
                Some(Ordering::Less) => true,
                Some(Ordering::Equal) => ranked.score.latency_ms < seen.score.latency_ms,
                _ => false,
            },
        };
        if better {
            best = Some(ranked);
        }
    }

    match best {
        Some(best) if beats(&best, &baseline, baseline_objective, gate) => Choice::Apply {
            edits: best.candidate.edits.clone(),
            expected: best.score,
            baseline,
        },
        _ if below_floor => Choice::Keep(Reason::BelowFloor),
        _ if thin => Choice::Keep(Reason::NoEvidence),
        Some(_) => Choice::Keep(Reason::NoGain),
        None => Choice::Keep(Reason::Unsupported),
    }
}

/// Whether a candidate beats keep. Keep wins every tie: a candidate has to be
/// cheaper per success by the gain, or exactly as cheap and faster when no
/// gain is required.
fn beats(ranked: &Ranked<'_>, baseline: &Score, baseline_objective: f32, gate: &Gate) -> bool {
    match ranked.objective.partial_cmp(&baseline_objective) {
        Some(Ordering::Less) => ranked.objective <= baseline_objective * (1.0 - gate.min_gain),
        Some(Ordering::Equal) => {
            gate.min_gain <= 0.0 && ranked.score.latency_ms < baseline.latency_ms
        }
        _ => false,
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
    use crate::candidates::generate;
    use crate::features::EDIT_KEY;
    use crate::model::{ModelVersion, NoModel};
    use crate::schema::learnable_slot;
    use crate::testing::Fixture;
    use spider_route::{Country, ProxyPool, RequestMode};

    /// A scorer that answers by which learnable key the edit touches, read
    /// back out of the features, with keep's score for keep.
    struct ByKey {
        keep: Score,
        keyed: Vec<(Key, Score)>,
    }

    impl Scorer for ByKey {
        fn score(&self, input: &Input) -> Score {
            for (key, score) in &self.keyed {
                let slot = EDIT_KEY + learnable_slot(*key).unwrap();
                if input.edit.get(slot) == Some(1.0) {
                    return *score;
                }
            }
            self.keep
        }

        fn version(&self) -> Option<ModelVersion> {
            Some(ModelVersion(1))
        }
    }

    const fn score(p_success: f32, latency_ms: f32, credits: f32, support: f32) -> Score {
        Score {
            p_success,
            latency_ms,
            credits,
            support,
        }
    }

    /// Keep costs 4 credits at 0.95; blocking ads costs 2 at 0.96.
    fn cheaper_block() -> ByKey {
        ByKey {
            keep: score(0.95, 900.0, 4.0, 50.0),
            keyed: vec![(Key::BlockAds, score(0.96, 800.0, 2.0, 50.0))],
        }
    }

    fn run(scorer: &dyn Scorer, fixture: &Fixture, gate: &Gate) -> Choice {
        let ctx = fixture.ctx();
        let cands = generate(&Schema::v1(), &ctx);
        choose(scorer, &ctx, &cands, gate)
    }

    #[test]
    fn a_confident_cheaper_candidate_is_applied() {
        let choice = run(&cheaper_block(), &Fixture::observed(), &Gate::default());
        let Choice::Apply {
            edits,
            expected,
            baseline,
        } = choice
        else {
            panic!("expected an edit, got {choice:?}");
        };
        assert_eq!(edits.edits().len(), 1);
        assert_eq!(edits.edits()[0].key, Key::BlockAds);
        assert_eq!(expected.credits, 2.0);
        assert_eq!(baseline.credits, 4.0);
    }

    #[test]
    fn no_model_always_keeps() {
        for fixture in [Fixture::cold(), Fixture::observed()] {
            assert_eq!(
                run(&NoModel, &fixture, &Gate::default()),
                Choice::Keep(Reason::NoModel)
            );
        }
        let ctx = Fixture::cold();
        let ctx = ctx.ctx();
        assert_eq!(
            choose(
                &cheaper_block(),
                &ctx,
                &[Candidate::keep(&ctx)],
                &Gate::default()
            ),
            Choice::Keep(Reason::NoCandidates)
        );
    }

    #[test]
    fn a_gate_never_applies_below_the_success_floor() {
        let fixture = Fixture::observed();
        let gate = Gate::default();

        // Cheaper, well supported, and under the floor.
        let under = ByKey {
            keep: score(0.95, 900.0, 4.0, 50.0),
            keyed: vec![(Key::BlockAds, score(0.89, 800.0, 1.0, 50.0))],
        };
        assert_eq!(
            run(&under, &fixture, &gate),
            Choice::Keep(Reason::BelowFloor)
        );

        // Over the floor, and more than a point worse than keep.
        let worse = ByKey {
            keep: score(0.99, 900.0, 4.0, 50.0),
            keyed: vec![(Key::BlockAds, score(0.97, 800.0, 1.0, 50.0))],
        };
        assert_eq!(
            run(&worse, &fixture, &gate),
            Choice::Keep(Reason::BelowFloor)
        );

        // Enough of a gain but too little evidence.
        let thin = ByKey {
            keep: score(0.95, 900.0, 4.0, 50.0),
            keyed: vec![(Key::BlockAds, score(0.96, 800.0, 1.0, 0.5))],
        };
        assert_eq!(
            run(&thin, &fixture, &gate),
            Choice::Keep(Reason::NoEvidence)
        );

        // Too little gain.
        let marginal = ByKey {
            keep: score(0.95, 900.0, 4.0, 50.0),
            keyed: vec![(Key::BlockAds, score(0.95, 800.0, 3.9, 50.0))],
        };
        assert_eq!(
            run(&marginal, &fixture, &gate),
            Choice::Keep(Reason::NoGain)
        );
    }

    #[test]
    fn keep_wins_ties() {
        let same = score(0.95, 900.0, 4.0, 50.0);
        let tied = ByKey {
            keep: same,
            keyed: vec![(Key::BlockAds, same)],
        };
        let no_gain_needed = Gate {
            min_gain: 0.0,
            ..Gate::default()
        };
        assert_eq!(
            run(&tied, &Fixture::observed(), &no_gain_needed),
            Choice::Keep(Reason::NoGain)
        );

        // The same objective and faster does win when no gain is required,
        // so the tie above is decided by keep and not by the objective alone.
        let faster = ByKey {
            keep: same,
            keyed: vec![(Key::BlockAds, score(0.95, 100.0, 4.0, 50.0))],
        };
        assert!(matches!(
            run(&faster, &Fixture::observed(), &no_gain_needed),
            Choice::Apply { .. }
        ));
    }

    #[test]
    fn nan_scores_abstain() {
        let fixture = Fixture::observed();
        let gate = Gate::default();
        let nan_keep = ByKey {
            keep: score(f32::NAN, 900.0, 4.0, 50.0),
            keyed: vec![(Key::BlockAds, score(0.99, 800.0, 1.0, 50.0))],
        };
        assert_eq!(
            run(&nan_keep, &fixture, &gate),
            Choice::Keep(Reason::NoEvidence)
        );

        let nan_candidate = ByKey {
            keep: score(0.95, 900.0, 4.0, 50.0),
            keyed: vec![
                (Key::BlockAds, score(0.99, 800.0, 1.0, 50.0)),
                (Key::FullResources, score(0.99, 800.0, f32::NAN, 50.0)),
            ],
        };
        assert_eq!(
            run(&nan_candidate, &fixture, &gate),
            Choice::Keep(Reason::NoEvidence)
        );
    }

    #[test]
    fn a_pinned_request_is_never_edited() {
        let gate = Gate::default();

        let mut mode = Fixture::observed();
        mode.caller.request = Some(RequestMode::Smart);
        let mut pool = Fixture::observed();
        pool.caller.proxy = Some(ProxyPool::Isp);
        let mut country = Fixture::observed();
        country.caller.country_code = Country::new("de");

        for fixture in [mode, pool, country] {
            assert_eq!(
                run(&cheaper_block(), &fixture, &gate),
                Choice::Keep(Reason::Pinned)
            );
        }
        // The same scorer edits the unpinned request, so the guard is what
        // kept these.
        assert!(matches!(
            run(&cheaper_block(), &Fixture::observed(), &gate),
            Choice::Apply { .. }
        ));
    }

    #[test]
    fn a_candidate_dearer_than_the_gate_is_passed_over() {
        let fixture = Fixture::observed();
        let browser = ByKey {
            keep: score(0.95, 900.0, 4.0, 50.0),
            keyed: vec![(Key::Request, score(0.99, 800.0, 1.0, 50.0))],
        };
        let strict = Gate {
            max_multiplier: 1.5,
            ..Gate::default()
        };
        // Only http survives the gate's cap, and the scorer prices it the
        // same as browser, so it is applied.
        let Choice::Apply { edits, .. } = run(&browser, &fixture, &strict) else {
            panic!("expected the plain fetch");
        };
        assert_eq!(
            edits.edits()[0],
            crate::edit::Edit::set(Key::Request, crate::edit::Value::Mode(RequestMode::Http))
        );
    }
}
