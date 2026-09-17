"""Floors a run must clear on the test window, read from a JSON file.

A floors file names, per model kind, the smallest `auroc` and `pr_auc` and the largest
`ece_after`, `brier_after`, `mae_credits` and `mae_millis` the run may show on the
chronological test window, the exact number of edits the gated policy must apply
there, and the gate status it must land on. It may also bound the policy against the
heuristic baseline as `compare.py` reads it: the smallest coverage, the largest harmful
override rate (checked on its bootstrap upper bound, so a policy with no override
cannot meet it) and the smallest harmful count. On a synthetic corpus it also names the
smallest predicted uplift the model must show for each planted effect. Every check is
a comparison with a number the run measured; nothing here says a floor is good, only
that the run did not fall through it.

    {
      "expect_applied": 0,
      "expect_gate_status": "insufficient",
      "min_coverage": 0.05,
      "max_harmful_rate": 0.01,
      "min_harmful": 1,
      "min_planted_uplift": 0.25,
      "min_planted_rows": 20,
      "kinds": {
        "mlp": {"min_auroc": 0.8, "min_pr_auc": 0.85, "max_ece_after": 0.08,
                "max_brier_after": 0.2, "max_mae_credits": 1.0, "max_mae_millis": 1500},
        "gbdt": {...}
      }
    }

A planted effect is recovered when the model's mean calibrated success for the
candidate arm minus the baseline arm, over the test rows the effect applies to, clears
`min_planted_uplift` in the planted direction. The rows are found from the row data
alone: the edit code, the row's `ext` and `mem` labels, the status class the base
features carry for a non-cold site, the day, and the blacklist identifier. Which
effects that reaches, and which it cannot, is in `evals/README.md`.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import compare, report, synth
from . import gates as gt
from . import schema as sch
from . import thresholds as th

MIN_METRICS = ("auroc", "pr_auc")
MAX_METRICS = ("ece_after", "brier_after", "mae_credits", "mae_millis")
DEFAULT_MIN_PLANTED_ROWS = 20


@dataclass
class Floors:
    kinds: dict[str, dict[str, float]]
    expect_applied: int | None = None
    expect_gate_status: str | None = None
    min_coverage: float | None = None
    max_harmful_rate: float | None = None
    min_harmful: int | None = None
    min_planted_uplift: float | None = None
    min_planted_rows: int = DEFAULT_MIN_PLANTED_ROWS

    @classmethod
    def read(cls, path: Path) -> Floors:
        doc = json.loads(Path(path).read_text())
        kinds = doc.get("kinds")
        if not isinstance(kinds, dict) or not kinds:
            raise ValueError(f"{path}: 'kinds' must map each model kind to its floors")
        allowed = {f"min_{m}" for m in MIN_METRICS} | {f"max_{m}" for m in MAX_METRICS}
        for kind, floors in kinds.items():
            unknown = sorted(set(floors) - allowed)
            if unknown:
                raise ValueError(f"{path}: unknown floor(s) for {kind}: {', '.join(unknown)}")
        return cls(
            kinds={k: {n: float(v) for n, v in f.items()} for k, f in kinds.items()},
            expect_applied=doc.get("expect_applied"),
            expect_gate_status=doc.get("expect_gate_status"),
            min_coverage=doc.get("min_coverage"),
            max_harmful_rate=doc.get("max_harmful_rate"),
            min_harmful=doc.get("min_harmful"),
            min_planted_uplift=doc.get("min_planted_uplift"),
            min_planted_rows=int(doc.get("min_planted_rows", DEFAULT_MIN_PLANTED_ROWS)),
        )


@dataclass
class Check:
    name: str
    value: float | int | str
    rule: str  # ">= 0.8", "= 0", "= insufficient"
    passed: bool | None  # None when the check reports a number but gates nothing
    note: str = ""


@dataclass
class KindResult:
    kind: str
    checks: list[Check] = field(default_factory=list)
    planted: list[Check] = field(default_factory=list)
    comparison: compare.Comparison | None = None

    def failed(self) -> list[Check]:
        return [c for c in self.checks + self.planted if c.passed is False]


def metric_checks(kind: str, metrics: dict, floors: Floors) -> list[Check]:
    out = []
    for name, floor in sorted(floors.kinds.get(kind, {}).items()):
        metric = name[4:]
        value = float(metrics[metric])
        if name.startswith("min_"):
            ok = math.isfinite(value) and value >= floor
            out.append(Check(metric, value, f">= {report.fmt(floor)}", ok))
        else:
            ok = math.isfinite(value) and value <= floor
            out.append(Check(metric, value, f"<= {report.fmt(floor)}", ok))
    return out


def gate_checks(result: gt.GateResult, floors: Floors) -> list[Check]:
    out = []
    if floors.expect_applied is not None:
        want = int(floors.expect_applied)
        out.append(Check("applied edits", result.applied, f"= {want}", result.applied == want))
    if floors.expect_gate_status is not None:
        out.append(Check("gate status", result.status, f"= {floors.expect_gate_status}",
                         result.status == floors.expect_gate_status, result.reason))
    return out


def comparison_checks(c: compare.Comparison, floors: Floors) -> list[Check]:
    """The policy against the baseline, where the floors file bounds it. The harmful
    rate is checked on its bootstrap upper bound, which is NaN without an override, so
    a policy that abstained everywhere misses `max_harmful_rate` as well as
    `min_coverage`."""
    out = []
    if floors.min_coverage is not None:
        least = float(floors.min_coverage)
        out.append(Check("coverage", c.coverage, f">= {report.fmt(least)}",
                         math.isfinite(c.coverage) and c.coverage >= least,
                         f"{c.overrides} overrides on {c.pairs} pairs"))
    if floors.max_harmful_rate is not None:
        most = float(floors.max_harmful_rate)
        out.append(Check("harmful override rate", c.harmful_rate_ucb,
                         f"<= {report.fmt(most)}",
                         math.isfinite(c.harmful_rate_ucb) and c.harmful_rate_ucb <= most,
                         f"95 percent upper bound; point {report.fmt(c.harmful_rate)}, "
                         f"{c.harmful} of {c.overrides} overrides"))
    if floors.min_harmful is not None:
        least_n = int(floors.min_harmful)
        out.append(Check("harmful overrides", c.harmful, f">= {least_n}",
                         c.harmful >= least_n, f"of {c.overrides} overrides"))
    return out


@dataclass
class Effect:
    """Which test rows a planted effect applies to, and what the plant did to the
    success label there."""

    name: str
    mask: np.ndarray  # over candidate rows of the window
    planted: float  # planted change in the success chance, negative for a break
    note: str = ""
    by_day: bool = False  # the plant depends on the day, so a window may hold no row
    gated: bool = True  # False reports the prediction beside the plant and checks nothing


def _status_of(rows: list[dict]) -> list[str]:
    return [synth.site_of(r).status for r in rows]


def planted_effects(planted: dict, scored: th.Scored) -> list[Effect]:
    """The planted effects the row data can locate, over the window's candidate rows.

    The synthetic generator gives a blocked or empty site a warm memory, so a cold
    row is an `ok` site and a non-cold row carries its status class in the base
    features. That is what lets the wait and the browser effect be located."""
    schema = sch.load()
    a = scored.arrays
    cand = np.flatnonzero(scored.candidates)
    rows = [a.rows[i] for i in cand]
    wire = np.array([schema.code_name(int(a.code[i])) if a.code[i] > 0 else "" for i in cand],
                    dtype=object)
    ext = a.ext[cand]
    mem = a.mem[cand]
    day = a.day[cand]
    status = np.array(_status_of(rows), dtype=object)
    out = []

    wait = planted.get("wait")
    if wait:
        mask = (wire == wait["wire"]) & (ext == wait["ext"]) & (mem == wait["mem"])
        out.append(Effect("wait on a cold markup site", mask,
                          wait["success_to"] - wait["success_from"]))

    browser = planted.get("browser")
    if browser:
        mask = (wire == browser["wire"]) & (status == browser["status"])
        out.append(Effect("browser mode on an empty site", mask,
                          browser["success_to"] - browser["success_from"]))

    res = planted.get("residential")
    if res:
        on = (wire == res["wire"]) & (status == res["status"])
        flipped = day >= planted["days"] - res["flipped_last_days"]
        out.append(Effect("residential proxy on a blocked site, before the flip",
                          on & ~flipped, res["success_to"] - res["success_from"],
                          "the chronological test window may lie past the flip", by_day=True))
        # No floor can say what a model should predict about a change that no row it
        # was fitted on carried, so the flipped days are reported beside the plant and
        # never gated; the gate on the test window is what rejects the artifact.
        out.append(Effect("residential proxy on a blocked site, after the flip",
                          on & flipped, res["success_when_flipped"] - res["success_from"],
                          "the plant changes the effect on these days; reported, not gated",
                          by_day=True, gated=False))

    style = planted.get("stylesheets")
    if style:
        mask = (wire == style["wire"]) & (ext == style["breaks_ext"])
        out.append(Effect("blocked stylesheets break the page", mask, -1.0,
                          "a break lowers the success label; the gate is on the drop"))

    black = planted.get("blacklist")
    if black and black.get("first_party_breaks"):
        first = np.array([
            bool(r["edit"]) and isinstance(r["edit"].get("ident"), dict)
            and not r["edit"]["ident"].get("third", True)
            for r in rows
        ])
        mask = (wire == black["wire"]) & first
        out.append(Effect("first party blacklist breaks the page", mask, -1.0,
                          "a break lowers the success label; the gate is on the drop"))
    return out


def planted_checks(planted: dict, scored: th.Scored, floors: Floors) -> list[Check]:
    if floors.min_planted_uplift is None:
        return []
    least = float(floors.min_planted_uplift)
    a = scored.arrays
    cand = np.flatnonzero(scored.candidates)
    base = scored.baseline_of[cand]
    predicted = scored.p[cand] - scored.p[base]
    observed = a.success[cand] - a.success[base]
    out = []
    for effect in planted_effects(planted, scored):
        n = int(effect.mask.sum())
        gated = effect.gated and effect.planted != 0.0
        if n < floors.min_planted_rows:
            note = f"{n} rows, fewer than {floors.min_planted_rows}"
            if effect.by_day:
                note = f"{note}; {effect.note}"
            out.append(Check(effect.name, float("nan"), "n/a",
                             False if gated and not effect.by_day else None, note))
            continue
        mean = float(predicted[effect.mask].mean())
        seen = float(observed[effect.mask].mean())
        note = f"{n} rows, observed {report.fmt(seen)}"
        if effect.note:
            note = f"{note}; {effect.note}"
        if not gated:
            out.append(Check(effect.name, mean, "reported", None, note))
        elif effect.planted > 0:
            out.append(Check(effect.name, mean, f">= {report.fmt(least)}", mean >= least, note))
        else:
            out.append(Check(effect.name, mean, f"<= {report.fmt(-least)}", mean <= -least,
                             note))
    return out


def render(results: list[KindResult], synthetic: bool, floors_path: str,
           test_rows: int) -> str:
    out = ["# Eval report\n\n", report.banner(synthetic)]
    out.append(
        f"Floors from `{floors_path}`, checked on the chronological test window "
        f"({test_rows} rows). A floor is met when the measured value is on the allowed "
        "side of it; the table says nothing about how good the value is.\n"
    )
    for r in results:
        out.append(f"\n## {r.kind}\n\n")
        rows = [[c.name, c.value, c.rule, _verdict(c.passed)] for c in r.checks]
        out.append(report.table(["metric", "value", "floor", "pass"], rows))
        if r.planted:
            out.append("\nPlanted effects, the model's mean calibrated success for the "
                       "candidate arm minus the baseline arm on the rows the plant "
                       "applies to:\n\n")
            rows = [[c.name, c.value, c.rule, _verdict(c.passed), c.note] for c in r.planted]
            out.append(report.table(["effect", "predicted", "floor", "pass", "rows"], rows))
        if r.comparison is not None:
            out.append("\n" + compare.render(r.comparison, synthetic, r.kind))
    failed = [c for r in results for c in r.failed()]
    out.append(f"\n{len(failed)} floor(s) missed.\n" if failed else "\nEvery floor met.\n")
    return "".join(out)


def _verdict(passed: bool | None) -> str:
    if passed is None:
        return "n/a"
    return "pass" if passed else "fail"


def failed_lines(results: list[KindResult]) -> list[str]:
    out = []
    for r in results:
        for c in r.failed():
            value = c.value if isinstance(c.value, str) else report.fmt(c.value)
            out.append(f"{r.kind} {c.name}: {value} misses {c.rule}")
    return out
