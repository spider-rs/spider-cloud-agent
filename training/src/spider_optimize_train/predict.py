"""The reference reader: artifact bytes in, the three heads and the abstain flags out.

This never sees a trained model object. It parses the bytes `export` wrote, the same
way `spider_optimize::artifact::Compact::from_bytes` does, refuses what that reader
refuses, and computes in float32 in the order `Compact::score_slices` does, so the two
agree to the bit on the same machine:

- A row with any non-finite input slot gets `p_success = NaN`, so the gate abstains.
  The MLP reads such a slot as 0; a GBDT split sends it the way the node's high bit
  says.
- A dense layer's output is its bias plus each weight times its input, added one input
  at a time in column order. A GBDT head is its base score plus each tree's leaf, in
  tree order.
- Every head goes through its calibration: identity passes the value through, Platt is
  `sigmoid(a * z + b)`, isotonic interpolates the knots and holds flat past either end.
  The success head's result is `p_success`, so an exporter must not give it identity.
- `latency_ms` and `credits` are `expm1` of their calibrated heads.
- `support` is 1 when the row's cell is in the support table or the table is empty,
  else 0.
- A row abstains when `p_success` is NaN, its edit code has no floor (NaN or out of
  range), `p_success` is under the floor, or `support` is 0.
"""

from __future__ import annotations

import struct
import zlib
from dataclasses import dataclass

import numpy as np

from . import calibrate
from . import export as ex
from . import schema as sch

MAX_WIDTH = 256
MAX_CODES = 10


class ArtifactError(ValueError):
    pass


class CrcError(ArtifactError):
    pass


@dataclass
class Artifact:
    kind: int
    feature_version: int
    edit_feature_version: int
    schema_version: int
    max_width: int
    thresholds: np.ndarray
    support: np.ndarray
    calibrations: list[tuple[int, np.ndarray]]
    mlp: list[list[tuple[np.ndarray, np.ndarray, int]]]
    gbdt: list[tuple[np.float32, list[np.ndarray]]]


NODE = np.dtype([("feature", "<u2"), ("threshold", "<f4"), ("left", "<u2"),
                 ("right", "<u2"), ("value", "<f4")])


class _Reader:
    def __init__(self, blob: bytes, at: int, end: int):
        self.blob, self.at, self.end = blob, at, end

    def take(self, fmt: str):
        size = struct.calcsize(fmt)
        if self.at + size > self.end:
            raise ArtifactError("truncated")
        values = struct.unpack_from(fmt, self.blob, self.at)
        self.at += size
        return values

    def floats(self, count: int, finite: bool = True) -> np.ndarray:
        size = 4 * count
        if self.at + size > self.end:
            raise ArtifactError("truncated")
        out = np.frombuffer(self.blob, dtype="<f4", count=count, offset=self.at).astype(np.float32)
        self.at += size
        if finite and not np.all(np.isfinite(out)):
            raise ArtifactError("shape: a float is not finite")
        return out


def _check_tree(table: np.ndarray, inputs: int) -> None:
    n = len(table)
    for node in table:
        if node["left"] == ex.LEAF and node["right"] == ex.LEAF:
            continue
        if int(node["feature"]) & 0x7FFF >= inputs or node["left"] >= n or node["right"] >= n:
            raise ArtifactError("shape: a split points outside its tree")
    # Colours as the Rust reader uses them: a cycle anywhere, even unreachable, refuses.
    colour = [0] * n
    for start in range(n):
        if colour[start]:
            continue
        stack = [(start, False)]
        while stack:
            at, done = stack.pop()
            if done:
                colour[at] = 2
                continue
            if colour[at] == 2:
                continue
            if colour[at] == 1:
                raise ArtifactError("shape: a tree has a cycle")
            colour[at] = 1
            stack.append((at, True))
            node = table[at]
            if not (node["left"] == ex.LEAF and node["right"] == ex.LEAF):
                # Right is pushed first so left is walked first, as the Rust reader does.
                for child in (int(node["right"]), int(node["left"])):
                    if colour[child] == 1:
                        raise ArtifactError("shape: a tree has a cycle")
                    if colour[child] == 0:
                        stack.append((child, False))


