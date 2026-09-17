"""How good a trained kind is on the test window, and what its policy would have cost.

Policy value is read on matched rows only: a pair counts for a policy when the arm that
policy would have chosen is an arm that was actually run. Every pair ran keep and one
candidate, so the logged policy, keep only, and the gated model are all matched on
every pair here; the matched count still sits beside each number, because a corpus with
more than one candidate per request will not be. The model-scored rows replace observed
cost and time with the model's own predictions and are labelled as such.
"""

from __future__ import annotations

import math

import numpy as np

from . import calibrate, report
from . import features as feat
from . import schema as sch
from . import thresholds as th


def auroc(scores, y) -> float:
    """The chance a random positive outranks a random negative, ties counting half."""
    scores = np.asarray(scores, dtype=np.float64)
    y = np.asarray(y) > 0.5
    positives, negatives = int(y.sum()), int((~y).sum())
    if positives == 0 or negatives == 0:
        return float("nan")
    order = np.argsort(scores, kind="mergesort")
    ranks = np.empty(len(scores), dtype=np.float64)
    sorted_scores = scores[order]
    start = 0
    while start < len(scores):
        end = start
        while end + 1 < len(scores) and sorted_scores[end + 1] == sorted_scores[start]:
            end += 1
        ranks[order[start : end + 1]] = (start + end) / 2.0 + 1.0
        start = end + 1
    return float((ranks[y].sum() - positives * (positives + 1) / 2.0) / (positives * negatives))


def pr_auc(scores, y) -> float:
    """Average precision: the sum over distinct score cut-offs, highest first, of the
    recall gained times the precision there."""
    scores = np.asarray(scores, dtype=np.float64)
    y = np.asarray(y) > 0.5
    positives = int(y.sum())
    if positives == 0:
        return float("nan")
    total = 0.0
    last_recall = 0.0
    for cut in np.unique(scores)[::-1]:
        taken = scores >= cut
        hits = int((taken & y).sum())
        recall = hits / positives
        precision = hits / int(taken.sum())
        total += (recall - last_recall) * precision
        last_recall = recall
    return float(total)


def mae(a, b) -> float:
    a = np.asarray(a, dtype=np.float64)
    b = np.asarray(b, dtype=np.float64)
    return float(np.mean(np.abs(a - b))) if len(a) else float("nan")


def success_metrics(scored: th.Scored) -> dict:
    a = scored.arrays
    before = calibrate.sigmoid(scored.p_raw)
    return {
        "rows": len(a),
        "auroc": auroc(scored.p, a.success),
        "pr_auc": pr_auc(scored.p, a.success),
        "brier_before": calibrate.brier(before, a.success),
        "brier_after": calibrate.brier(scored.p, a.success),
        "ece_before": calibrate.ece(before, a.success),
        "ece_after": calibrate.ece(scored.p, a.success),
        "mae_millis": mae(np.expm1(scored.log_millis), np.expm1(a.log_millis)),
        "mae_credits": mae(np.expm1(scored.log_credits), np.expm1(a.log_credits)),
    }


def categories(scored: th.Scored) -> dict[str, list[list]]:
    a = scored.arrays
    schema = sch.load()
    columns = {
        "need": a.need,
        "ext": a.ext,
        "tld group": a.tld,
        "mem": a.mem,
        "edit code": np.array([schema.code_name(c) if c >= 0 else "other" for c in a.code],
                              dtype=object),
    }
    out = {}
    for name, values in columns.items():
        rows = []
        for value in sorted(set(values.tolist()), key=str):
            mask = values == value
            rows.append([
                str(value),
                int(mask.sum()),
                float(a.success[mask].mean()),
                float(scored.p[mask].mean()),
                calibrate.brier(scored.p[mask], a.success[mask]),
                auroc(scored.p[mask], a.success[mask]),
            ])
        out[name] = rows
    return out


