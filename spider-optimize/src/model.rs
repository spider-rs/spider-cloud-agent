//! The scoring boundary.
//!
//! A [`Scorer`] estimates, for one candidate, how likely the request is to
//! succeed, how long it takes and what it costs, and how much evidence stands
//! behind that estimate. It answers from the [`Input`] alone, with no input
//! or output of any kind.
//!
//! [`NoModel`] is what ships until weights exist. It abstains on every field,
//! and the gate reads that as a reason to keep the request unchanged. The
//! reader for a trained artifact lands in a later change, behind the
//! `embedded-model` feature, as one more implementation of this trait.

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
