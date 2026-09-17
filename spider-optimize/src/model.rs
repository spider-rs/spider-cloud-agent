//! The scoring boundary.
//!
//! A [`Scorer`] estimates, for one candidate, how likely the request is to
//! succeed, how long it takes and what it costs, and how much evidence stands
//! behind that estimate. It answers from the [`Input`] alone, with no input
//! or output of any kind.
//!
//! [`NoModel`] abstains on every field. [`Compact`] reads numeric artifacts
//! and performs calibrated FP32 inference without allocation.

use crate::features::Input;
use std::sync::Arc;

/// The version of a set of weights, which moves independently of the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelVersion(pub u16);

/// A scorer's estimate for one candidate.
///
/// A NaN in any field means the scorer does not know, and the gate keeps the
/// request as it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    /// The chance the request gets what the caller asked for, zero to one.
    pub p_success: f32,
    /// The expected time for the page, in milliseconds.
    pub latency_ms: f32,
    /// The expected spend, in credits.
    pub credits: f32,
    /// How much evidence the estimate rests on. Zero is none.
    pub support: f32,
}

impl Score {
    /// No estimate at all.
    pub const fn unknown() -> Score {
        Score {
            p_success: f32::NAN,
            latency_ms: f32::NAN,
            credits: f32::NAN,
            support: 0.0,
        }
    }

    /// Whether any field is NaN.
    pub fn has_nan(&self) -> bool {
        self.p_success.is_nan()
            || self.latency_ms.is_nan()
            || self.credits.is_nan()
            || self.support.is_nan()
    }
}

/// Estimates what a candidate edit does.
pub trait Scorer: Send + Sync {
    /// Score one candidate. Must not allocate or reach outside the process.
    fn score(&self, input: &Input) -> Score;

    /// Which weights answered, or `None` when no model is involved.
    fn version(&self) -> Option<ModelVersion>;
}

/// The scorer with no weights. It abstains on everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct NoModel;

impl Scorer for NoModel {
    fn score(&self, _input: &Input) -> Score {
        Score::unknown()
    }

    fn version(&self) -> Option<ModelVersion> {
        None
    }
}

impl<T: Scorer + ?Sized> Scorer for Arc<T> {
    fn score(&self, input: &Input) -> Score {
        (**self).score(input)
    }

    fn version(&self) -> Option<ModelVersion> {
        (**self).version()
    }
}

impl<T: Scorer + ?Sized> Scorer for Box<T> {
    fn score(&self, input: &Input) -> Score {
        (**self).score(input)
    }

    fn version(&self) -> Option<ModelVersion> {
        (**self).version()
    }
}

impl<T: Scorer + ?Sized> Scorer for &T {
    fn score(&self, input: &Input) -> Score {
        (**self).score(input)
    }

    fn version(&self) -> Option<ModelVersion> {
        (**self).version()
    }
}

