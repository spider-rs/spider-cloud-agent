"""Whether the gated policy may ship, read pair by pair on the chronological test window.

For every test pair the policy either keeps the request, and its arm is the baseline
arm, or applies the candidate, and its arm is the candidate arm. Each check compares
the policy's arm with the baseline arm of the same pair, and every interval comes from
2000 bootstrap resamples of pairs. Fewer than 300 pairs is `insufficient`, which fails.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

from . import labels, report

MIN_PAIRS = 300
RESAMPLES = 2000
SUCCESS_FLOOR = -0.005
P50_SLACK = 0.10
P90_SLACK = 0.20


@dataclass
class PairOutcome:
    """One pair as the gate reads it: the baseline arm and the arm the policy chose."""

    pair: int
    base_success: bool
    base_credits: float
    base_millis: float
    arm_success: bool
    arm_content_ok: bool  # False when the chosen arm's content was judged broken
    arm_credits: float
    arm_millis: float
    applied: bool


def outcomes(baselines: list[dict], chosen: list[dict], applied, tau: float) -> list[PairOutcome]:
    out = []
    for base, arm, did in zip(baselines, chosen, applied, strict=True):
        out.append(PairOutcome(
            pair=int(base["pair"]),
            base_success=bool(base["success"]),
            base_credits=float(base["credits"]),
            base_millis=float(base["millis"]),
            arm_success=bool(arm["success"]),
            arm_content_ok=not (did and arm["success"] and labels.content_ok(arm, tau) is False),
            arm_credits=float(arm["credits"]),
            arm_millis=float(arm["millis"]),
            applied=bool(did),
        ))
    return out


def rule_of_three(n: int) -> float:
    """With zero failures in `n` independent trials, the 95 percent upper bound on the
    failure rate is about 3 / n."""
    return 3.0 / n if n > 0 else float("nan")


@dataclass
class Check:
    name: str
    passed: bool
    value: float
    bound: float
    limit: float
    note: str


@dataclass
class GateResult:
    status: str  # "pass", "fail" or "insufficient"
    pairs: int
    applied: int
    regressions: int  # applied pairs whose arm lost what the baseline got
    checks: list[Check] = field(default_factory=list)

    @property
    def passed(self) -> bool:
        return self.status == "pass"

    def failed(self) -> list[str]:
        return [c.name for c in self.checks if not c.passed]


def run(pairs: list[PairOutcome], resamples: int = RESAMPLES, seed: int = 0,
        min_pairs: int = MIN_PAIRS) -> GateResult:
    n = len(pairs)
    applied = sum(p.applied for p in pairs)
    regressions = sum(
        p.applied and ((p.base_success and not p.arm_success) or not p.arm_content_ok)
        for p in pairs
    )
    if n < min_pairs:
        return GateResult("insufficient", n, applied, regressions)

    base_s = np.array([p.base_success for p in pairs], dtype=np.float64)
    arm_s = np.array([p.arm_success for p in pairs], dtype=np.float64)
    arm_ok = np.array([p.arm_content_ok for p in pairs], dtype=np.float64)
    base_c = np.array([p.base_credits for p in pairs])
    arm_c = np.array([p.arm_credits for p in pairs])
    base_m = np.array([p.base_millis for p in pairs])
    arm_m = np.array([p.arm_millis for p in pairs])

    rng = np.random.default_rng(seed)
    idx = rng.integers(0, n, size=(resamples, n))

    def lower(values):
        return float(np.percentile(values, 2.5))

    def upper(values):
        return float(np.percentile(values, 97.5))

    checks = []
    delta = (arm_s - base_s)[idx].mean(axis=1)
    point = float((arm_s - base_s).mean())
    checks.append(Check("success delta", lower(delta) >= SUCCESS_FLOOR, point, lower(delta),
                        SUCCESS_FLOOR, "95 percent lower bound of policy minus baseline"))
    content = (arm_ok - 1.0)[idx].mean(axis=1)
    point = float((arm_ok - 1.0).mean())
    checks.append(Check("content_ok delta", lower(content) >= SUCCESS_FLOOR, point,
                        lower(content), SUCCESS_FLOOR,
                        "95 percent lower bound of the share not broken, minus one"))

    correct = arm_s * arm_ok
    with np.errstate(divide="ignore", invalid="ignore"):
        cpc_b = arm_c[idx].sum(axis=1) / correct[idx].sum(axis=1)
    base_point = float(base_c.sum() / base_s.sum()) if base_s.sum() else float("inf")
    arm_point = float(arm_c.sum() / correct.sum()) if correct.sum() else float("inf")
    checks.append(Check("credits per correct result", upper(cpc_b) <= base_point, arm_point,
                        upper(cpc_b), base_point,
                        "95 percent upper bound at or under the baseline point estimate"))

    for name, q, slack in (("p50 millis", 50, P50_SLACK), ("p90 millis", 90, P90_SLACK)):
        base_q = float(np.percentile(base_m, q))
        arm_q = float(np.percentile(arm_m, q))
        limit = base_q * (1.0 + slack)
        checks.append(Check(name, arm_q <= limit, arm_q, arm_q, limit,
                            f"within {int(slack * 100)} percent of the baseline"))

    status = "pass" if all(c.passed for c in checks) else "fail"
    return GateResult(status, n, applied, regressions, checks)


GUARANTEES = """## What code guarantees and what is measured

Some of what keeps a bad edit off a request is enforced by code and holds for every
request, whatever the model says. The validator refuses an edit the schema does not
allow. A field the caller set is never edited, and a caller who pinned the mode, the
pool or the country gets no edit at all. Budget caps are checked before any request
goes out. A missing model, a NaN score or an unsupported cell falls back to sending
the request unchanged, deterministically. An edit code whose floor is NaN in the
abstain table is never applied.

Everything else in this report is empirical: success, content, cost and latency deltas
estimated from a finite set of pairs, with intervals that hold at 95 percent under the
assumption that test pairs resemble future requests. A clean result bounds a failure
rate; it does not rule failures out. With zero failures in n pairs the 95 percent
upper bound on the failure rate is about 3/n (the rule of three). {count}
"""


def render(result: GateResult, synthetic: bool, kind: str) -> str:
    out = [f"# Regression report: {kind}\n\n", report.banner(synthetic)]
    out.append(
        f"Status: **{result.status}**. {result.pairs} test pairs, the policy applied an "
        f"edit on {result.applied}.\n\n"
    )
    if result.status == "insufficient":
        out.append(f"Fewer than {MIN_PAIRS} pairs, so no check ran and the gate fails.\n\n")
    else:
        rows = [[c.name, "pass" if c.passed else "fail", c.value, c.bound, c.limit, c.note]
                for c in result.checks]
        out.append(report.table(["check", "result", "point", "bound", "limit", "rule"], rows))
        out.append("\n")
    n = result.applied
    if n == 0:
        count = "The policy applied no edit here, so nothing is bounded."
    elif result.regressions == 0:
        count = (f"None of the {n} applied pairs regressed, which bounds the rate near "
                 f"{report.fmt(rule_of_three(n))}.")
    else:
        count = (f"{result.regressions} of the {n} applied pairs regressed, so the rule of "
                 "three does not apply and the intervals above are the evidence.")
    out.append(GUARANTEES.format(count=count))
    return "".join(out)
