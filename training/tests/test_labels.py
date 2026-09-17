import pytest

from spider_optimize_train import labels


def arm(**kw):
    row = {
        "arm": "candidate",
        "edit": {"key": 46},
        "need": "markdown",
        "status": "ok",
        "success": True,
        "shingle_jaccard": 0.95,
        "byte_ratio": 0.9,
        "fields_requested": 0,
        "fields_present": 0,
        "fields_ok": None,
    }
    row.update(kw)
    return row


def test_content_ok_is_recomputed_from_the_stored_scalars():
    assert labels.content_ok(arm()) is True
    assert labels.content_ok(arm(shingle_jaccard=0.79)) is False
    assert labels.content_ok(arm(shingle_jaccard=0.79), tau=0.7) is True
    assert labels.content_ok(arm(byte_ratio=0.49)) is False
    assert labels.content_ok(arm(status="empty")) is False
    assert labels.content_ok(arm(shingle_jaccard=None)) is None
    assert labels.content_ok(arm(need="fields", fields_ok=False)) is False
    assert labels.content_ok(arm(need="fields", fields_ok=True)) is True
    assert labels.content_ok(arm(need="links", fields_requested=10, fields_present=9)) is True
    assert labels.content_ok(arm(need="metadata", fields_requested=10, fields_present=8)) is False
    assert labels.content_ok(arm(need="screenshot")) is None
    assert labels.content_ok(arm(need="raw", status="blocked")) is None


def test_regressed_means_the_baseline_had_it_and_the_candidate_lost_it():
    base_ok = arm(arm="baseline", edit=None)
    base_failed = arm(arm="baseline", edit=None, success=False, status="blocked")
    assert labels.regressed(arm(), base_ok) is False
    assert labels.regressed(arm(success=False, status="blocked"), base_ok) is True
    assert labels.regressed(arm(shingle_jaccard=0.3), base_ok) is True
    assert labels.regressed(arm(shingle_jaccard=0.3), base_failed) is True
    assert labels.regressed(arm(success=False, status="blocked"), base_failed) is False
    assert labels.correct(arm(shingle_jaccard=0.3)) is False
    assert labels.correct(arm(shingle_jaccard=None)) is True


def test_choose_tau_lands_on_the_planted_repeat_percentile(corpus_rows, planted):
    a, b = planted["repeats"]["beta"]
    assert b == 1, "the closed form below holds for Beta(a, 1)"
    # Beta(a, 1) has CDF x^a, so its 5th percentile is 0.05^(1/a).
    expected = 0.05 ** (1.0 / a)
    tau, note = labels.choose_tau(corpus_rows)
    assert tau == pytest.approx(expected, abs=0.03)
    assert "5th percentile" in note


def test_choose_tau_keeps_the_default_with_few_repeats(corpus_rows):
    repeats = [r for r in corpus_rows if r["arm"] != "baseline" and r["edit"] is None]
    tau, note = labels.choose_tau(repeats[: labels.MIN_REPEATS - 1])
    assert tau == labels.DEFAULT_TAU
    assert "kept the default" in note
