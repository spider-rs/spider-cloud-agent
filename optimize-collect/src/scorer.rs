//! A scorer that makes the optimizer pick one edit set and nothing else.
//!
//! A candidate arm has to go out through the optimizer in `ApplyMode::Apply`,
//! so its row carries the features, the descriptor and the multiplier the
//! optimizer itself computed. A [`Scorer`] only sees an [`Input`], never the
//! edit set, so [`ForcedScorer`] recognises its target by the slots of the edit
//! features that describe the edits alone: which keys, which operations, which
//! value buckets, which pair, and the appended identifier's buckets. Keep and
//! every other candidate score the same plain estimate, which no candidate
//! beats, and the target scores cheaper and surer, so the gate applies it.
//!
//! Two different sets can share those slots, for instance a smart fetch with a
//! five second wait and a browser fetch with a two second one. The collector
//! checks the written row against the target afterwards and drops a pair whose
//! candidate arm carried something else.

use spider_cloud_agent::optimize::{EditSet, Scorer};
use spider_cloud_agent::RequestParams;
use spider_cloud_agent::{DeclaredNeed, RouteDecision};
use spider_optimize::features::{EDIT_KEY, MISSING, NEED_BITS, OBS_SHARE};
use spider_optimize::{
    featurize_edit, Candidate, Context, EditDescriptor, EditFeatures, Input, ModelVersion,
    Observation, Score,
};
use url::Url;

/// The slots that depend on the edits and the observation only.
const SIGNATURE: [std::ops::Range<usize>; 2] = [EDIT_KEY..NEED_BITS, OBS_SHARE..MISSING];

/// Favours exactly one edit set.
#[derive(Clone)]
pub struct ForcedScorer {
    target: EditFeatures,
}

impl ForcedScorer {
    /// A scorer for this edit set on this page, as this observation sees it.
    pub fn new(
        url: &Url,
        need: DeclaredNeed,
        edits: &EditSet,
        observation: &Observation,
    ) -> ForcedScorer {
        ForcedScorer {
            target: signature(url, need, edits, observation),
        }
    }

    /// Whether these edit features describe the target.
    pub fn is_target(&self, edit: &[f32]) -> bool {
        let target = self.target.as_slice();
        SIGNATURE
            .iter()
            .all(|range| edit.get(range.clone()) == target.get(range.clone()))
    }
}

impl Scorer for ForcedScorer {
    fn score(&self, input: &Input) -> Score {
        if self.is_target(input.edit.as_slice()) {
            Score {
                p_success: 0.99,
                latency_ms: 100.0,
                credits: 0.01,
                support: 1e6,
            }
        } else {
            Score {
                p_success: 0.95,
                latency_ms: 1_000.0,
                credits: 1.0,
                support: 1e6,
            }
        }
    }

    fn version(&self) -> Option<ModelVersion> {
        Some(ModelVersion(0))
    }
}

/// The edit features of `edits` in a context that differs from the real one
/// only in slots outside [`SIGNATURE`].
fn signature(
    url: &Url,
    need: DeclaredNeed,
    edits: &EditSet,
    observation: &Observation,
) -> EditFeatures {
    let params = RequestParams::default();
    let routed = RouteDecision::default();
    let ctx = Context {
        url,
        need,
        current: &params,
        caller: &params,
        routed: &routed,
        observation,
        multiplier_cap: f32::INFINITY,
    };
    featurize_edit(
        &ctx,
        &Candidate {
            edits: edits.clone(),
            multiplier: 1.0,
        },
    )
}

/// Whether a written row describes the target: a candidate arm whose
/// descriptor and edit features are the target's.
pub fn row_matches(
    scorer: &ForcedScorer,
    row: &serde_json::Value,
    edits: &EditSet,
    observation: &Observation,
) -> bool {
    if row["arm"] != "candidate" {
        return false;
    }
    let Some(want) = EditDescriptor::describe(edits, observation) else {
        return false;
    };
    let edit = &row["edit"];
    let same_descriptor = edit["key"].as_u64() == Some(u64::from(want.key))
        && edit["op"].as_u64() == Some(u64::from(want.op))
        && edit["bucket"].as_u64() == Some(u64::from(want.bucket));
    let feats: Option<Vec<f32>> = row["edit_feats"].as_array().map(|values| {
        values
            .iter()
            .map(|value| value.as_f64().unwrap_or(f64::NAN) as f32)
            .collect()
    });
    same_descriptor && feats.is_some_and(|feats| scorer.is_target(&feats))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use spider_cloud_agent::RequestMode;
    use spider_optimize::{Edit, Key, Value};

    fn set(edits: Vec<Edit>) -> EditSet {
        EditSet::new(edits).unwrap()
    }

    #[test]
    fn only_the_target_scores_favourably() {
        let url = Url::parse("https://example.com/a").unwrap();
        let observation = Observation::cold();
        let browser = set(vec![Edit::set(
            Key::Request,
            Value::Mode(RequestMode::Browser),
        )]);
        let http = set(vec![Edit::set(
            Key::Request,
            Value::Mode(RequestMode::Http),
        )]);
        let scorer = ForcedScorer::new(&url, DeclaredNeed::Markdown, &browser, &observation);

        // A different current request changes the config slots and not the match.
        let current = RequestParams {
            request: Some(RequestMode::Smart),
            ..RequestParams::default()
        };
        let routed = RouteDecision::default();
        let ctx = Context {
            url: &url,
            need: DeclaredNeed::Markdown,
            current: &current,
            caller: &RequestParams::default(),
            routed: &routed,
            observation: &observation,
            multiplier_cap: 8.0,
        };
        let input = |edits: &EditSet| Input {
            base: spider_cloud_agent::featurize(&spider_cloud_agent::RouteInput::new(
                &url,
                DeclaredNeed::Markdown,
            )),
            edit: featurize_edit(
                &ctx,
                &Candidate {
                    edits: edits.clone(),
                    multiplier: 4.0,
                },
            ),
        };
        let favoured = scorer.score(&input(&browser));
        let plain = scorer.score(&input(&http));
        let keep = scorer.score(&input(&EditSet::keep()));
        assert!(favoured.credits < plain.credits && favoured.p_success > plain.p_success);
        assert_eq!(plain, keep);
    }
}
