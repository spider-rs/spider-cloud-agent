"""Where each edit's success floor goes, and which cells have the evidence to be used.

For each learnable edit code, and pooled over all of them, the sweep raises a floor `t`
from 0.5 to 1.0. A candidate row is covered at `t` when its calibrated success chance
is at least `t` and its predicted credits per correct result are below the prediction
for its own pair's baseline arm. Risk at `t` is the share of covered rows whose pair
regressed. The risk bound is the 95th percentile of that share over 1000 bootstrap
resamples drawn by pair, since arms of one pair are not independent evidence. The
floor is the smallest `t` whose bound is at most `r_max` with at least 200 rows
covered; when no `t` qualifies the code abstains, written as NaN.

A cell is `need << 24 | ext << 16 | mem << 8 | edit_code`. It is supported when the
train window saw it on at least 50 distinct sites.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

import numpy as np

from . import calibrate, labels
from . import features as feat
from . import schema as sch

T_GRID = np.round(np.arange(0.5, 1.0 + 1e-9, 0.005), 3)
R_MAX = 0.01
MIN_COVERED = 200
RESAMPLES = 1000
MIN_SITES = 50


@dataclass
class Scored:
    """A window with a model's calibrated predictions and each row's pair context."""

    arrays: feat.Arrays
    p: np.ndarray
    p_raw: np.ndarray
    log_millis: np.ndarray
    log_credits: np.ndarray
    baseline_of: np.ndarray  # row index of the pair's baseline arm, -1 when none
    regressed: np.ndarray  # candidate rows only; False on baseline rows
    cheaper: np.ndarray  # predicted credits per correct result below the baseline's

    @property
    def candidates(self) -> np.ndarray:
        return (~self.arrays.baseline) & (self.baseline_of >= 0)


def cost_per_correct(p, log_credits):
    p = np.asarray(p, dtype=np.float64)
    credits = np.expm1(np.asarray(log_credits, dtype=np.float64))
    with np.errstate(divide="ignore", invalid="ignore"):
        return np.where(p > 0, credits / np.maximum(p, 1e-12), np.inf)


def score(model, cal: calibrate.Calibration, arrays: feat.Arrays,
          tau: float = labels.DEFAULT_TAU) -> Scored:
    raw, log_millis, log_credits = model.predict(arrays.X)
    p = cal.apply(raw)
    index_of_baseline: dict[int, int] = {}
    for i, is_base in enumerate(arrays.baseline):
        if is_base:
            index_of_baseline[int(arrays.pair[i])] = i
    n = len(arrays)
    baseline_of = np.full(n, -1, dtype=np.int64)
    regressed = np.zeros(n, dtype=bool)
    for i in range(n):
        if arrays.baseline[i]:
            continue
        j = index_of_baseline.get(int(arrays.pair[i]), -1)
        baseline_of[i] = j
        if j >= 0:
            regressed[i] = labels.regressed(arrays.rows[i], arrays.rows[j], tau)
    cpc = cost_per_correct(p, log_credits)
    cheaper = np.zeros(n, dtype=bool)
    has = baseline_of >= 0
    cheaper[has] = cpc[has] < cpc[baseline_of[has]]
    return Scored(arrays, p, raw, log_millis, log_credits, baseline_of, regressed, cheaper)


def bootstrap_pair_weights(pairs, resamples: int, rng: np.random.Generator) -> np.ndarray:
    """(resamples, rows) weights: how many times each row's pair was drawn when the
    pairs are resampled with replacement."""
    unique, inverse = np.unique(np.asarray(pairs), return_inverse=True)
    k = len(unique)
    counts = rng.multinomial(k, np.full(k, 1.0 / k), size=resamples).astype(np.float32)
    return counts[:, inverse]


@dataclass
class Sweep:
    t: np.ndarray
    covered: np.ndarray
    coverage: np.ndarray
    risk: np.ndarray
    risk_ucb: np.ndarray
    rows: int
    threshold: float
    r_max: float
    min_covered: int

    def at(self, t: float) -> int:
        return int(np.argmin(np.abs(self.t - t)))

    def to_dict(self) -> dict:
        return {
            "threshold": None if math.isnan(self.threshold) else self.threshold,
            "rows": self.rows,
            "r_max": self.r_max,
            "min_covered": self.min_covered,
        }


def sweep(p, cheaper, regressed, pairs, r_max: float = R_MAX, min_covered: int = MIN_COVERED,
          resamples: int = RESAMPLES, seed: int = 0) -> Sweep:
    p = np.asarray(p, dtype=np.float64)
    cheaper = np.asarray(cheaper, dtype=bool)
    regressed = np.asarray(regressed, dtype=bool)
    n = len(p)
    t = T_GRID
    covered_t = (p[None, :] >= t[:, None]) & cheaper[None, :]
    covered = covered_t.sum(axis=1)
    bad = (covered_t & regressed[None, :]).sum(axis=1)
    with np.errstate(divide="ignore", invalid="ignore"):
        risk = np.where(covered > 0, bad / np.maximum(covered, 1), np.nan)
    ucb = np.full(len(t), np.nan)
    if n > 0:
        rng = np.random.default_rng(seed)
        w = bootstrap_pair_weights(pairs, resamples, rng)
        cov_b = w @ covered_t.T.astype(np.float32)
        bad_b = w @ (covered_t & regressed[None, :]).T.astype(np.float32)
        with np.errstate(divide="ignore", invalid="ignore"):
            risk_b = np.where(cov_b > 0, bad_b / np.maximum(cov_b, 1e-9), 1.0)
        ucb = np.where(covered > 0, np.percentile(risk_b, 95, axis=0), np.nan)
    threshold = float("nan")
    for at in range(len(t)):
        if covered[at] >= min_covered and ucb[at] <= r_max:
            threshold = float(t[at])
            break
    return Sweep(t, covered, covered / max(n, 1), risk, ucb, n, threshold, r_max, min_covered)


def support_table(train: feat.Arrays, min_sites: int = MIN_SITES) -> list[int]:
    sites: dict[int, set[int]] = {}
    for cell, dk in zip(train.cell, train.dk, strict=True):
        if cell >= 0:
            sites.setdefault(int(cell), set()).add(int(dk))
    return sorted(cell for cell, seen in sites.items() if len(seen) >= min_sites)


@dataclass
class Thresholds:
    per_code: list[float]  # indexed by edit code, keep at 0
    pooled: float
    sweeps: dict[str, Sweep]
    support: list[int]
    notes: list[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        nan = lambda v: None if math.isnan(v) else v  # noqa: E731
        return {
            "per_code": [nan(v) for v in self.per_code],
            "pooled": nan(self.pooled),
            "sweeps": {k: s.to_dict() for k, s in self.sweeps.items()},
            "support": self.support,
            "notes": self.notes,
        }

    @classmethod
    def from_dict(cls, doc: dict) -> Thresholds:
        undo = lambda v: float("nan") if v is None else float(v)  # noqa: E731
        return cls([undo(v) for v in doc["per_code"]], undo(doc["pooled"]), {},
                   list(doc["support"]), list(doc.get("notes", [])))


def choose(scored: Scored, train: feat.Arrays, r_max: float = R_MAX,
           min_covered: int = MIN_COVERED, min_sites: int = MIN_SITES,
           resamples: int = RESAMPLES, seed: int = 0) -> Thresholds:
    schema = sch.load()
    a = scored.arrays
    cand = scored.candidates & (a.code > 0)
    sweeps: dict[str, Sweep] = {}
    per_code = [float("nan")] * schema.edit_codes
    for code in range(1, schema.edit_codes):
        mask = cand & (a.code == code)
        s = sweep(scored.p[mask], scored.cheaper[mask], scored.regressed[mask], a.pair[mask],
                  r_max, min_covered, resamples, seed + code)
        sweeps[schema.code_name(code)] = s
        per_code[code] = s.threshold
    pooled = sweep(scored.p[cand], scored.cheaper[cand], scored.regressed[cand], a.pair[cand],
                   r_max, min_covered, resamples, seed)
    sweeps["pooled"] = pooled
    return Thresholds(per_code, pooled.threshold, sweeps, support_table(train, min_sites))


def applies(scored: Scored, th: Thresholds) -> np.ndarray:
    """Whether the gated policy would apply each candidate row's edit: a floor exists,
    the calibrated chance clears it, the cell is supported and it is cheaper per
    correct result than keep."""
    a = scored.arrays
    floors = np.array(th.per_code + [float("nan")])
    code = np.where((a.code > 0) & (a.code < len(th.per_code)), a.code, len(th.per_code))
    floor = floors[code]
    supported = np.isin(a.cell, np.asarray(th.support, dtype=np.int64))
    with np.errstate(invalid="ignore"):
        clears = np.isfinite(floor) & (scored.p >= floor)
    return scored.candidates & (a.code > 0) & clears & supported & scored.cheaper
