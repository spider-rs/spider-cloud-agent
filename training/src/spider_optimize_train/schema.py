"""The layout and vocabulary the rows and the artifact follow.

The edit feature blocks, the keys, the needs and the status classes come from
`training/fixtures/schema-v1.json`, which `spider-optimize` regenerates from its own
constants and checks in a test, so this module never restates them. The router's
base slots are not in that file; the handful the synthetic corpus writes are copied
from `spider-route/src/features.rs` below, with the arithmetic that places them.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path

TRAINING_ROOT = Path(__file__).resolve().parents[2]
SCHEMA_PATH = TRAINING_ROOT / "fixtures" / "schema-v1.json"

# The router's feature layout, version 1. TLD_SLOTS is 30, which is what makes
# the used slots come to 152.
BASE_FEATURE_VERSION = 1
BASE_DIM = 152
BASE_PATH_DEPTH = 0
BASE_EXTENSION = 23
BASE_TLD = 69
BASE_TLD_W = 30
BASE_NEED = 105
BASE_PINS = 113
BASE_MEM_COUNT = 124
BASE_MEM_STATUS = 142
BASE_BIAS = 151

# The row's `ext` label in the router's slot order.
EXT_LABELS = (
    "none",
    "markup",
    "xml",
    "json",
    "feed",
    "text",
    "csv",
    "pdf",
    "office",
    "image",
    "media",
    "archive",
    "asset",
    "other",
)
MEMORY_LABELS = ("cold", "thin", "warm", "steady")
ARM_LABELS = ("baseline", "candidate", "shadow")

# The idle waits an edit may set, and the mode and pool orders an edit's value
# bucket indexes (`spider-optimize/src/schema.rs`).
WAIT_BUCKETS = (0, 2_000, 5_000, 10_000)
MODES = ("http", "smart", "browser")
PROXIES = ("isp", "residential")

KEEP_CODE = 0
HEADS = ("success", "millis", "credits")


@dataclass(frozen=True)
class Schema:
    schema_version: int
    edit_feature_version: int
    edit_dim: int
    blocks: dict[str, tuple[int, int]]
    keys: tuple[dict, ...]
    needs: tuple[str, ...]
    status: tuple[str, ...]
    ops: tuple[str, ...]

    @property
    def learnable(self) -> tuple[dict, ...]:
        return tuple(k for k in self.keys if k["learnable"])

    def block(self, name: str) -> int:
        return self.blocks[name][0]

    def key_index(self, wire: str) -> int:
        for key in self.keys:
            if key["wire"] == wire:
                return key["index"]
        raise KeyError(wire)

    def edit_code(self, key_index: int | None) -> int | None:
        """The compact code of a key: 0 for keep (`None`), `1 + rank` among the
        learnable keys in key order, or `None` for a key that is not learnable.

        The same rule as `spider_optimize::schema::edit_code`.
        """
        if key_index is None:
            return KEEP_CODE
        for rank, key in enumerate(self.learnable):
            if key["index"] == key_index:
                return 1 + rank
        return None

    @property
    def edit_codes(self) -> int:
        """How many threshold slots an artifact holds: keep plus each learnable key."""
        return 1 + len(self.learnable)

    def code_name(self, code: int) -> str:
        if code == KEEP_CODE:
            return "keep"
        return self.learnable[code - 1]["wire"]


@lru_cache(maxsize=1)
def load(path: Path = SCHEMA_PATH) -> Schema:
    doc = json.loads(Path(path).read_text())
    return Schema(
        schema_version=doc["schema_version"],
        edit_feature_version=doc["edit_feature_version"],
        edit_dim=doc["edit_dim"],
        blocks={b["name"]: (b["base"], b["width"]) for b in doc["blocks"]},
        keys=tuple(doc["keys"]),
        needs=tuple(doc["needs"]),
        status=tuple(doc["status"]),
        ops=tuple(o["name"] for o in sorted(doc["ops"], key=lambda o: o["code"])),
    )


def cell_id(need: int, ext: int, mem: int, edit_code: int) -> int:
    """`need << 24 | ext << 16 | mem << 8 | edit_code`, each field under 256."""
    for value in (need, ext, mem, edit_code):
        if not 0 <= value < 256:
            raise ValueError(f"cell field {value} is outside 0..255")
    return (need << 24) | (ext << 16) | (mem << 8) | edit_code


def row_cell(row: dict, schema: Schema | None = None) -> int:
    schema = schema or load()
    return cell_id(
        schema.needs.index(row["need"]),
        EXT_LABELS.index(row["ext"]),
        MEMORY_LABELS.index(row["mem"]),
        row_code(row, schema),
    )


def row_code(row: dict, schema: Schema | None = None) -> int:
    """The edit code a row carries. A row whose key is not learnable reads as -1."""
    schema = schema or load()
    edit = row.get("edit")
    code = schema.edit_code(None if edit is None else edit["key"])
    return -1 if code is None else code
