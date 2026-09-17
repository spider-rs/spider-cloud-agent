import numpy as np
import pytest

from spider_optimize_train import features as feat
from spider_optimize_train import synth
from spider_optimize_train.export import artifact_size
from spider_optimize_train.models import class_weights, sample_weights
from spider_optimize_train.models.lightgbm_head import eval_tables
from spider_optimize_train.models.mlp import Net


@pytest.mark.parametrize("kind", ["bce", "huber", "l2"])
def test_gradient_check(kind):
    rng = np.random.default_rng(11)
    net = Net.init(kind, [6, 5, 4, 1], rng)
    X = rng.normal(size=(9, 6))
    y = (rng.random(9) > 0.5).astype(float) if kind == "bce" else rng.normal(0, 2, 9)
    w = rng.uniform(0.5, 2.0, 9)
    _, gw, gb = net.loss_and_grads(X, y, w)
    eps = 1e-6
    checked = 0
    for params, grads in ((net.weights, gw), (net.biases, gb)):
        for p, g in zip(params, grads, strict=True):
            flat, gflat = p.reshape(-1), g.reshape(-1)
            for i in range(flat.size):
                old = flat[i]
                flat[i] = old + eps
                up = net.loss_and_grads(X, y, w)[0]
                flat[i] = old - eps
                down = net.loss_and_grads(X, y, w)[0]
                flat[i] = old
                assert gflat[i] == pytest.approx((up - down) / (2 * eps), abs=1e-6, rel=1e-4)
                checked += 1
    assert checked == 6 * 5 + 5 * 4 + 4 + 5 + 4 + 1


def site_rows(rows, test):
    return [r for r in rows if r["arm"] == "baseline" and test(synth.site_of(r))]


def edited(rows, edit):
    return np.stack([feat.row_vector(synth.with_edit(r, edit)) for r in rows])


def test_planted_wait_effect_is_recovered_by_both_models(trained, planted):
    wait = planted["wait"]
    held_out = trained.split.calibrate + trained.split.test
    rows = site_rows(held_out, lambda s: s.status == "ok" and s.ext == wait["ext"]
                     and s.mem == wait["mem"])
    assert len(rows) > 100
    keep = edited(rows, None)
    with_wait = edited(rows, synth.standard_edit("wait_for"))
    lift = wait["success_to"] - wait["success_from"]
    for kind, model in trained.models.items():
        cal = trained.calibrations[kind]
        base, cand = model.predict(keep), model.predict(with_wait)
        delta_p = cal.apply(cand[0]).mean() - cal.apply(base[0]).mean()
        assert delta_p == pytest.approx(lift, abs=0.12), kind
        added = np.expm1(cand[1]).mean() - np.expm1(base[1]).mean()
        assert added == pytest.approx(wait["add_millis"], rel=0.2), kind
        factor = np.expm1(cand[2]).mean() / np.expm1(base[2]).mean()
        assert factor == pytest.approx(wait["credit_factor"], rel=0.1), kind


def test_planted_browser_effect_on_empty_sites_is_recovered(trained, planted):
    browser = planted["browser"]
    held_out = trained.split.calibrate + trained.split.test
    rows = site_rows(held_out, lambda s: s.status == browser["status"])
    assert len(rows) > 30
    keep = edited(rows, None)
    cand = edited(rows, synth.standard_edit("request"))
    for kind, model in trained.models.items():
        cal = trained.calibrations[kind]
        p_keep = cal.apply(model.predict(keep)[0]).mean()
        p_cand = cal.apply(model.predict(cand)[0]).mean()
        assert p_keep == pytest.approx(browser["success_from"], abs=0.12), kind
        assert p_cand == pytest.approx(browser["success_to"], abs=0.15), kind


def test_the_residential_flip_lands_after_training(trained, planted):
    """Trained on days before the flip, both models still believe residential helps a
    blocked site; the test window, after the flip, shows it does not."""
    res = planted["residential"]
    flip_day = planted["days"] - res["flipped_last_days"]
    assert max(r["day"] for r in trained.split.train) < flip_day
    key = synth.standard_edit("proxy").key
    test_rows = [r for r in trained.split.test if r["edit"] and r["edit"]["key"] == key
                 and synth.site_of(r).status == res["status"]]
    assert len(test_rows) >= 20
    observed = np.mean([r["success"] for r in test_rows])
    assert observed == pytest.approx(res["success_when_flipped"], abs=0.12)
    X = np.stack([feat.row_vector(r) for r in test_rows])
    for kind, model in trained.models.items():
        believed = trained.calibrations[kind].apply(model.predict(X)[0]).mean()
        assert believed > observed + (res["success_to"] - res["success_when_flipped"]) / 2, kind


def test_class_weights_balance_the_classes(trained):
    y = np.array([1, 1, 1, 0])
    neg, pos = class_weights(y)
    assert (neg, pos) == pytest.approx((2.0, 2.0 / 3.0))
    assert neg * 1 == pytest.approx(pos * 3)
    arrays = trained.windows["train"]
    weights = sample_weights(arrays, int(arrays.day.max()), class_weights(arrays.success))[0]
    positives = arrays.success > 0.5
    recency = 0.5 ** ((arrays.day.max() - arrays.day) / 30.0)
    per_class = weights / recency
    assert per_class[positives].sum() == pytest.approx(per_class[~positives].sum())


def test_gbdt_tables_reproduce_lightgbm_raw_scores(trained):
    model = trained.models["gbdt"]
    X = trained.windows["test"].X[:150]
    for head, booster in enumerate(model.boosters):
        raw = booster.predict(X, raw_score=True, num_iteration=booster.best_iteration or None)
        assert np.allclose(model.predict(X)[head], raw, atol=1e-9)
        assert np.allclose(eval_tables(model.tables[head], 0.0, X[:20]), raw[:20], atol=1e-9)


def test_cap_bytes_drops_trailing_trees_round_robin(trained):
    import copy

    model = copy.copy(trained.models["gbdt"])
    model.tables = [list(head) for head in trained.models["gbdt"].tables]
    before = [len(h) for h in model.tables]
    size = artifact_size(model)
    limit = size - 20_000
    dropped = model.cap_bytes(limit)
    after = [len(h) for h in model.tables]
    assert artifact_size(model) <= limit
    assert dropped == sum(before) - sum(after) > 0
    removed = [b - a for b, a in zip(before, after, strict=True)]
    assert max(removed) - min(removed) <= 1
    for head, kept in enumerate(after):
        assert model.tables[head] == trained.models["gbdt"].tables[head][:kept]
