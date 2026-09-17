//! Score one input with no model, the path the default build takes.
//!
//! The twin of `score_embedded`: `scripts/measure-optimize.sh` builds both and
//! reports the difference as what the reader and the artifact cost.

use spider_optimize::{EditFeatures, Input, NoModel, Scorer};
use spider_route::FeatureVector;

fn main() {
    let input = Input {
        base: FeatureVector::zeroed(),
        edit: EditFeatures::zeroed(),
    };
    let score = NoModel.score(&input);
    println!("{}", score.p_success);
}
