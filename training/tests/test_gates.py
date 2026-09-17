import numpy as np
import pytest

from spider_optimize_train import gates, synth
from spider_optimize_train.dataset import pairs_of


def window_pairs(trained, choose):
    """Every test-window pair, with the policy applying the candidate where `choose`
    says so."""
    baselines, chosen, applied = [], [], []
    for arms in pairs_of(trained.split.test).values():
        base = next(a for a in arms if a["arm"] == "baseline")
        cand = next(a for a in arms if a["arm"] != "baseline")
        did = choose(cand)
        baselines.append(base)
        chosen.append(cand if did else base)
        applied.append(did)
    return baselines, chosen, applied


def is_edit(row, wire):
    return bool(row["edit"]) and row["edit"]["key"] == synth.standard_edit(wire).key


def test_gates_fail_on_a_planted_regression(trained, planted):
    res, style = planted["residential"], planted["stylesheets"]

    def flipped_residential(row):
        return is_edit(row, "proxy") and synth.site_of(row).status == res["status"]

    def breaking_stylesheet(row):
        return is_edit(row, style["wire"]) and row["ext"] == style["breaks_ext"]

    baselines, chosen, applied = window_pairs(
        trained, lambda r: flipped_residential(r) or breaking_stylesheet(r)
    )
    assert len(baselines) >= gates.MIN_PAIRS
    result = gates.run(gates.outcomes(baselines, chosen, applied, trained.tau))
    assert result.status == "fail"
    assert "credits per correct result" in result.failed()
    assert "content_ok delta" in result.failed()

    # The residential arms bought no success after the flip and cost the planted factor.
    idx = [i for i, c in enumerate(chosen) if applied[i] and flipped_residential(c)]
    assert len(idx) >= 20
    gained = np.mean([chosen[i]["success"] for i in idx]) - np.mean(
        [baselines[i]["success"] for i in idx]
    )
    assert gained == pytest.approx(res["success_when_flipped"] - res["success_from"], abs=0.1)
    factor = np.mean([chosen[i]["credits"] / baselines[i]["credits"] for i in idx])
    assert factor == pytest.approx(res["credit_factor"], rel=0.1)


def outcome(pair, cheaper=1.0, lose=False, broken=False, slower=1.0):
    return gates.PairOutcome(
        pair=pair, base_success=True, base_credits=1.0, base_millis=1000.0 + pair % 97,
        arm_success=not lose, arm_content_ok=not broken, arm_credits=cheaper,
        arm_millis=(1000.0 + pair % 97) * slower, applied=cheaper != 1.0 or lose or broken,
    )


def test_gates_report_insufficient_below_min_pairs():
    clean = [outcome(i, cheaper=0.5) for i in range(gates.MIN_PAIRS - 1)]
    result = gates.run(clean)
    assert result.status == "insufficient" and not result.passed
    assert result.checks == []
    result = gates.run(clean + [outcome(gates.MIN_PAIRS, cheaper=0.5)])
    assert result.status == "pass"
    assert "fewer than 300 pairs" in gates.render(gates.run(clean), True, "mlp")


def test_each_check_fails_on_its_own_regression():
    n = 600
    lose = [outcome(i, cheaper=0.5, lose=i % 20 == 0) for i in range(n)]
    assert gates.run(lose).failed() == ["success delta"]
    broken = [outcome(i, cheaper=0.5, broken=i % 20 == 0) for i in range(n)]
    assert gates.run(broken).failed() == ["content_ok delta"]
    dear = [outcome(i, cheaper=1.02) for i in range(n)]
    assert gates.run(dear).failed() == ["credits per correct result"]
    slow = [outcome(i, cheaper=0.5, slower=1.15) for i in range(n)]
    assert gates.run(slow).failed() == ["p50 millis"]


def test_rule_of_three_bounds_zero_failures():
    assert gates.rule_of_three(300) == pytest.approx(0.01)
    # The exact 95 percent bound for 0 of n solves (1 - r)^n = 0.05.
    n = 300
    exact = 1 - 0.05 ** (1 / n)
    assert gates.rule_of_three(n) == pytest.approx(exact, rel=0.01)
    clean = [outcome(i, cheaper=0.5) for i in range(n)]
    text = gates.render(gates.run(clean), True, "gbdt")
    assert f"None of the {n} applied pairs regressed" in text


def test_zero_applied_edits_is_insufficient_not_a_cost_failure():
    # The policy kept the request on every pair: its arm is the baseline itself.
    same = [outcome(i) for i in range(600)]
    assert all(not p.applied for p in same)
    result = gates.run(same)
    assert result.status == "insufficient" and not result.passed
    assert result.checks == [] and result.applied == 0
    assert "credits per correct result" not in result.failed()
    text = gates.render(result, True, "mlp")
    assert "applied no edit" in text and "nothing to compare" in text
    assert "the gate fails" in text


def test_identical_policy_passes_the_paired_cost_check():
    # An edit applied on every pair whose outcome equals the baseline's exactly.
    n = 600
    same = []
    for i in range(n):
        millis = 1000.0 + i % 97
        credits = 0.5 + (i % 13) / 10
        same.append(gates.PairOutcome(
            pair=i, base_success=i % 7 != 0, base_credits=credits, base_millis=millis,
            arm_success=i % 7 != 0, arm_content_ok=True, arm_credits=credits,
            arm_millis=millis, applied=True,
        ))
    result = gates.run(same)
    assert result.status == "pass", result.failed()
    cost = next(c for c in result.checks if c.name == "credits per correct result")
    assert cost.value == 0.0 and cost.bound == 0.0 and cost.limit == 0.0


def test_a_dearer_policy_fails_the_paired_cost_check():
    n = 600
    dear = [outcome(i, cheaper=1.01) for i in range(n)]
    result = gates.run(dear)
    assert result.failed() == ["credits per correct result"]
    cost = next(c for c in result.checks if c.name == "credits per correct result")
    assert cost.value == pytest.approx(0.01)
    assert cost.bound == pytest.approx(0.01) and cost.limit == 0.0
    # Cheaper by the same margin passes: the bound sits at -0.01.
    cheap = gates.run([outcome(i, cheaper=0.99) for i in range(n)])
    assert cheap.status == "pass"