def read(blob: bytes) -> Artifact:
    blob = bytes(blob)
    if len(blob) > ex.MAX_BYTES:
        raise ArtifactError(f"size {len(blob)}")
    if len(blob) < ex.HEADER_BYTES + 4:
        raise ArtifactError("too short")
    if blob[:5] != ex.MAGIC:
        raise ArtifactError("bad magic")
    version, kind, fv, efv, sv, width, reserved = struct.unpack_from("<BBHHHHB", blob, 5)
    if version != ex.ARTIFACT_VERSION:
        raise ArtifactError(f"version {version}")
    if kind not in (ex.KIND_MLP, ex.KIND_GBDT):
        raise ArtifactError(f"kind {kind}")
    schema = sch.load()
    if fv != sch.BASE_FEATURE_VERSION:
        raise ArtifactError(f"feature version {fv}")
    if efv != schema.edit_feature_version:
        raise ArtifactError(f"edit feature version {efv}")
    if sv != schema.schema_version:
        raise ArtifactError(f"schema version {sv}")
    if width == 0 or width > MAX_WIDTH:
        raise ArtifactError(f"width {width}")
    if reserved != 0:
        raise ArtifactError("shape: reserved header byte is set")
    body = len(blob) - 4
    (crc,) = struct.unpack_from("<I", blob, body)
    if zlib.crc32(blob[:body]) & 0xFFFFFFFF != crc:
        raise CrcError("crc mismatch")

    r = _Reader(blob, ex.HEADER_BYTES, body)
    (codes,) = r.take("<H")
    if not 1 <= codes <= MAX_CODES:
        raise ArtifactError(f"shape: {codes} thresholds")
    thresholds = r.floats(codes, finite=False)
    if np.any(~np.isnan(thresholds) & ((thresholds < 0) | (thresholds > 1))):
        raise ArtifactError("shape: a threshold is outside [0, 1] and not NaN")
    (support_len,) = r.take("<I")
    if support_len * 4 > body - r.at:
        raise ArtifactError("truncated")
    support = np.frombuffer(blob, dtype="<u4", count=support_len, offset=r.at).astype(np.int64)
    r.at += 4 * support_len
    if np.any(np.diff(support) <= 0):
        raise ArtifactError("support table is not sorted")
    calibrations = []
    for _ in range(3):
        ckind, knots = r.take("<HH")
        if (ckind, knots) == (calibrate.IDENTITY, 0):
            values = np.zeros(0, dtype=np.float32)
        elif (ckind, knots) == (calibrate.PLATT, 0):
            values = r.floats(2)
        elif ckind == calibrate.ISOTONIC and knots >= 2:
            values = r.floats(2 * knots)
            xs, ys = values[0::2], values[1::2]
            if np.any(np.diff(xs) <= 0) or np.any(np.diff(ys) < 0):
                raise ArtifactError("shape: isotonic knots are out of order")
        else:
            raise ArtifactError(f"shape: calibration kind {ckind} with {knots} knots")
        calibrations.append((ckind, values))

    inputs = sch.BASE_DIM + schema.edit_dim
    largest = 0
    mlp, gbdt = [], []
    if kind == ex.KIND_MLP:
        for _ in range(3):
            (layers,) = r.take("<H")
            if layers == 0:
                raise ArtifactError("shape")
            net, cols_expected = [], inputs
            for _ in range(layers):
                rows, cols, act = r.take("<HHB")
                if rows == 0 or rows > width or cols != cols_expected or cols > MAX_WIDTH \
                        or act > 2:
                    raise ArtifactError("shape")
                largest = max(largest, rows, cols)
                W = r.floats(rows * cols).reshape(rows, cols)
                b = r.floats(rows)
                net.append((W, b, act))
                cols_expected = rows
            if cols_expected != 1:
                raise ArtifactError("shape")
            mlp.append(net)
    else:
        for _ in range(3):
            (trees,) = r.take("<I")
            (base_score,) = r.floats(1)
            head = []
            for _ in range(trees):
                (nodes,) = r.take("<H")
                if nodes == 0 or nodes > width:
                    raise ArtifactError("width")
                largest = max(largest, nodes)
                if r.at + 14 * nodes > body:
                    raise ArtifactError("truncated")
                table = np.frombuffer(blob, dtype=NODE, count=nodes, offset=r.at).copy()
                r.at += 14 * nodes
                if not (np.all(np.isfinite(table["threshold"]))
                        and np.all(np.isfinite(table["value"]))):
                    raise ArtifactError("shape: a node float is not finite")
                _check_tree(table, inputs)
                head.append(table)
            gbdt.append((np.float32(base_score), head))
    if largest != width:
        raise ArtifactError(f"shape: widest table is {largest}, header says {width}")
    if r.at != body:
        raise ArtifactError("trailing bytes")
    return Artifact(kind, fv, efv, sv, width, thresholds, support, calibrations, mlp, gbdt)


def _sigmoid32(z: np.ndarray) -> np.ndarray:
    one = np.float32(1)
    return (one / (one + np.exp(-z))).astype(np.float32)


