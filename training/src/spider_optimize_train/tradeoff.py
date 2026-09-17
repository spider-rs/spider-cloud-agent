"""What each risk budget buys: the floors re-chosen at several `r_max` on the calibrate
window, and the policy each table gives read against the baseline on the test window.

A larger `r_max` lets the sweep accept a lower floor, so coverage never falls as it
rises, and the test window says what the extra overrides did. The table is the
tradeoff between useful overrides and regression risk for one run; it chooses nothing
and claims nothing, and on a synthetic corpus it describes a plant.
"""

from __future__ import annotations

from dataclasses import dataclass

from . import compare, report
from . import features as feat
from . import gates as gt
from . import thresholds as th

R_MAX_GRID = (0.005, 0.01, 0.02, 0.05, 0.1)


@dataclass
class Row:
    r_max: float
    floors: int  # edit codes with a finite floor
    comparison: compare.Comparison
    gate: gt.GateResult


def sweep(model, cal, windows: dict[str, feat.Arrays], tau: float,
          grid=R_MAX_GRID, min_covered: int = th.MIN_COVERED, min_sites: int = th.MIN_SITES,
          resamples: int = th.RESAMPLES, gate_resamples: int = gt.RESAMPLES,
          seed: int = 0) -> list[Row]:
    scored = th.score(model, cal, windows["calibrate"], tau)
    out = []
    for r_max in grid:
        chosen = th.choose(scored, windows["train"], r_max, min_covered, min_sites, resamples,
                           seed)
        pairs = compare.gate_pairs(model, cal, windows["test"], chosen, tau)
        finite = sum(1 for v in chosen.per_code if v == v)
        out.append(Row(float(r_max), finite, compare.from_outcomes(pairs, gate_resamples, seed),
                       gt.run(pairs, gate_resamples, seed)))
    return out


HEADERS = ["r_max", "floors", "coverage", "overrides", "helpful", "harmful", "harmful rate",
           "success delta", "credits per correct delta", "gate"]


def table_rows(rows: list[Row]) -> list[list]:
    out = []
    for r in rows:
        c = r.comparison
        out.append([r.r_max, r.floors, c.coverage, c.overrides, c.helpful, c.harmful,
                    c.harmful_rate, c.success_delta, c.credits_per_correct_delta,
                    r.gate.status])
    return out


def render(per_kind: dict[str, list[Row]], synthetic: bool, min_covered: int,
           min_sites: int) -> str:
    out = ["# Tradeoff: overrides against regression risk\n\n", report.banner(synthetic)]
    out.append(
        "For each `r_max` the floors were chosen again on the calibrate window, with at "
        f"least {min_covered} rows covered and cells supported on {min_sites} sites, and "
        "the policy that table gives was read against the heuristic baseline on the "
        "chronological test window. `floors` counts the edit codes that got a finite floor; "
        "the rest abstain. Coverage is the share of test pairs with an override. Helpful and "
        "harmful count overrides as `compare.py` defines them, and the harmful rate is over "
        "overrides. The deltas are policy minus baseline. `gate` is what "
        "`gates` would return for that table.\n"
    )
    for kind, rows in per_kind.items():
        out.append(f"\n## {kind}\n\n")
        out.append(report.table(HEADERS, table_rows(rows)))
    return "".join(out)
