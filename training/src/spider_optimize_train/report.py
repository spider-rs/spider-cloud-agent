"""Markdown for the three reports, and the label every synthetic number carries."""

from __future__ import annotations

import math

FIXTURE_ONLY = "FIXTURE-ONLY"

BANNER = (
    f"> **{FIXTURE_ONLY}.** Every number below comes from a synthetic corpus with planted "
    "effects and no fetched page. It shows whether the pipeline recovers what was planted. "
    "It says nothing about real requests and claims no improvement."
)


def banner(synthetic: bool) -> str:
    return BANNER + "\n\n" if synthetic else ""


def label(synthetic: bool) -> str:
    """The prefix for a printed line that holds a number."""
    return f"{FIXTURE_ONLY}: " if synthetic else ""


def fmt(value, digits: int = 4) -> str:
    if value is None:
        return "n/a"
    if isinstance(value, bool):
        return "yes" if value else "no"
    if isinstance(value, int):
        return str(value)
    value = float(value)
    if math.isnan(value):
        return "NaN"
    if math.isinf(value):
        return "inf" if value > 0 else "-inf"
    if abs(value) >= 1000:
        return f"{value:.0f}"
    return f"{value:.{digits}f}"


def table(headers: list[str], rows: list[list]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    for row in rows:
        lines.append("| " + " | ".join(fmt(v) if not isinstance(v, str) else v for v in row) + " |")
    return "\n".join(lines) + "\n"
