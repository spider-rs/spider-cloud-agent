"""Reading a corpus and refusing a bad one.

A corpus is a directory with `rows.jsonl`, one comparison row per line as
`spider_optimize::row::comparison_row` writes it, and `manifest.json`. The validator
returns every violation it finds rather than the first, so one run shows everything
a collector got wrong.
"""

from __future__ import annotations

import json
import math
import re
from collections import Counter, defaultdict
from dataclasses import asdict, dataclass, field
from pathlib import Path

from . import schema as sch

MIN_ROWS = 200
MIN_PAIRS = 100
MIN_CODE_ROWS = 20

# Anything that looks like a host or an address. A row holds labels from a fixed
# vocabulary, so a match is a leak whatever the field.
HOST_LIKE = re.compile(r"[a-z0-9-]+\.[a-z]{2,}")


@dataclass
class Manifest:
    schema_version: int
    feature_version: int
    edit_feature_version: int
    edit_dim: int
    rows: int
    pairs: int
    day_min: int
    day_max: int
    collector_rev: str
    client_version: str
    service_revision: str
    credits_spent: float
    tau: float
    salt_id: str
    synthetic: bool
    planted: dict = field(default_factory=dict)

    @classmethod
    def read(cls, directory: Path) -> Manifest:
        doc = json.loads((Path(directory) / "manifest.json").read_text())
        names = cls.__dataclass_fields__.keys()
        return cls(**{name: doc[name] for name in names if name in doc})

    def write(self, directory: Path) -> None:
        text = json.dumps(asdict(self), indent=2, sort_keys=True) + "\n"
        (Path(directory) / "manifest.json").write_text(text)


def read_rows(directory: Path) -> list[dict]:
    rows = []
    with open(Path(directory) / "rows.jsonl") as lines:
        for line in lines:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def pairs_of(rows: list[dict]) -> dict[int, list[dict]]:
    grouped: dict[int, list[dict]] = defaultdict(list)
    for row in rows:
        grouped[row["pair"]].append(row)
    return grouped


def _strings(value, path: str):
    if isinstance(value, str):
        yield path, value
    elif isinstance(value, dict):
        for key, inner in value.items():
            yield from _strings(key, f"{path}.<key>")
            yield from _strings(inner, f"{path}.{key}")
    elif isinstance(value, list):
        for at, inner in enumerate(value):
            yield from _strings(inner, f"{path}[{at}]")


def _unit_violation(values, width: int, name: str) -> str | None:
    if not isinstance(values, list) or len(values) != width:
        got = len(values) if isinstance(values, list) else type(values).__name__
        return f"{name} has length {got}, expected {width}"
    for at, value in enumerate(values):
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            return f"{name}[{at}] is not a number"
        if not math.isfinite(value) or not -1.0 <= value <= 1.0:
            return f"{name}[{at}] is {value}, outside [-1, 1] or not finite"
    return None


def validate_rows(rows: list[dict], manifest: Manifest) -> list[str]:
    """Every violation in a set of rows, as sentences. Empty means valid."""
    schema = sch.load()
    out: list[str] = []

    if len(rows) < MIN_ROWS:
        out.append(f"too few rows: {len(rows)} < {MIN_ROWS}")
    pairs = pairs_of(rows)
    if len(pairs) < MIN_PAIRS:
        out.append(f"too few pairs: {len(pairs)} < {MIN_PAIRS}")

    expected_versions = {
        "schema_v": manifest.schema_version,
        "feat_v": manifest.feature_version,
        "edit_feat_v": manifest.edit_feature_version,
    }
    if manifest.schema_version != schema.schema_version:
        out.append(
            f"manifest schema_version {manifest.schema_version} is not "
            f"schema-v1.json's {schema.schema_version}"
        )
    if manifest.edit_dim != schema.edit_dim:
        out.append(f"manifest edit_dim {manifest.edit_dim} is not {schema.edit_dim}")

    for number, row in enumerate(rows, start=1):
        where = f"row {number} (pair {row.get('pair')})"
        for name, want in expected_versions.items():
            if row.get(name) != want:
                out.append(f"{where}: version {name} {row.get(name)} differs from manifest {want}")
        for path, text in _strings(row, "row"):
            lowered = text.lower()
            if "://" in lowered or HOST_LIKE.search(lowered):
                out.append(f"{where}: host-like string at {path}")
        problem = _unit_violation(row.get("base"), sch.BASE_DIM, "base")
        if problem:
            out.append(f"{where}: {problem}")
        problem = _unit_violation(row.get("edit_feats"), manifest.edit_dim, "edit_feats")
        if problem:
            out.append(f"{where}: {problem}")

    for pair, arms in sorted(pairs.items()):
        baselines = sum(1 for arm in arms if arm.get("arm") == "baseline")
        if baselines != 1:
            out.append(f"pair {pair}: {baselines} baseline arms, expected exactly one")
        if len({arm.get("day") for arm in arms}) > 1:
            out.append(f"pair {pair}: arms on different day")
        if len({arm.get("dk") for arm in arms}) > 1:
            out.append(f"pair {pair}: arms on different dk")

    counts: Counter[int] = Counter()
    outcomes: dict[int, set[bool]] = defaultdict(set)
    for row in rows:
        if row.get("arm") == "baseline" or row.get("edit") is None:
            continue
        code = sch.row_code(row, schema)
        if code > 0:
            counts[code] += 1
            outcomes[code].add(bool(row.get("success")))
    for code in range(1, schema.edit_codes):
        name = schema.code_name(code)
        if counts[code] < MIN_CODE_ROWS:
            out.append(
                f"edit code {code} ({name}): {counts[code]} candidate rows < {MIN_CODE_ROWS}"
            )
        elif len(outcomes[code]) < 2:
            out.append(f"edit code {code} ({name}): only one success outcome")
    return out


def validate(directory: Path) -> list[str]:
    directory = Path(directory)
    for name in ("rows.jsonl", "manifest.json"):
        if not (directory / name).is_file():
            return [f"{name} is missing from {directory}"]
    return validate_rows(read_rows(directory), Manifest.read(directory))
