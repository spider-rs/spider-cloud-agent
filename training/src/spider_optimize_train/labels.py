"""Labels recomputed from a row's stored scalars.

The collector writes `content_ok` with the tau it had. The trainer recomputes it from
`shingle_jaccard`, `byte_ratio` and the field counts, so a corpus can be relabelled
with a different tau without being collected again.
"""

from __future__ import annotations

import numpy as np

DEFAULT_TAU = 0.80
MIN_REPEATS = 20
BODY_NEEDS = ("text", "markdown", "html")


def content_ok(row: dict, tau: float = DEFAULT_TAU) -> bool | None:
    """Whether an arm's content matched the baseline's, or `None` when that cannot be
    judged: a screenshot or raw need, or a body with nothing stored to compare."""
    need = row["need"]
    if need in ("screenshot", "raw"):
        return None
    if row["status"] != "ok":
        return False
    if need == "fields":
        return row.get("fields_ok")
    if need in ("links", "metadata"):
        requested = row.get("fields_requested") or 0
        if requested == 0:
            return None
        return (row.get("fields_present") or 0) / requested >= 0.9
    jaccard = row.get("shingle_jaccard")
    ratio = row.get("byte_ratio")
    if jaccard is None or ratio is None:
        return None
    return jaccard >= tau and 0.5 <= ratio <= 2.0


def correct(row: dict, tau: float = DEFAULT_TAU) -> bool:
    """The success label a scorer is trained on: the arm succeeded and its content was
    not judged broken."""
    return bool(row["success"]) and content_ok(row, tau) is not False


def regressed(candidate: dict, baseline: dict, tau: float = DEFAULT_TAU) -> bool:
    """The baseline succeeded and the candidate did not, or the candidate came back with
    content judged broken. A candidate that failed beside a failed baseline lost
    nothing, so it is not a regression."""
    if baseline["success"] and not candidate["success"]:
        return True
    return bool(candidate["success"]) and content_ok(candidate, tau) is False


def repeat_jaccards(rows: list[dict]) -> list[float]:
    """`shingle_jaccard` of every baseline-versus-baseline repeat: a pair whose second
    arm also kept the request."""
    out = []
    for row in rows:
        if row["arm"] != "baseline" and row.get("edit") is None:
            value = row.get("shingle_jaccard")
            if value is not None:
                out.append(float(value))
    return out


def choose_tau(rows: list[dict]) -> tuple[float, str]:
    """Tau at the 5th percentile of repeat jaccard, so 95 percent of pages fetched twice
    with nothing changed count as matching themselves."""
    values = repeat_jaccards(rows)
    if len(values) < MIN_REPEATS:
        return DEFAULT_TAU, (
            f"only {len(values)} baseline repeats (< {MIN_REPEATS}), "
            f"kept the default tau {DEFAULT_TAU}"
        )
    tau = float(np.percentile(np.asarray(values), 5))
    return tau, f"tau {tau:.4f} is the 5th percentile of {len(values)} baseline repeats"
