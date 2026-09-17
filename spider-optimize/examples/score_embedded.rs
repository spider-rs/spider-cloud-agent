//! Load the bundled fixture model and score one input.
//!
//! Built and run by `scripts/measure-optimize.sh` to measure what carrying the
//! reader and the artifact costs: binary bytes, peak memory and cold start.
//! It is the twin of `keep_no_model`, which does the same work with no model.

use spider_optimize::{embedded, EDIT_DIM};
use spider_route::FEATURES_USED;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = embedded()?;
    let base = [0.0f32; FEATURES_USED];
    let edit = [0.0f32; EDIT_DIM];
    let score = model.score_slices(&base, &edit, 0);
    println!("{}", score.p_success);
    Ok(())
}