def _dense(W: np.ndarray, b: np.ndarray, x: np.ndarray) -> np.ndarray:
    """`b + sum_j W[:, j] * x[:, j]`, added in column order in float32."""
    out = np.tile(b.astype(np.float32), (len(x), 1))
    for j in range(W.shape[1]):
        out += W[:, j] * x[:, j : j + 1]
    return out


def _mlp_head(net, X: np.ndarray) -> np.ndarray:
    h = X.astype(np.float32)
    for W, b, act in net:
        h = _dense(W, b, h)
        if act == 1:
            h = np.maximum(h, np.float32(0))
        elif act == 2:
            h = _sigmoid32(h)
    return h[:, 0].astype(np.float32)


def _gbdt_head(base_score, trees, X: np.ndarray) -> np.ndarray:
    n = len(X)
    rows = np.arange(n)
    out = np.full(n, base_score, dtype=np.float32)
    for table in trees:
        at = np.zeros(n, dtype=np.int64)
        for _ in range(len(table)):
            node = table[at]
            leaf = (node["left"] == 0xFFFF) & (node["right"] == 0xFFFF)
            if leaf.all():
                break
            feature = (node["feature"] & 0x7FFF).astype(np.int64)
            x = X[rows, np.minimum(feature, X.shape[1] - 1)]
            missing_left = (node["feature"] & 0x8000) != 0
            go_left = np.where(np.isfinite(x), x <= node["threshold"], missing_left)
            nxt = np.where(go_left, node["left"], node["right"]).astype(np.int64)
            at = np.where(leaf, at, nxt)
        out = (out + table[at]["value"]).astype(np.float32)
    return out


def _isotonic(values: np.ndarray, z: np.ndarray) -> np.ndarray:
    """Linear interpolation in float32 with the Rust reader's arithmetic:
    `a.y + (b.y - a.y) * ((z - a.x) / (b.x - a.x))` on the first knot `b` with
    `z <= b.x`, flat past either end."""
    xs, ys = values[0::2], values[1::2]
    right = np.searchsorted(xs, z, side="left")
    inner = np.clip(right, 1, len(xs) - 1)
    ax, ay, bx, by = xs[inner - 1], ys[inner - 1], xs[inner], ys[inner]
    out = (ay + (by - ay) * ((z - ax) / (bx - ax))).astype(np.float32)
    out = np.where(right == 0, ys[0], out)
    out = np.where(right >= len(xs), ys[-1], out)
    return out.astype(np.float32)


def _calibrate(ckind: int, values: np.ndarray, z: np.ndarray) -> np.ndarray:
    z = z.astype(np.float32)
    if ckind == calibrate.PLATT:
        return _sigmoid32((values[0] * z + values[1]).astype(np.float32))
    if ckind == calibrate.ISOTONIC:
        return _isotonic(values, z)
    return z


@dataclass
class Prediction:
    p_success: np.ndarray
    latency_ms: np.ndarray
    credits: np.ndarray
    support: np.ndarray
    abstain: np.ndarray


def predict(blob_or_artifact, base, edit, cells) -> Prediction:
    art = blob_or_artifact if isinstance(blob_or_artifact, Artifact) else read(blob_or_artifact)
    base = np.atleast_2d(np.asarray(base, dtype=np.float32))
    edit = np.atleast_2d(np.asarray(edit, dtype=np.float32))
    cells = np.atleast_1d(np.asarray(cells, dtype=np.int64))
    X = np.concatenate([base, edit], axis=1).astype(np.float32)
    bad = ~np.all(np.isfinite(X), axis=1)
    with np.errstate(over="ignore", invalid="ignore"):
        if art.kind == ex.KIND_MLP:
            clean = np.where(np.isfinite(X), X, np.float32(0))
            raw = [_mlp_head(net, clean) for net in art.mlp]
        else:
            raw = [_gbdt_head(b, trees, X) for b, trees in art.gbdt]
        p = _calibrate(*art.calibrations[0], raw[0])
        p = np.where(bad, np.float32(np.nan), p).astype(np.float32)
        millis = np.expm1(_calibrate(*art.calibrations[1], raw[1])).astype(np.float32)
        credits = np.expm1(_calibrate(*art.calibrations[2], raw[2])).astype(np.float32)
    if len(art.support) == 0:
        support = np.ones(len(X), dtype=np.float32)
    else:
        support = np.isin(cells, art.support).astype(np.float32)
    code = cells & 0xFF
    floors = np.concatenate([art.thresholds, [np.float32(np.nan)]])
    floor = floors[np.minimum(code, len(art.thresholds))]
    with np.errstate(invalid="ignore"):
        abstain = np.isnan(p) | np.isnan(floor) | (p < floor) | (support == 0)
    return Prediction(p, millis, credits, support, abstain)
