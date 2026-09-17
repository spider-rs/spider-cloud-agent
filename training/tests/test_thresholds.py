import math

import numpy as np
import pytest

from spider_optimize_train import schema as sch
from spider_optimize_train import synth
from spider_optimize_train import thresholds as th


def planted_risk(n_pairs=20_000, seed=4):
    """Risk that falls with p: a covered row regresses with chance (1 - p) / 5."""
    rng = np.random.default_rng(seed)
    p = rng.uniform(0.5, 1.0, n_pairs)
    regressed = rng.random(n_pairs) < (1.0 - p) / 5.0
    return p, np.ones(n_pairs, dtype=bool), regressed, np.arange(n_pairs)


def test_threshold_meets_risk_bound_with_ucb():
    p, cheaper, regressed, pairs = planted_risk()
    s = th.sweep(p, cheaper, regressed, pairs, r_max=0.01, min_covered=200, resamples=1000)
    assert not math.isnan(s.threshold)
    at = s.at(s.threshold)
    assert s.risk_ucb[at] <= 0.01
    assert s.covered[at] >= 200
    assert s.risk_ucb[at] >= s.risk[at]
    # Every lower floor fails the bound, so this is the smallest that meets it.
    for below in range(at):
        assert s.risk_ucb[below] > 0.01 or s.covered[below] < 200
    # The planted risk of rows above t averages (1 - t) / 10, which is 0.01 at t = 0.9;
    # the upper bound pushes the floor above that.
    assert 0.9 <= s.threshold < 1.0


def test_a_sweep_with_no_safe_floor_abstains():
    rng = np.random.default_rng(1)
    n = 3000
    p = rng.uniform(0.5, 1.0, n)
    regressed = rng.random(n) < 0.05
    s = th.sweep(p, np.ones(n, bool), regressed, np.arange(n), r_max=0.01, min_covered=200)
    assert math.isnan(s.threshold)
    # And too few rows abstains even with no regression at all.
    s = th.sweep(p[:150], np.ones(150, bool), np.zeros(150, bool), np.arange(150))
    assert math.isnan(s.threshold)


def test_a_row_that_is_not_cheaper_is_never_covered():
    p, _, regressed, pairs = planted_risk(1000)
    s = th.sweep(p, np.zeros(1000, bool), regressed, pairs, min_covered=1)
    assert s.covered.max() == 0 and math.isnan(s.threshold)


def test_bootstrap_resamples_whole_pairs():
    pairs = np.repeat(np.arange(50), 4)
    w = th.bootstrap_pair_weights(pairs, 300, np.random.default_rng(0))
    assert w.shape == (300, 200)
    grouped = w.reshape(300, 50, 4)
    assert np.all(grouped == grouped[:, :, :1])
    assert np.allclose(w.sum(axis=1), 200)


def test_clustered_regressions_widen_the_bound():
    """Ten arms per pair that regress together are ten rows but one piece of evidence.
    Drawn by pair, the bound on 40 pairs with one bad pair must stay near what 40
    independent trials give, well above what 400 would."""
    pairs = np.repeat(np.arange(40), 10)
    regressed = pairs == 0
    p = np.full(400, 0.95)
    s = th.sweep(p, np.ones(400, bool), regressed, pairs, r_max=1.0, min_covered=1,
                 resamples=2000)
    at = s.at(0.95)
    assert s.risk[at] == pytest.approx(0.025)
    # 95th percentile of Binomial(40, 1/40) / 40 is 3/40; of Binomial(400, 1/40) / 400
    # it is 16/400.
    assert s.risk_ucb[at] >= 0.075 - 1e-9


def test_support_table_needs_fifty_distinct_sites(trained):
    train = trained.windows["train"]
    support = th.support_table(train)
    sites = {}
    for cell, dk in zip(train.cell, train.dk, strict=True):
        sites.setdefault(int(cell), set()).add(int(dk))
    assert support == sorted(c for c, s in sites.items() if len(s) >= th.MIN_SITES)
    assert support, "the common cells must pass"
    assert any(len(s) < th.MIN_SITES for s in sites.values()), "the rare cells must not"


def open_thresholds(floor: float, cells) -> th.Thresholds:
    per_code = [float("nan")] + [floor] * (sch.load().edit_codes - 1)
    return th.Thresholds(per_code, floor, {}, sorted(int(c) for c in set(cells)))


def test_an_unsupported_cell_is_never_applied(trained):
    scored = th.score(trained.models["mlp"], trained.calibrations["mlp"],
                      trained.windows["test"], trained.tau)
    everything = open_thresholds(0.0, scored.arrays.cell)
    applied = th.applies(scored, everything)
    assert applied.any()
    first = int(scored.arrays.cell[np.flatnonzero(applied)[0]])
    without = th.Thresholds(everything.per_code, 0.0, {},
                            [c for c in everything.support if c != first])
    again = th.applies(scored, without)
    assert not again[scored.arrays.cell == first].any()
    abstain = th.Thresholds([float("nan")] * len(everything.per_code), float("nan"), {},
                            everything.support)
    assert not th.applies(scored, abstain).any()


def held_out_scores(trained, kind):
    import copy

    from spider_optimize_train import features as feat

    rows = trained.split.calibrate + trained.split.test
    scored = th.score(trained.models[kind], trained.calibrations[kind],
                      feat.build(rows, trained.tau), trained.tau)
    return scored, copy.deepcopy(rows)


@pytest.mark.parametrize("kind", ["mlp", "gbdt"])
def test_stylesheet_break_is_refused_by_the_gate(trained, planted, kind):
    style = planted["stylesheets"]
    scored, rows = held_out_scores(trained, kind)
    a = scored.arrays
    key = synth.standard_edit(style["wire"]).key
    is_style = np.array([bool(r["edit"]) and r["edit"]["key"] == key for r in rows])
    breaks = is_style & (a.ext == style["breaks_ext"])
    elsewhere = is_style & (a.ext != style["breaks_ext"]) & (a.success > 0.5)
    assert breaks.sum() >= 30 and elsewhere.sum() >= 30
    # The planted break is in the labels: every succeeded arm there has jaccard under
    # the planted ceiling.
    jaccards = [rows[i]["shingle_jaccard"] for i in np.flatnonzero(breaks)
                if rows[i]["shingle_jaccard"] is not None]
    assert jaccards and max(jaccards) < style["jaccard_below"]
    # With a floor of one half on every code and every cell supported, the other cells
    # still get the edit and the breaking cell never does.
    gate = open_thresholds(0.5, a.cell)
    applied = th.applies(scored, gate)
    assert not applied[breaks].any()
    assert applied[elsewhere].any()


@pytest.mark.parametrize("kind", ["mlp", "gbdt"])
def test_first_party_append_is_never_applied(trained, kind):
    scored, rows = held_out_scores(trained, kind)
    key = synth.standard_edit("network_blacklist").key
    first_party = np.array([bool(r["edit"]) and r["edit"]["key"] == key
                            and not r["edit"]["ident"]["third"] for r in rows])
    third_party = np.array([bool(r["edit"]) and r["edit"]["key"] == key
                            and r["edit"]["ident"]["third"] for r in rows])
    assert first_party.sum() >= 20
    applied = th.applies(scored, open_thresholds(0.5, scored.arrays.cell))
    assert not applied[first_party].any()
    assert applied[third_party].any()
