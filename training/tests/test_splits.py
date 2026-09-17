import random

import numpy as np
import pytest

from spider_optimize_train import splits


def shuffled(rows):
    rows = list(rows)
    random.Random(3).shuffle(rows)
    return rows


def test_chronological_windows_follow_day_order_in_whole_days(corpus_rows, planted):
    split = splits.chronological(shuffled(corpus_rows))
    days = [sorted({r["day"] for r in split.window(name)}) for name in splits.WINDOWS]
    for earlier, later in zip(days, days[1:], strict=False):
        assert max(earlier) < min(later)
    counts = [len(d) for d in days]
    total = planted["days"]
    assert counts == [round(share * total) for share in splits.SHARES]
    assert sum(len(split.window(n)) for n in splits.WINDOWS) == len(corpus_rows)


def test_pairs_never_split_chronologically(corpus_rows):
    split = splits.chronological(shuffled(corpus_rows))
    window_of = {}
    for name in splits.WINDOWS:
        for row in split.window(name):
            assert window_of.setdefault(row["pair"], name) == name


def test_domain_folds_never_split_a_site_or_a_pair(corpus_rows):
    folds = splits.domain_folds(shuffled(corpus_rows), k=5)
    assert len(folds) == 5
    tested = set()
    for fold, split in enumerate(folds):
        test_sites = {r["dk"] for r in split.test}
        assert all(dk % 5 == fold for dk in test_sites)
        rest = split.train + split.tune + split.calibrate
        assert test_sites.isdisjoint({r["dk"] for r in rest})
        window_of = {}
        for name in splits.WINDOWS:
            for row in split.window(name):
                assert window_of.setdefault(row["pair"], name) == name
        inner = len(split.train) / max(len(rest), 1)
        assert inner == pytest.approx(0.8, abs=0.05)
        tested |= test_sites
    assert tested == {r["dk"] for r in corpus_rows}


def test_recency_weight_halves_every_half_life():
    w = splits.recency_weight(np.array([59, 29, -1, 70]), 59, half_life=30)
    assert w == pytest.approx([1.0, 0.5, 0.25, 1.0])
