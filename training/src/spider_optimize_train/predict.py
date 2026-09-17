"""The reference reader: artifact bytes in, the three heads and the abstain flags out.

This never sees a trained model object. It parses the bytes `export` wrote, the same
way `spider-optimize` does, and computes in float32:

- A row with any non-finite input slot gets `p_success = NaN`, so the gate abstains.
  The MLP reads such a slot as 0; a GBDT split sends it the way the node's high bit
  says.
- `p_success` is the success head's logit through its calibration: identity is the
  logistic function, Platt is `sigmoid(a * z + b)`, isotonic interpolates the knots
  and holds flat past either end.
- `latency_ms` and `credits` are `expm1` of their heads after calibration, where
  identity passes the value through, Platt is `a * z + b` and isotonic interpolates.
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

    def floats(self, count: int) -> np.ndarray:
        size = 4 * count
        if self.at + size > self.end:
            raise ArtifactError("truncated")
        out = np.frombuffer(self.blob, dtype="<f4", count=count, offset=self.at).astype(np.float32)
        self.at += size
        return out


def read(blob: bytes) -> Artifact:
    blob = bytes(blob)
    if len(blob) > ex.MAX_BYTES:
        raise ArtifactError(f"size {len(blob)}")
    if len(blob) < ex.HEADER_BYTES + 4:
        raise ArtifactError("too short")
    if blob[:5] != ex.MAGIC:
        raise ArtifactError("bad magic")
    version, kind, fv, efv, sv, width, _ = struct.unpack_from("<BBHHHHB", blob, 5)
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
    body = len(blob) - 4
    (crc,) = struct.unpack_from("<I", blob, body)
    if zlib.crc32(blob[:body]) & 0xFFFFFFFF != crc:
        raise CrcError("crc mismatch")

    r = _Reader(blob, ex.HEADER_BYTES, body)
    (codes,) = r.take("<H")
    thresholds = r.floats(codes)
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
        if ckind == calibrate.IDENTITY:
            values = np.zeros(0, dtype=np.float32)
        elif ckind == calibrate.PLATT:
            values = r.floats(2)
        elif ckind == calibrate.ISOTONIC:
            if knots == 0:
                raise ArtifactError("isotonic calibration with no knots")
            values = r.floats(2 * knots)
        else:
            raise ArtifactError(f"calibration kind {ckind}")
        calibrations.append((ckind, values))

    inputs = sch.BASE_DIM + schema.edit_dim
    mlp, gbdt = [], []
    if kind == ex.KIND_MLP:
        for _ in range(3):
            (layers,) = r.take("<H")
            if layers == 0:
                raise ArtifactError("shape")
            net, cols_expected = [], inputs
            for _ in range(layers):
                rows, cols, act = r.take("<HHB")
                if cols != cols_expected or max(rows, cols) > max(width, 0) or rows > MAX_WIDTH:
                    raise ArtifactError("width" if rows > MAX_WIDTH else "shape")
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
            (base_score,) = r.take("<f")
            head = []
            for _ in range(trees):
                (nodes,) = r.take("<H")
                if nodes == 0 or nodes > width:
                    raise ArtifactError("width")
                if r.at + 14 * nodes > body:
                    raise ArtifactError("truncated")
                dt = np.dtype([("feature", "<u2"), ("threshold", "<f4"), ("left", "<u2"),
                               ("right", "<u2"), ("value", "<f4")])
                table = np.frombuffer(blob, dtype=dt, count=nodes, offset=r.at).copy()
                r.at += 14 * nodes
                head.append(table)
            gbdt.append((np.float32(base_score), head))
    if r.at != body:
        raise ArtifactError("trailing bytes")
    return Artifact(kind, fv, efv, sv, width, thresholds, support, calibrations, mlp, gbdt)


def _mlp_head(net, X: np.ndarray) -> np.ndarray:
    h = X
    for W, b, act in net:
        h = h @ W.T + b
        if act == 1:
            h = np.maximum(h, np.float32(0))
        elif act == 2:
            h = (np.float32(1) / (np.float32(1) + np.exp(-h))).astype(np.float32)
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
            if np.any(feature[~leaf] >= X.shape[1]):
                raise ArtifactError("shape")
            x = X[rows, np.minimum(feature, X.shape[1] - 1)]
            missing_left = (node["feature"] & 0x8000) != 0
            go_left = np.where(np.isfinite(x), x <= node["threshold"], missing_left)
            nxt = np.where(go_left, node["left"], node["right"]).astype(np.int64)
            if np.any(nxt[~leaf] >= len(table)):
                raise ArtifactError("shape")
            at = np.where(leaf, at, nxt)
        out = (out + table[at]["value"]).astype(np.float32)
    return out


def _calibrate(ckind: int, values: np.ndarray, z: np.ndarray, probability: bool) -> np.ndarray:
    z = z.astype(np.float32)
    if ckind == calibrate.PLATT:
        z = values[0] * z + values[1]
    elif ckind == calibrate.ISOTONIC:
        return np.interp(z, values[0::2], values[1::2]).astype(np.float32)
    if probability:
        return (np.float32(1) / (np.float32(1) + np.exp(-z))).astype(np.float32)
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
        p = _calibrate(*art.calibrations[0], raw[0], probability=True)
        p = np.where(bad, np.float32(np.nan), p).astype(np.float32)
        millis = np.expm1(_calibrate(*art.calibrations[1], raw[1], False)).astype(np.float32)
        credits = np.expm1(_calibrate(*art.calibrations[2], raw[2], False)).astype(np.float32)
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
