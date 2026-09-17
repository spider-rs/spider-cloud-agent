"""The matrix both model kinds are trained on.

`X = base[:152] ++ edit_feats ++ pinned_bucket(4)`. The pinned bucket counts the
fields the caller set, from the row's two masks. The Rust `Input` carries no such
count and the optimizer only edits a request whose caller left the field alone, so
`export` folds the bucket in at zero pins (see `export.fold_pinned`).
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from . import labels
from . import schema as sch

PINNED_W = 4


def pinned_count(pinned: int, pinned_hi: int) -> int:
    return int(pinned).bit_count() + int(pinned_hi).bit_count()


def pinned_bucket(pinned: int, pinned_hi: int) -> int:
    """0, 1 to 2, 3 to 5, or 6 and more pinned fields."""
    count = pinned_count(pinned, pinned_hi)
    if count == 0:
        return 0
    if count <= 2:
        return 1
    if count <= 5:
        return 2
    return 3


def input_dim(edit_dim: int | None = None) -> int:
    return sch.BASE_DIM + (edit_dim or sch.load().edit_dim) + PINNED_W


def row_vector(row: dict, edit_dim: int | None = None) -> np.ndarray:
    edit_dim = edit_dim or sch.load().edit_dim
    x = np.zeros(input_dim(edit_dim), dtype=np.float32)
    x[: sch.BASE_DIM] = row["base"][: sch.BASE_DIM]
    x[sch.BASE_DIM : sch.BASE_DIM + edit_dim] = row["edit_feats"]
    x[sch.BASE_DIM + edit_dim + pinned_bucket(row["pinned"], row["pinned_hi"])] = 1.0
    return x


@dataclass
class Arrays:
    """A window as arrays, with the row-level columns every later step needs."""

    X: np.ndarray
    success: np.ndarray  # the `labels.correct` label, 0 or 1
    log_millis: np.ndarray
    log_credits: np.ndarray
    pair: np.ndarray
    day: np.ndarray
    dk: np.ndarray
    baseline: np.ndarray  # bool, the row is its pair's baseline arm
    code: np.ndarray
    cell: np.ndarray
    need: np.ndarray
    ext: np.ndarray
    mem: np.ndarray
    tld: np.ndarray
    rows: list[dict]

    def __len__(self) -> int:
        return len(self.rows)

    def take(self, mask: np.ndarray) -> Arrays:
        idx = np.flatnonzero(mask)
        fields = {
            name: getattr(self, name)[idx]
            for name in self.__dataclass_fields__
            if name != "rows"
        }
        return Arrays(**fields, rows=[self.rows[i] for i in idx])


def build(rows: list[dict], tau: float = labels.DEFAULT_TAU) -> Arrays:
    schema = sch.load()
    n = len(rows)
    X = np.zeros((n, input_dim(schema.edit_dim)), dtype=np.float32)
    for i, row in enumerate(rows):
        X[i] = row_vector(row, schema.edit_dim)
    if not np.all(np.isfinite(X)):
        raise ValueError("a feature is not finite; validate the corpus first")
    return Arrays(
        X=X,
        success=np.array([labels.correct(r, tau) for r in rows], dtype=np.float32),
        log_millis=np.log1p(np.array([r["millis"] for r in rows], dtype=np.float64)),
        log_credits=np.log1p(np.array([r["credits"] for r in rows], dtype=np.float64)),
        pair=np.array([r["pair"] for r in rows], dtype=np.uint64),
        day=np.array([r["day"] for r in rows], dtype=np.int64),
        dk=np.array([r["dk"] for r in rows], dtype=np.uint64),
        baseline=np.array([r["arm"] == "baseline" for r in rows], dtype=bool),
        code=np.array([sch.row_code(r, schema) for r in rows], dtype=np.int64),
        cell=np.array([sch.row_cell(r, schema) for r in rows], dtype=np.int64),
        need=np.array([r["need"] for r in rows], dtype=object),
        ext=np.array([r["ext"] for r in rows], dtype=object),
        mem=np.array([r["mem"] for r in rows], dtype=object),
        tld=np.array([r["tld"] for r in rows], dtype=np.int64),
        rows=list(rows),
    )
