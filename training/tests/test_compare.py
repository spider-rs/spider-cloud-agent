"""`compare.py` on hand-built pairs, so every expected number is written down here."""

import math

import pytest

from spider_optimize_train import compare, gates


def pair(n, base_success, arm_success, applied, base_credits=1.0, arm_credits=1.0,
         content_ok=True):
    return gates.PairOutcome(
        pair=n, base_success=base_success, base_credits=base_credits, base_millis=1000.0,
        arm_success=arm_success, arm_content_ok=content_ok, arm_credits=arm_credits,
        arm_millis=1000.0, applied=applied,
    )


def six_pairs():
    return [
        # A harmful override: the baseline succeeded, the policy's arm did not.
        pair(0, True, False, True, base_credits=1.0, arm_credits=2.0),
        # Two helpful overrides: the baseline failed, the policy's arm is correct.
        pair(1, False, True, True, base_credits=1.0, arm_credits=1.5),
        pair(2, False, True, True, base_credits=3.0, arm_credits=1.0),
        # Two kept pairs: the policy's arm is the baseline arm itself.
        pair(3, True, True, False),
        pair(4, False, False, False),
        # A harmful override by content: both succeeded, the arm came back broken.
        pair(5, True, True, True, base_credits=1.0, arm_credits=0.5, content_ok=False),
    ]


def test_comparison_counts_every_pair_the_way_it_was_built():
    c = compare.from_outcomes(six_pairs(), resamples=500, seed=3)
    assert c.pairs == 6
    assert c.overrides == 4
    assert c.coverage == pytest.approx(4 / 6)
    # Policy arms: F T T T F T; baseline arms: T F F T F T.
    assert c.policy_success == pytest.approx(4 / 6)
    assert c.baseline_success == pytest.approx(3 / 6)
    assert c.success_delta == pytest.approx(1 / 6)
    assert c.policy_content_ok == pytest.approx(5 / 6)
    assert c.baseline_content_ok == 1.0
    # Policy credits 2 + 1.5 + 1 + 1 + 1 + 0.5 over three correct arms (pairs 1, 2, 3;
    # pair 5 succeeded but its content is broken). Baseline credits 1 + 1 + 3 + 1 + 1 + 1
    # over three successes.
    assert c.policy_credits_per_correct == pytest.approx(7.0 / 3)
    assert c.baseline_credits_per_correct == pytest.approx(8.0 / 3)
    assert c.credits_per_correct_delta == pytest.approx(-1.0 / 3)
    assert c.harmful == 2 and c.harmful_rate == pytest.approx(0.5)
    assert c.helpful == 2 and c.helpful_rate == pytest.approx(0.5)
    assert c.harmful_rate <= c.harmful_rate_ucb <= 1.0


def test_no_override_means_no_rates_and_full_agreement():
    kept = [pair(n, n % 2 == 0, n % 2 == 0, False) for n in range(10)]
    c = compare.from_outcomes(kept, resamples=100)
    assert c.overrides == 0 and c.coverage == 0.0
    assert c.harmful == 0 and c.helpful == 0
    assert math.isnan(c.harmful_rate) and math.isnan(c.harmful_rate_ucb)
    assert c.success_delta == 0.0 and c.credits_per_correct_delta == 0.0
    assert c.policy_success == c.baseline_success == 0.5


def test_render_writes_one_table_with_the_counts():
    c = compare.from_outcomes(six_pairs(), resamples=100)
    text = compare.render(c, synthetic=True, kind="mlp")
    assert text.startswith(compare.HEADING + "\n\n")
    assert sum(line.startswith("|---") for line in text.splitlines()) == 1
    assert "| overrides | 4 | 0 | coverage 0.6667 |" in text
    assert "| harmful overrides | 2 | 0 | rate 0.5000, upper bound " in text
    assert "| helpful overrides | 2 | 0 | rate 0.5000 |" in text
    assert "| success rate | 0.6667 | 0.5000 | delta 0.1667 |" in text
    assert "| credits per correct result | 2.3333 | 2.6667 | delta -0.3333 |" in text
    assert "improve" not in text.lower()
    assert "planted" in text
    assert "planted" not in compare.render(c, synthetic=False)