def _policy_rows(scored: th.Scored, choice_rows: np.ndarray) -> dict:
    a = scored.arrays
    idx = choice_rows
    matched = len(idx)
    if matched == 0:
        return {"matched": 0}
    credits = np.expm1(a.log_credits[idx])
    millis = np.expm1(a.log_millis[idx])
    correct = a.success[idx].sum()
    model_credits = np.expm1(scored.log_credits[idx])
    model_millis = np.expm1(scored.log_millis[idx])
    return {
        "matched": matched,
        "credits_per_correct": float(credits.sum() / correct) if correct else float("inf"),
        "p50_millis": float(np.percentile(millis, 50)),
        "p90_millis": float(np.percentile(millis, 90)),
        "model_credits_per_correct": float(model_credits.sum() / scored.p[idx].sum()),
        "model_p50_millis": float(np.percentile(model_millis, 50)),
        "model_p90_millis": float(np.percentile(model_millis, 90)),
    }


def policy_value(scored: th.Scored, thresholds: th.Thresholds) -> dict:
    cand = np.flatnonzero(scored.candidates)
    base = scored.baseline_of[cand]
    applied = th.applies(scored, thresholds)[cand]
    gated = np.where(applied, cand, base)
    return {
        "logged": _policy_rows(scored, cand),
        "keep only": _policy_rows(scored, base),
        "gated model": _policy_rows(scored, gated),
        "applied": int(applied.sum()),
    }


def evaluate(model, cal, test: feat.Arrays, thresholds: th.Thresholds, tau: float) -> dict:
    scored = th.score(model, cal, test, tau)
    return {
        "success": success_metrics(scored),
        "categories": categories(scored),
        "policy": policy_value(scored, thresholds),
    }


def render(kind: str, result: dict, synthetic: bool, quantized: dict | None = None,
           calibration: dict | None = None) -> str:
    out = [f"# Evaluation: {kind}\n\n", report.banner(synthetic)]
    out.append("## Success, millis and credits on the test window\n\n")
    headers = ["metric", "fp32"] + (["int8 (Python only)"] if quantized else [])
    rows = []
    for name, value in result["success"].items():
        row = [name, value]
        if quantized:
            row.append(quantized["success"][name])
        rows.append(row)
    out.append(report.table(headers, rows))
    if calibration:
        out.append(
            f"\nCalibration ({calibration['method']}) on the calibrate window, "
            f"{calibration['rows']} rows: ECE {report.fmt(calibration['ece_before'])} before, "
            f"{report.fmt(calibration['ece_after'])} after; Brier "
            f"{report.fmt(calibration['brier_before'])} before, "
            f"{report.fmt(calibration['brier_after'])} after.\n"
        )
    for name, rows in result["categories"].items():
        out.append(f"\n## By {name}\n\n")
        out.append(report.table([name, "rows", "success rate", "mean p", "brier", "auroc"], rows))
    out.append("\n## Policy value on matched rows\n\n")
    policy = result["policy"]
    observed, scored = [], []
    for name in ("logged", "keep only", "gated model"):
        p = policy[name]
        if p["matched"] == 0:
            observed.append([name, 0, None, None, None])
            scored.append([name, 0, None, None, None])
            continue
        observed.append([name, p["matched"], p["credits_per_correct"], p["p50_millis"],
                         p["p90_millis"]])
        scored.append([name, p["matched"], p["model_credits_per_correct"],
                       p["model_p50_millis"], p["model_p90_millis"]])
    headers = ["policy", "matched", "credits per correct", "p50 millis", "p90 millis"]
    out.append(report.table(headers, observed))
    out.append(f"\nThe gated model applied an edit on {policy['applied']} pairs.\n")
    out.append("\n### Model-scored, not observed\n\n")
    out.append(report.table(headers, scored))
    return "".join(out)


def finite_or_none(value):
    return None if isinstance(value, float) and not math.isfinite(value) else value
