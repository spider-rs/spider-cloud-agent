import numpy as np
import pytest

from spider_optimize_train import calibrate
from spider_optimize_train.evaluate import auroc, mae, pr_auc


def test_metrics_against_hand_computed_cases():
    p = [0.1, 0.4, 0.35, 0.8]
    y = [0, 0, 1, 1]
    # Brier: (0.01 + 0.16 + 0.4225 + 0.04) / 4
    assert calibrate.brier(p, y) == pytest.approx(0.158125)
    # Pairs (pos, neg): (0.35, 0.1) win, (0.35, 0.4) lose, (0.8, *) win twice: 3 of 4.
    assert auroc(p, y) == pytest.approx(0.75)
    assert auroc([0.5, 0.5], [0, 1]) == pytest.approx(0.5)
    assert np.isnan(auroc([0.2, 0.3], [1, 1]))
    # Ranked 0.8 (hit), 0.4 (miss), 0.35 (hit), 0.1: AP = 0.5 * 1 + 0.5 * 2/3.
    assert pr_auc(p, y) == pytest.approx(0.5 + 1.0 / 3.0)
    # ECE, 10 bins: bin 1 {0.1}: |0.1 - 0|; bin 3 {0.35}: |0.35 - 1|; bin 4 {0.4}:
    # |0.4 - 0|; bin 8 {0.8}: |0.8 - 1|. Each weighted 1/4.
    assert calibrate.ece(p, y) == pytest.approx((0.1 + 0.65 + 0.4 + 0.2) / 4)
    assert calibrate.ece([1.0, 0.95], [1, 1]) == pytest.approx(0.025)
    assert mae([1, 2, 3], [2, 2, 5]) == pytest.approx(1.0)


def test_pav_pools_adjacent_violators():
    lo, hi, values = calibrate.pav([1, 2, 3, 4, 5], [1, 3, 2, 2, 5])
    # 3, 2, 2 pool to 7/3.
    assert list(values) == pytest.approx([1, 7 / 3, 5])
    assert list(lo) == [1, 2, 5]
    assert list(hi) == [1, 4, 5]
    _, _, weighted = calibrate.pav([1, 2], [1.0, 0.0], [3.0, 1.0])
    assert list(weighted) == pytest.approx([0.75])


def test_platt_recovers_a_known_logistic():
    rng = np.random.default_rng(5)
    z = rng.normal(0, 2, 20_000)
    y = (rng.random(len(z)) < calibrate.sigmoid(0.5 * z - 1.0)).astype(float)
    a, b = calibrate.platt(z, y)
    assert a == pytest.approx(0.5, abs=0.05)
    assert b == pytest.approx(-1.0, abs=0.08)


def miscalibrated(rng, n):
    truth = rng.uniform(0.02, 0.98, n)
    y = (rng.random(n) < truth).astype(float)
    # The model is overconfident: its logit is three times the true one.
    return calibrate.logit(truth) * 3.0, y


def test_isotonic_reduces_ece():
    rng = np.random.default_rng(9)
    z_fit, y_fit = miscalibrated(rng, 4000)
    z_new, y_new = miscalibrated(rng, 4000)
    cal = calibrate.fit(z_fit, y_fit, candidate_rows=4000)
    assert cal.kind == calibrate.ISOTONIC
    before = calibrate.ece(calibrate.sigmoid(z_new), y_new)
    after = calibrate.ece(cal.apply(z_new), y_new)
    assert before > 0.08
    assert after < before / 2
    assert cal.report["ece_after"] < cal.report["ece_before"]
    xs = [k[0] for k in cal.knots]
    ys = [k[1] for k in cal.knots]
    assert all(b > a for a, b in zip(xs, xs[1:], strict=False))
    assert all(b >= a for a, b in zip(ys, ys[1:], strict=False))


def test_isotonic_needs_500_candidate_rows_else_platt():
    rng = np.random.default_rng(2)
    z, y = miscalibrated(rng, 2000)
    assert calibrate.fit(z, y, candidate_rows=500).kind == calibrate.ISOTONIC
    assert calibrate.fit(z, y, candidate_rows=499).kind == calibrate.PLATT


def test_isotonic_below_two_knots_falls_back_to_platt():
    # Every row at one logit with one label pools into a single knot.
    z = np.zeros(600)
    y = np.ones(600)
    assert len(calibrate.isotonic_knots(z, y)) == 1
    cal = calibrate.fit(z, y, candidate_rows=600)
    assert cal.kind == calibrate.PLATT
    assert cal.report["method"] == "platt"
    assert len(cal.params) == 2
