"""The selected policy against the heuristic baseline, pair by pair on the test window.

Every test pair ran the baseline arm and one candidate. The policy either keeps the
request, and its arm is the baseline arm, or applies the candidate, and its arm is the
candidate arm; a pair on which it applied is an override. The comparison reads both
arms of every pair and reports what the overrides bought and what they cost:

- correctness: the success rate and the content_ok rate of the policy's arms and of the
  baseline arms. A kept pair counts the same row on both sides.
- credits per correct result: total credits over correct arms (a success whose content
  was not judged broken), for the policy and for the baseline.
- harmful overrides: applied, and the baseline succeeded where the policy's arm did
  not, or the policy's arm came back with broken content. This is `labels.regressed`
  read on the pair the policy chose.
- helpful overrides: applied, the policy's arm is correct and the baseline failed.
- the net success delta, policy minus baseline over all pairs.

`from_outcomes` reads the `gates.PairOutcome` list the gate reads, so the two never
disagree about which arm the policy ran. Nothing here claims an improvement; on a
synthetic corpus every number describes a plant.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

from . import features as feat
from . import gates as gt
from . import report
from . import thresholds as th

RESAMPLES = 2000


@dataclass
class Comparison:
    pairs: int
    overrides: int
    coverage: float  # overrides / pairs
    policy_success: float
    baseline_success: float
    policy_content_ok: float
    baseline_content_ok: float
    policy_credits_per_correct: float
    baseline_credits_per_correct: float
    harmful: int
    harmful_rate: float  # over overrides, NaN when there is none
    harmful_rate_ucb: float  # 95th percentile over resamples by pair, NaN when none
    helpful: int
    helpful_rate: float
    success_delta: float
    credits_per_correct_delta: float

    def to_dict(self) -> dict:
        return {name: _plain(getattr(self, name)) for name in self.__dataclass_fields__}


def _plain(value):
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return value


def _ratio(num: float, den: float) -> float:
    return float(num / den) if den > 0 else float("nan")


def from_outcomes(pairs: list[gt.PairOutcome], resamples: int = RESAMPLES,
                  seed: int = 0) -> Comparison:
    n = len(pairs)
    applied = np.array([p.applied for p in pairs], dtype=bool)
    base_s = np.array([p.base_success for p in pairs], dtype=bool)
    arm_s = np.array([p.arm_success for p in pairs], dtype=bool)
    arm_ok = np.array([p.arm_content_ok for p in pairs], dtype=bool)
    base_c = np.array([p.base_credits for p in pairs], dtype=np.float64)
    arm_c = np.array([p.arm_credits for p in pairs], dtype=np.float64)
    correct = arm_s & arm_ok
    harmful = applied & ((base_s & ~arm_s) | ~arm_ok)
    helpful = applied & correct & ~base_s
    overrides = int(applied.sum())

    ucb = float("nan")
    if overrides > 0 and n > 0:
        rng = np.random.default_rng(seed)
        idx = rng.integers(0, n, size=(resamples, n))
        drawn = applied[idx].sum(axis=1)
        with np.errstate(divide="ignore", invalid="ignore"):
            rates = np.where(drawn > 0, harmful[idx].sum(axis=1) / np.maximum(drawn, 1), 0.0)
        ucb = float(np.percentile(rates, 95))

    policy_cpc = _ratio(arm_c.sum(), correct.sum()) if n else float("nan")
    base_cpc = _ratio(base_c.sum(), base_s.sum()) if n else float("nan")
    return Comparison(
        pairs=n,
        overrides=overrides,
        coverage=_ratio(overrides, n) if n else float("nan"),
        policy_success=float(arm_s.mean()) if n else float("nan"),
        baseline_success=float(base_s.mean()) if n else float("nan"),
        policy_content_ok=float(arm_ok.mean()) if n else float("nan"),
        baseline_content_ok=1.0 if n else float("nan"),
        policy_credits_per_correct=policy_cpc,
        baseline_credits_per_correct=base_cpc,
        harmful=int(harmful.sum()),
        harmful_rate=_ratio(harmful.sum(), overrides),
        harmful_rate_ucb=ucb,
        helpful=int(helpful.sum()),
        helpful_rate=_ratio(helpful.sum(), overrides),
        success_delta=float(arm_s.mean() - base_s.mean()) if n else float("nan"),
        credits_per_correct_delta=policy_cpc - base_cpc,
    )


def gate_pairs(model, cal, test: feat.Arrays, thresholds: th.Thresholds,
               tau: float) -> list[gt.PairOutcome]:
    """Every test pair as the gate reads it: the baseline arm beside the arm the gated
    policy would have run."""
    scored = th.score(model, cal, test, tau)
    cand = np.flatnonzero(scored.candidates)
    applied = th.applies(scored, thresholds)[cand]
    baselines = [test.rows[j] for j in scored.baseline_of[cand]]
    chosen = [test.rows[i] if a else test.rows[j]
              for i, j, a in zip(cand, scored.baseline_of[cand], applied, strict=True)]
    return gt.outcomes(baselines, chosen, applied, tau)


def compare(model, cal, test_window: feat.Arrays, thresholds: th.Thresholds, tau: float,
            resamples: int = RESAMPLES, seed: int = 0) -> Comparison:
    return from_outcomes(gate_pairs(model, cal, test_window, thresholds, tau), resamples, seed)


HEADING = "## Policy against the heuristic baseline"


def render(c: Comparison, synthetic: bool = False, kind: str | None = None) -> str:
    """One markdown section with one table. `synthetic` and `kind` only shape the
    sentence above it; the banner belongs to the report that embeds the section."""
    out = [f"{HEADING}\n\n"]
    who = f"The {kind} policy" if kind else "The policy"
    out.append(
        f"{who} against the arm the heuristic would have sent, on every one of the "
        f"{c.pairs} test pairs. An override is a pair on which the policy applied the "
        "candidate; on the rest its arm is the baseline arm. A harmful override lost a "
        "success the baseline had or came back with broken content; a helpful one is "
        "correct where the baseline failed. The harmful rate carries its 95 percent "
        "bootstrap upper bound, resampled by pair."
    )
    if synthetic:
        out.append(" Every number describes a planted effect.")
    out.append("\n\n")
    rows = [
        ["pairs", c.pairs, c.pairs, ""],
        ["overrides", c.overrides, 0, f"coverage {report.fmt(c.coverage)}"],
        ["success rate", c.policy_success, c.baseline_success,
         f"delta {report.fmt(c.success_delta)}"],
        ["content_ok rate", c.policy_content_ok, c.baseline_content_ok, ""],
        ["credits per correct result", c.policy_credits_per_correct,
         c.baseline_credits_per_correct, f"delta {report.fmt(c.credits_per_correct_delta)}"],
        ["harmful overrides", c.harmful, 0,
         f"rate {report.fmt(c.harmful_rate)}, upper bound {report.fmt(c.harmful_rate_ucb)}"],
        ["helpful overrides", c.helpful, 0, f"rate {report.fmt(c.helpful_rate)}"],
    ]
    out.append(report.table(["measure", "policy", "baseline", "note"], rows))
    return "".join(out)


def summary(c: Comparison) -> str:
    """The one printed line."""
    return (f"overrides {c.overrides} of {c.pairs} pairs (coverage {report.fmt(c.coverage)}), "
            f"harmful {c.harmful}, helpful {c.helpful}, success delta "
            f"{report.fmt(c.success_delta)}, credits per correct delta "
            f"{report.fmt(c.credits_per_correct_delta)}")
