"""The chronological test window is read by the gate and by nothing before it.

Train, calibrate and choose thresholds on the shared 4000 pair corpus; then rewrite
every row of the test window (labels, content scalars, cost, time) and do it all again
with the same seeds. Everything the gate does not read must come out byte for byte the
same, and the gate itself must not, or the check would be vacuous.
"""

import copy

import numpy as np

from spider_optimize_train import compare, labels, splits
from spider_optimize_train import gates as gt
from spider_optimize_train import thresholds as th
from spider_optimize_train.models import train

SEED = 7


def rewrite(row: dict) -> None:
    """Flip the success label, judge the content the other way where it can be judged,
    and move the cost and time. A repeat pair's jaccard moves too, which is what tau
    is chosen from."""
    row["success"] = not row["success"]
    row["status"] = "ok" if row["success"] else "server_error"
    if row["success"]:
        was_ok = row.get("shingle_jaccard") is not None and row["shingle_jaccard"] >= 0.9
        row["shingle_jaccard"] = 0.2 if was_ok else 0.99
        row["byte_ratio"] = 1.0
        if row["need"] == "fields":
            row["fields_ok"] = not bool(row.get("fields_ok"))
        row["bytes"] = max(int(row["bytes"]), 1) * 3
    else:
        row["shingle_jaccard"] = None
        row["byte_ratio"] = None
        row["bytes"] = 0
    row["content_ok"] = None if row["arm"] == "baseline" else labels.content_ok(row)
    row["credits"] = round(float(row["credits"]) * 2.5 + 0.1, 5)
    row["millis"] = int(row["millis"] * 1.5) + 7


def fit(rows):
    result = train(rows, model="both", split="chronological", seed=SEED)
    chosen = {}
    for kind, model in result.models.items():
        scored = th.score(model, result.calibrations[kind], result.windows["calibrate"],
                          result.tau)
        chosen[kind] = th.choose(scored, result.windows["train"], r_max=0.05, min_covered=30,
                                 min_sites=20, resamples=200, seed=SEED)
    return result, chosen


def test_the_test_window_is_never_read_before_the_gate(corpus_rows):
    rows = copy.deepcopy(corpus_rows)
    test_pairs = {r["pair"] for r in splits.chronological(rows).test}
    flipped = copy.deepcopy(rows)
    touched = 0
    for row in flipped:
        if row["pair"] in test_pairs:
            rewrite(row)
            touched += 1
    assert touched > 200
    # The rewrite reached the repeat pairs, whose jaccard is what tau is read from.
    repeats_before = labels.repeat_jaccards([r for r in rows if r["pair"] in test_pairs])
    repeats_after = labels.repeat_jaccards([r for r in flipped if r["pair"] in test_pairs])
    assert repeats_before and repeats_before != repeats_after
    assert [r for r in rows if r["pair"] not in test_pairs] == [
        r for r in flipped if r["pair"] not in test_pairs
    ]

    a, chosen_a = fit(rows)
    b, chosen_b = fit(flipped)

    assert a.tau == b.tau and a.tau_note == b.tau_note
    probe = a.windows["calibrate"].X[:64]
    for kind in a.models:
        for head_a, head_b in zip(a.models[kind].predict(probe), b.models[kind].predict(probe),
                                  strict=True):
            assert np.asarray(head_a).tobytes() == np.asarray(head_b).tobytes(), kind
        assert a.calibrations[kind].to_dict() == b.calibrations[kind].to_dict(), kind
        assert chosen_a[kind].to_dict() == chosen_b[kind].to_dict(), kind

    # Not vacuous: the same model, calibration and floors read on the two test windows
    # give a different gate and a different comparison.
    for kind in a.models:
        model, cal, floors = a.models[kind], a.calibrations[kind], chosen_a[kind]
        on_a = compare.gate_pairs(model, cal, a.windows["test"], floors, a.tau)
        on_b = compare.gate_pairs(model, cal, b.windows["test"], floors, a.tau)
        gate_a, gate_b = gt.run(on_a, 200, 0), gt.run(on_b, 200, 0)
        assert gate_a.applied > 0 and gate_a.checks, gate_a
        assert [(c.name, c.value) for c in gate_a.checks] != [
            (c.name, c.value) for c in gate_b.checks
        ], kind
        assert compare.from_outcomes(on_a, 100) != compare.from_outcomes(on_b, 100), kind
