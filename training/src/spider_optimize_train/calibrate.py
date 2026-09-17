"""Turning a success head's raw output into a probability that means what it says.

The success head is trained with class weights and recency weights, so its raw
probabilities are off by design. Calibration is fitted on the calibrate window, which
the model never trained or early stopped on: isotonic regression when that window
holds at least 500 candidate rows, Platt scaling otherwise, since a step function fitted
to a few hundred rows mostly memorises them.

Both work on the raw logit, which is what the artifact's success head emits. The
artifact layout is: kind 1 (Platt) with one (a, b) pair, `p = sigmoid(a * z + b)`, or
kind 2 (isotonic) with knots (x, y), `p` read by linear interpolation and held flat
past either end. The artifact needs at least two isotonic knots, so a fit that pools
into a single block falls back to Platt.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

IDENTITY = 0
PLATT = 1
ISOTONIC = 2
MIN_ISOTONIC_ROWS = 500
ECE_BINS = 10
MAX_KNOTS = 512


def sigmoid(z):
    """The logistic function, written so a large |z| does not overflow."""
    z = np.asarray(z, dtype=np.float64)
    e = np.exp(-np.abs(z))
    return np.where(z >= 0, 1.0 / (1.0 + e), e / (1.0 + e))


def logit(p, eps: float = 1e-7):
    p = np.clip(np.asarray(p, dtype=np.float64), eps, 1.0 - eps)
    return np.log(p / (1.0 - p))


def brier(p, y) -> float:
    p = np.asarray(p, dtype=np.float64)
    y = np.asarray(y, dtype=np.float64)
    return float(np.mean((p - y) ** 2))


def ece(p, y, bins: int = ECE_BINS) -> float:
    """Expected calibration error over equal-width bins of `p`: the row-weighted mean of
    |mean predicted - observed rate| per bin. A `p` of exactly 1 falls in the last bin."""
    p = np.asarray(p, dtype=np.float64)
    y = np.asarray(y, dtype=np.float64)
    if len(p) == 0:
        return float("nan")
    at = np.minimum((p * bins).astype(int), bins - 1)
    total = 0.0
    for b in range(bins):
        mask = at == b
        if mask.any():
            total += mask.sum() * abs(p[mask].mean() - y[mask].mean())
    return float(total / len(p))


def pav(x, y, w=None) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Pool adjacent violators. Returns, per pooled block in order of `x`, the block's
    smallest x, largest x and fitted value; the fit is non-decreasing."""
    x = np.asarray(x, dtype=np.float64)
    y = np.asarray(y, dtype=np.float64)
    w = np.ones_like(y) if w is None else np.asarray(w, dtype=np.float64)
    order = np.argsort(x, kind="mergesort")
    x, y, w = x[order], y[order], w[order]
    values: list[float] = []
    weights: list[float] = []
    lo: list[float] = []
    hi: list[float] = []
    for xi, yi, wi in zip(x, y, w, strict=True):
        values.append(yi)
        weights.append(wi)
        lo.append(xi)
        hi.append(xi)
        while len(values) > 1 and values[-2] > values[-1]:
            total = weights[-2] + weights[-1]
            merged = (values[-2] * weights[-2] + values[-1] * weights[-1]) / total
            values[-2:] = [merged]
            weights[-2:] = [total]
            lo[-2:] = [lo[-2]]
            hi[-2:] = [hi[-1]]
    return np.array(lo), np.array(hi), np.array(values)


def platt(z, y, iterations: int = 100) -> tuple[float, float]:
    """Fit `p = sigmoid(a * z + b)` by Newton's method on the log loss, with Platt's
    target smoothing so a separable window does not send `a` to infinity."""
    z = np.asarray(z, dtype=np.float64)
    y = np.asarray(y, dtype=np.float64)
    positives = y.sum()
    negatives = len(y) - positives
    target = np.where(y > 0.5, (positives + 1) / (positives + 2), 1 / (negatives + 2))
    a, b = 1.0, 0.0
    for _ in range(iterations):
        p = sigmoid(a * z + b)
        g = np.array([np.sum((p - target) * z), np.sum(p - target)])
        s = p * (1 - p)
        h = np.array([[np.sum(s * z * z), np.sum(s * z)], [np.sum(s * z), np.sum(s)]])
        h += np.eye(2) * 1e-9
        step = np.linalg.solve(h, g)
        a, b = a - step[0], b - step[1]
        if np.max(np.abs(step)) < 1e-10:
            break
    return float(a), float(b)


@dataclass
class Calibration:
    kind: int = IDENTITY
    params: list[float] = field(default_factory=list)  # (a, b) for Platt
    knots: list[tuple[float, float]] = field(default_factory=list)  # (x, y) for isotonic
    report: dict = field(default_factory=dict)

    def apply(self, z):
        """A probability from the success head's raw logit."""
        z = np.asarray(z, dtype=np.float64)
        if self.kind == PLATT:
            a, b = self.params
            return sigmoid(a * z + b)
        if self.kind == ISOTONIC:
            xs = np.array([k[0] for k in self.knots])
            ys = np.array([k[1] for k in self.knots])
            return np.interp(z, xs, ys)
        return sigmoid(z)

    def to_dict(self) -> dict:
        return {"kind": self.kind, "params": self.params, "knots": self.knots,
                "report": self.report}

    @classmethod
    def from_dict(cls, doc: dict) -> Calibration:
        return cls(doc["kind"], list(doc["params"]), [tuple(k) for k in doc["knots"]],
                   doc.get("report", {}))


def isotonic_knots(z, y) -> list[tuple[float, float]]:
    """Knots for the fitted step function: each block's two ends, strictly increasing
    in x, at most `MAX_KNOTS`."""
    lo, hi, values = pav(z, y)
    knots: list[tuple[float, float]] = []
    for a, b, v in zip(lo, hi, values, strict=True):
        for x in (a, b) if b > a else (a,):
            if knots and x <= knots[-1][0] + 1e-4:
                continue
            knots.append((float(x), float(v)))
    if len(knots) > MAX_KNOTS:
        pick = np.unique(np.linspace(0, len(knots) - 1, MAX_KNOTS).round().astype(int))
        knots = [knots[i] for i in pick]
    return knots


def fit(z, y, candidate_rows: int, force: int | None = None) -> Calibration:
    """Fit on the calibrate window and report ECE and Brier before and after on it."""
    z = np.asarray(z, dtype=np.float64)
    y = np.asarray(y, dtype=np.float64)
    kind = force if force is not None else (
        ISOTONIC if candidate_rows >= MIN_ISOTONIC_ROWS else PLATT
    )
    knots = isotonic_knots(z, y) if kind == ISOTONIC else []
    if kind == ISOTONIC and len(knots) < 2:
        # One pooled block is a constant, and the artifact needs two knots for
        # isotonic; Platt still ranks the rows.
        kind = PLATT
    if kind == ISOTONIC:
        cal = Calibration(ISOTONIC, knots=knots)
    elif kind == PLATT:
        cal = Calibration(PLATT, params=list(platt(z, y)))
    else:
        cal = Calibration(IDENTITY)
    before = sigmoid(z)
    after = cal.apply(z)
    cal.report = {
        "method": {IDENTITY: "identity", PLATT: "platt", ISOTONIC: "isotonic"}[kind],
        "rows": int(len(z)),
        "candidate_rows": int(candidate_rows),
        "ece_before": ece(before, y),
        "ece_after": ece(after, y),
        "brier_before": brier(before, y),
        "brier_after": brier(after, y),
    }
    return cal
