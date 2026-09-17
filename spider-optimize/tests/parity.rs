//! Numeric interchange contract with the Python trainer. The fixtures are the
//! trainer's exports, copied from `training/fixtures/golden/`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use serde::Deserialize;
use spider_optimize::{Compact, ModelKind};

#[derive(Deserialize)]
struct Expected {
    p_success: Option<f32>,
    latency_ms: f32,
    credits: f32,
    support: f32,
}
#[derive(Deserialize)]
struct Case {
    base: Vec<Option<f32>>,
    edit: Vec<Option<f32>>,
    cell: u32,
    expect: Expected,
}
fn parity(bytes: &[u8], golden: &str, kind: ModelKind) {
    let model = Compact::from_bytes(bytes).unwrap();
    assert_eq!(model.kind(), kind);
    let cases: Vec<Case> = serde_json::from_str(golden).unwrap();
    assert!(
        cases.len() >= 32,
        "parity corpus must contain at least 32 cases"
    );
    // The trainer writes a non-finite slot as null, and that case must abstain.
    assert!(cases.iter().any(|case| {
        case.base.iter().chain(&case.edit).any(Option::is_none) && case.expect.p_success.is_none()
    }));
    for (i, case) in cases.iter().enumerate() {
        assert_eq!(case.base.len(), 152);
        assert_eq!(case.edit.len(), 96);
        let base: Vec<f32> = case.base.iter().map(|x| x.unwrap_or(f32::NAN)).collect();
        let edit: Vec<f32> = case.edit.iter().map(|x| x.unwrap_or(f32::NAN)).collect();
        let actual = model.score_slices(&base, &edit, case.cell);
        match case.expect.p_success {
            Some(value) => assert!(
                (actual.p_success - value).abs() <= 1e-5,
                "case {i}: success"
            ),
            None => assert!(actual.p_success.is_nan(), "case {i}: NaN"),
        }
        for (actual, expected) in [
            (actual.latency_ms, case.expect.latency_ms),
            (actual.credits, case.expect.credits),
            (actual.support, case.expect.support),
        ] {
            assert!(
                (actual - expected).abs() <= 1e-5,
                "case {i}: {actual} != {expected}"
            );
        }
    }
}
#[test]
fn mlp_parity() {
    parity(
        include_bytes!("../assets/spider-optimize-v1.bin"),
        include_str!("fixtures/golden-v1.json"),
        ModelKind::Mlp,
    );
}
#[test]
fn fixtures_are_the_trainer_exports_byte_for_byte() {
    assert_eq!(
        include_bytes!("../assets/spider-optimize-v1.bin"),
        include_bytes!("../../training/fixtures/golden/synth-mlp.bin")
    );
    assert_eq!(
        include_str!("fixtures/golden-v1.json"),
        include_str!("../../training/fixtures/golden/golden-mlp.json")
    );
    assert_eq!(
        include_bytes!("fixtures/gbdt-v1.bin"),
        include_bytes!("../../training/fixtures/golden/synth-gbdt.bin")
    );
    assert_eq!(
        include_str!("fixtures/golden-gbdt-v1.json"),
        include_str!("../../training/fixtures/golden/golden-gbdt.json")
    );
}
#[test]
fn gbdt_parity() {
    parity(
        include_bytes!("fixtures/gbdt-v1.bin"),
        include_str!("fixtures/golden-gbdt-v1.json"),
        ModelKind::Gbdt,
    );
}