use crate::artifact::{Calibration, Compact, Head, MAX_WIDTH};
use crate::features::EDIT_DIM;
use spider_route::features::FEATURES_USED;

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}
impl Calibration {
    pub(crate) fn apply(&self, x: f32) -> f32 {
        match self {
            Self::Identity => x,
            Self::Platt(a, b) => sigmoid(a * x + b),
            Self::Isotonic(pairs) => {
                let Some(first) = pairs.first() else {
                    return f32::NAN;
                };
                if x <= first.0 {
                    return first.1;
                }
                for pair in pairs.windows(2) {
                    if let [a, b] = pair {
                        if x <= b.0 {
                            return a.1 + (b.1 - a.1) * ((x - a.0) / (b.0 - a.0));
                        }
                    }
                }
                pairs.last().map_or(f32::NAN, |p| p.1)
            }
        }
    }
}
impl Compact {
    /// Score exactly 152 base slots and 96 edit slots. Invalid lengths abstain.
    /// Non-finite values use zero in dense nets, and the encoded missing branch
    /// in trees; either way success becomes NaN so the gate abstains.
    /// An empty support table means no support restriction.
    pub fn score_slices(&self, base: &[f32], edit: &[f32], cell: u32) -> Score {
        if base.len() != FEATURES_USED || edit.len() != EDIT_DIM {
            return Score::unknown();
        }
        let nonfinite = base.iter().chain(edit).any(|v| !v.is_finite());
        let mut a = [0.0; MAX_WIDTH];
        let mut b = [0.0; MAX_WIDTH];
        let mut outputs = [0.0f32; 3];
        for ((head, calibration), output) in
            self.heads.iter().zip(&self.calibration).zip(&mut outputs)
        {
            let raw = match head {
                Head::Mlp(layers) => {
                    for (dest, src) in a.iter_mut().zip(base.iter().chain(edit)) {
                        *dest = if src.is_finite() { *src } else { 0.0 };
                    }
                    for layer in layers {
                        for ((dest, weights), bias) in b
                            .iter_mut()
                            .take(layer.rows)
                            .zip(layer.weights.chunks_exact(layer.cols))
                            .zip(&layer.bias)
                        {
                            let mut sum = *bias;
                            for (weight, input) in weights.iter().zip(&a) {
                                sum += weight * input;
                            }
                            *dest = match layer.activation {
                                1 => sum.max(0.0),
                                2 => sigmoid(sum),
                                _ => sum,
                            };
                        }
                        (a, b) = (b, a);
                    }
                    a.first().copied().unwrap_or(f32::NAN)
                }
                Head::Gbdt(base_score, trees) => {
                    let mut sum = *base_score;
                    for tree in trees {
                        let mut at = 0;
                        // Validated acyclic trees terminate in at most node count steps.
                        for _ in 0..tree.len() {
                            let Some(node) = tree.get(at) else {
                                return Score::unknown();
                            };
                            if node.left == 65535 && node.right == 65535 {
                                sum += node.value;
                                break;
                            }
                            let feature = usize::from(node.feature & 0x7fff);
                            let x = if feature < FEATURES_USED {
                                base.get(feature)
                            } else {
                                edit.get(feature - FEATURES_USED)
                            }
                            .copied()
                            .unwrap_or(f32::NAN);
                            let left = if x.is_finite() {
                                x <= node.threshold
                            } else {
                                node.feature & 0x8000 != 0
                            };
                            at = usize::from(if left { node.left } else { node.right });
                        }
                    }
                    sum
                }
            };
            *output = calibration.apply(raw);
        }
        let [success, latency, credits] = outputs;
        Score {
            p_success: if nonfinite { f32::NAN } else { success },
            latency_ms: latency.exp_m1(),
            credits: credits.exp_m1(),
            support: if self.support.is_empty() || self.supported(cell) {
                1.0
            } else {
                0.0
            },
        }
    }
    /// Score an input with a known support cell, using only the base slots in use.
    pub fn score_in_cell(&self, input: &Input, cell: u32) -> Score {
        let base = input.base.as_slice();
        let Some(used) = base.get(..FEATURES_USED) else {
            return Score::unknown();
        };
        let mut score = self.score_slices(used, input.edit.as_slice(), cell);
        if base.iter().any(|x| !x.is_finite()) {
            score.p_success = f32::NAN;
        }
        score
    }
}
impl Scorer for Compact {
    /// Without a cell, support is one only when the support table is empty.
    fn score(&self, input: &Input) -> Score {
        let mut score = self.score_in_cell(input, 0);
        score.support = if self.support.is_empty() { 1.0 } else { 0.0 };
        score
    }
    fn version(&self) -> Option<ModelVersion> {
        Some(Compact::version(self))
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

    #[test]
    fn no_model_abstains_through_every_wrapper() {
        let input = Input::default();
        let scorers: [Box<dyn Scorer>; 4] = [
            Box::new(NoModel),
            Box::new(Arc::new(NoModel)),
            Box::new(Box::new(NoModel) as Box<dyn Scorer>),
            Box::new(&NoModel),
        ];
        for scorer in &scorers {
            let score = scorer.score(&input);
            assert!(score.p_success.is_nan() && score.latency_ms.is_nan());
            assert!(score.credits.is_nan());
            assert_eq!(score.support, 0.0);
            assert!(score.has_nan());
            assert_eq!(scorer.version(), None);
        }
    }
}
