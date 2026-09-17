"""The artifact `spider-optimize` reads, written from either model kind.

Version 1, little endian throughout:

    header, 16 bytes: b"SPOPT", u8 artifact_version = 1, u8 kind (1 MLP, 2 GBDT),
        u16 feature_version, u16 edit_feature_version, u16 schema_version,
        u16 max_width, u8 reserved
    u16 edit_codes, f32 threshold[edit_codes]         NaN abstains; index = edit code
    u32 support_len, u32 cell[support_len]            sorted ascending
    3 x calibration: u16 kind, u16 knots, f32 ...     (0, 0) identity; (1, 0) Platt, one
                                                      (a, b); (2, knots >= 2) isotonic,
                                                      knots x (x, y), x strictly rising,
                                                      y never falling
    MLP, 3 x net: u16 layers, then per layer u16 rows, u16 cols, u8 activation,
        f32 weight[rows * cols] row-major, f32 bias[rows]
    GBDT, 3 x head: u32 trees, f32 base_score, then per tree u16 nodes, then per node
        u16 feature (high bit: a non-finite input goes left), f32 threshold,
        u16 left, u16 right, f32 value               leaf when left == right == 0xFFFF
    u32 crc32 of every byte before it

The heads are success (a logit), log1p millis and log1p credits, in that order. Every
float but a threshold must be finite; a threshold is NaN or in [0, 1]. The success head
never carries an identity calibration, because the reader would pass its logit through
as a probability.

No run of printable ASCII after the header may be longer than 8 bytes, and none may look
like a host. A float's lowest byte is the only one this writer changes to break a run,
which moves a weight by at most 48 units in its last place; `predict` reads the bytes
that were written, so the golden cases carry the change. Calibration floats are never
changed, since a nudged knot could break the order the reader requires.
"""

from __future__ import annotations

import hashlib
import json
import math
import re
import struct
import zlib
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import calibrate
from . import features as feat
from . import schema as sch
from .models.lightgbm_head import LEAF

MAGIC = b"SPOPT"
ARTIFACT_VERSION = 1
KIND_MLP = 1
KIND_GBDT = 2
HEADER_BYTES = 16
MAX_ASCII_RUN = 8
MAX_BYTES = 2_000_000
GBDT_CAP = 1_500_000
MISSING_LEFT = 0x8000
DOMAIN_LIKE = re.compile(rb"[a-z0-9-]+\.[a-z]{2,}")


@dataclass
class Tables:
    """Everything an artifact holds, as plain values."""

    kind: int
    thresholds: list[float]
    support: list[int]
    calibrations: list[tuple[int, list[float]]]  # (kind, flat floats) per head
    mlp: list[list[tuple[np.ndarray, np.ndarray, int]]] = field(default_factory=list)
    gbdt: list[tuple[float, list[list[tuple]]]] = field(default_factory=list)
    feature_version: int = sch.BASE_FEATURE_VERSION
    edit_feature_version: int = 1
    schema_version: int = 1


def calibration_entry(cal: calibrate.Calibration) -> tuple[int, list[float]]:
    if cal.kind == calibrate.PLATT:
        return calibrate.PLATT, [float(v) for v in cal.params]
    if cal.kind == calibrate.ISOTONIC:
        if len(cal.knots) < 2:
            raise ValueError("an isotonic calibration needs at least two knots")
        return calibrate.ISOTONIC, [float(v) for knot in cal.knots for v in knot]
    return calibrate.IDENTITY, []


def fold_pinned_mlp(layers, edit_dim: int):
    """Drop the pinned bucket columns from the first layer, adding the zero-pins column
    into the bias."""
    first = sch.BASE_DIM + edit_dim
    W, b, act = layers[0]
    folded = [(W[:, :first].copy(), b + W[:, first], act)]
    return folded + [(W.copy(), b.copy(), act) for W, b, act in layers[1:]]


# The reader requires finite thresholds. Every input slot lies in [-1, 1], so a split on
# slot 0 at 2.0 always goes left and one at -2.0 always goes right; the missing bit is
# set to match, so a non-finite slot goes the same way.
ALWAYS_LEFT = 2.0
ALWAYS_RIGHT = -2.0


def fold_pinned_tree(nodes, edit_dim: int):
    """Resolve every split on a pinned bucket column as zero pins would: bucket 0 reads
    1, the others 0. The split becomes one that always goes the same way."""
    first = sch.BASE_DIM + edit_dim
    out = []
    for feature, default_left, threshold, left, right, value in nodes:
        if left != LEAF and feature >= first:
            x = 1.0 if feature == first else 0.0
            goes_left = x <= threshold
            threshold = ALWAYS_LEFT if goes_left else ALWAYS_RIGHT
            feature, default_left = 0, goes_left
        out.append((feature, default_left, threshold, left, right, value))
    return out


def tables_for(model, cal: calibrate.Calibration, thresholds: list[float],
               support: list[int]) -> Tables:
    schema = sch.load()
    if cal.kind == calibrate.IDENTITY:
        raise ValueError(
            "the success head needs a Platt or isotonic calibration; the reader passes an "
            "identity-calibrated logit through as the probability"
        )
    cals = [calibration_entry(cal), (calibrate.IDENTITY, []), (calibrate.IDENTITY, [])]
    common = dict(
        thresholds=[float(t) for t in thresholds],
        support=sorted(int(c) for c in support),
        calibrations=cals,
        edit_feature_version=schema.edit_feature_version,
        schema_version=schema.schema_version,
    )
    if model.kind == "mlp":
        heads = [fold_pinned_mlp(layers, schema.edit_dim) for layers in model.to_tables()]
        return Tables(KIND_MLP, mlp=heads, **common)
    heads = [(0.0, [fold_pinned_tree(t, schema.edit_dim) for t in trees])
             for trees in model.to_tables()]
    return Tables(KIND_GBDT, gbdt=heads, **common)


class _Writer:
    def __init__(self):
        self.buf = bytearray()
        self.floats: list[int] = []

    def u8(self, v):
        self.buf += struct.pack("<B", v)

    def u16(self, v):
        self.buf += struct.pack("<H", v)

    def u32(self, v):
        self.buf += struct.pack("<I", v)

    def f32(self, v):
        self.floats.append(len(self.buf))
        self.buf += struct.pack("<f", np.float32(v))

    def f32s(self, values, adjustable: bool = True):
        """Append floats. Only `adjustable` ones may have a byte moved to break a run."""
        values = np.asarray(values, dtype="<f4").ravel()
        start = len(self.buf)
        if adjustable:
            self.floats.extend(range(start, start + 4 * len(values), 4))
        self.buf += values.tobytes()


def max_width(t: Tables) -> int:
    if t.kind == KIND_MLP:
        return max(max(W.shape) for layers in t.mlp for W, _, _ in layers)
    return max((len(tree) for _, trees in t.gbdt for tree in trees), default=1)


def _body(t: Tables) -> _Writer:
    w = _Writer()
    w.buf += MAGIC
    w.u8(ARTIFACT_VERSION)
    w.u8(t.kind)
    w.u16(t.feature_version)
    w.u16(t.edit_feature_version)
    w.u16(t.schema_version)
    w.u16(max_width(t))
    w.u8(0)
    assert len(w.buf) == HEADER_BYTES

    w.u16(len(t.thresholds))
    w.f32s(t.thresholds)
    w.u32(len(t.support))
    for cell in t.support:
        w.u32(cell)
    for kind, values in t.calibrations:
        w.u16(kind)
        w.u16(len(values) // 2 if kind == calibrate.ISOTONIC else 0)
        w.f32s(values, adjustable=False)

    if t.kind == KIND_MLP:
        for layers in t.mlp:
            w.u16(len(layers))
            for W, b, act in layers:
                rows, cols = W.shape
                w.u16(rows)
                w.u16(cols)
                w.u8(act)
                w.f32s(W)
                w.f32s(b)
    else:
        for base_score, trees in t.gbdt:
            w.u32(len(trees))
            w.f32(base_score)
            for nodes in trees:
                w.u16(len(nodes))
                for feature, default_left, threshold, left, right, value in nodes:
                    w.u16(int(feature) | (MISSING_LEFT if default_left else 0))
                    w.f32(threshold)
                    w.u16(int(left))
                    w.u16(int(right))
                    w.f32(value)
    return w


def ascii_runs(blob: bytes, min_len: int, start: int = HEADER_BYTES):
    """(offset, length) of each run of printable ASCII at least `min_len` long, from
    `start` on. A run is cut at `start`, as the leak check cuts it at the header."""
    pattern = re.compile(rb"[\x20-\x7e]{%d,}" % min_len)
    return [(m.start() + start, m.end() - m.start()) for m in pattern.finditer(blob[start:])]


def violations(blob: bytes) -> list[tuple[int, int]]:
    bad = []
    for at, length in ascii_runs(blob, 4):
        text = bytes(blob[at : at + length]).lower()
        if length > MAX_ASCII_RUN or DOMAIN_LIKE.search(text):
            bad.append((at, length))
    return bad


def _break_run(buf: bytearray, floats: list[int], at: int, length: int) -> None:
    """Move the lowest byte of the float nearest the middle of a run out of the
    printable range."""
    middle = at + length / 2.0
    inside = [o for o in floats if at <= o < at + length]
    if not inside:
        raise ValueError(f"a printable run at byte {at} holds no float to adjust")
    o = min(inside, key=lambda o: abs(o - middle))
    b = buf[o]
    buf[o] = 0x1F if b - 0x1F <= 0x7F - b else 0x7F


def serialize(t: Tables) -> bytes:
    w = _body(t)
    buf = w.buf
    for _ in range(1_000):
        blob = bytes(buf) + struct.pack("<I", zlib.crc32(bytes(buf)) & 0xFFFFFFFF)
        bad = violations(blob)
        if not bad:
            break
        for at, length in bad:
            _break_run(buf, w.floats, at, length)
    else:
        raise ValueError("could not clear the printable runs")
    audit(blob)
    return blob


def audit(blob: bytes) -> None:
    """Refuse a blob that would fail the repository's artifact leak check, or that the
    reader would refuse: its size limit, and every structural rule, read from the final
    bytes after any run was broken."""
    from . import predict

    for at, length in ascii_runs(blob, MAX_ASCII_RUN + 1):
        raise ValueError(f"printable run of {length} bytes at byte {at}")
    for at, length in ascii_runs(blob, 4):
        if DOMAIN_LIKE.search(bytes(blob[at : at + length]).lower()):
            raise ValueError(f"domain-like string at byte {at}")
    if len(blob) > MAX_BYTES:
        raise ValueError(f"artifact is {len(blob)} bytes, over {MAX_BYTES}")
    predict.read(blob)


def artifact_size(model, thresholds: int = 10, support: int = 0, knots: int = 0) -> int:
    """The size the artifact would have, without writing it."""
    size = HEADER_BYTES + 2 + 4 * thresholds + 4 + 4 * support + 3 * 4 + 8 * knots + 4 + 4
    if model.kind == "mlp":
        for layers in model.to_tables():
            size += 2
            for W, b, _ in layers:
                size += 5 + 4 * W.size + 4 * b.size
    else:
        for trees in model.to_tables():
            size += 8 + sum(2 + 14 * len(nodes) for nodes in trees)
    return size


def sha256(blob: bytes) -> str:
    return hashlib.sha256(blob).hexdigest()


def write_sidecar(path: Path, blob: bytes, t: Tables, info: dict) -> None:
    doc = {
        "artifact_version": ARTIFACT_VERSION,
        "kind": {KIND_MLP: "mlp", KIND_GBDT: "gbdt"}[t.kind],
        "feature_version": t.feature_version,
        "edit_feature_version": t.edit_feature_version,
        "schema_version": t.schema_version,
        "bytes": len(blob),
        "sha256": sha256(blob),
        "thresholds": [None if math.isnan(v) else v for v in t.thresholds],
        "support_cells": len(t.support),
        **info,
    }
    Path(path).write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")


GOLDEN_CASES = 64
# JSON has no infinity or NaN. A non-finite input slot is written as `null`, as the Rust
# parity test reads it. The reader also takes 1e39, past the float32 range, which older
# golden files used.
NON_FINITE = None
LEGACY_NON_FINITE = 1e39


def golden_cases(blob: bytes, window: feat.Arrays | None = None, seed: int = 0) -> list[dict]:
    """64 cases computed by `predict` from the artifact bytes: all zeros, all ones, all
    minus ones, one non-finite slot, one unsupported cell, one per threshold code, then
    rows from `window` when given and uniform draws in [-1, 1] for the rest."""
    from . import predict

    art = predict.read(blob)
    schema = sch.load()
    rng = np.random.default_rng(seed)
    edit_dim = schema.edit_dim
    support = [int(c) for c in art.support]

    def some_cell(code: int | None = None) -> int:
        matching = [c for c in support if code is None or c & 0xFF == code]
        if matching:
            return matching[int(rng.integers(0, len(matching)))]
        return sch.cell_id(1, 1, 0, 0 if code is None else code)

    def uniform() -> tuple[np.ndarray, np.ndarray]:
        return (rng.uniform(-1, 1, sch.BASE_DIM).astype(np.float32),
                rng.uniform(-1, 1, edit_dim).astype(np.float32))

    inputs: list[tuple[np.ndarray, np.ndarray, int]] = []
    for value in (0.0, 1.0, -1.0):
        inputs.append((np.full(sch.BASE_DIM, value, np.float32),
                       np.full(edit_dim, value, np.float32), some_cell()))
    base, edit = uniform()
    base[7] = np.inf
    inputs.append((base, edit, some_cell()))
    unsupported = sch.cell_id(7, 13, 3, 0)
    while unsupported in support:
        unsupported += 1
    inputs.append((*uniform(), unsupported))
    for code in range(len(art.thresholds)):
        inputs.append((*uniform(), some_cell(code)))
    if window is not None and len(window):
        picks = rng.choice(len(window), size=min(24, len(window)), replace=False)
        for i in picks:
            x = window.X[i]
            inputs.append((x[: sch.BASE_DIM], x[sch.BASE_DIM : sch.BASE_DIM + edit_dim],
                           int(window.cell[i])))
    while len(inputs) < GOLDEN_CASES:
        inputs.append((*uniform(), some_cell()))

    cases = []
    for base, edit, cell in inputs[:GOLDEN_CASES]:
        got = predict.predict(art, base, edit, [cell])

        def num(v):
            v = float(v)
            return None if math.isnan(v) else v

        cases.append({
            "base": [float(v) if np.isfinite(v) else NON_FINITE for v in base],
            "edit": [float(v) if np.isfinite(v) else NON_FINITE for v in edit],
            "cell": int(cell),
            "expect": {
                "p_success": num(got.p_success[0]),
                "latency_ms": num(got.latency_ms[0]),
                "credits": num(got.credits[0]),
                "support": num(got.support[0]),
            },
        })
    return cases


def golden_json(cases: list[dict]) -> str:
    return json.dumps(cases, separators=(",", ":")) + "\n"


def read_golden(text: str) -> list[dict]:
    doc = json.loads(text)
    return doc["cases"] if isinstance(doc, dict) else doc


def golden_slots(values) -> np.ndarray:
    """A golden input vector as float32: `null` is NaN, and a number past the float32
    range, such as the legacy 1e39, overflows to infinity. Both are non-finite."""
    out = np.array([np.nan if v is None else v for v in values], dtype=np.float64)
    with np.errstate(over="ignore"):
        return out.astype(np.float32)


# Quantisation, Python only. The Rust reader stays FP32.


def quantize_tensor(x) -> dict:
    """Per-tensor affine int8: `x ~ (q - zero_point) * scale`."""
    x = np.asarray(x, dtype=np.float64)
    lo, hi = float(min(x.min(), 0.0)), float(max(x.max(), 0.0))
    scale = (hi - lo) / 255.0 if hi > lo else 1.0
    zero_point = int(round(-128 - lo / scale))
    q = np.clip(np.round(x / scale) + zero_point, -128, 127).astype(np.int8)
    return {"q": q, "scale": scale, "zero_point": zero_point}


def dequantize(t: dict) -> np.ndarray:
    return (t["q"].astype(np.float64) - t["zero_point"]) * t["scale"]


def quantize(model):
    """An int8 copy of a model, dequantised back into the same class so every metric
    runs on it unchanged."""
    from .models.lightgbm_head import GbdtModel
    from .models.mlp import MlpModel, Net

    if model.kind == "mlp":
        heads = []
        for net in model.heads:
            heads.append(Net(
                net.kind,
                [dequantize(quantize_tensor(W)) for W in net.weights],
                [dequantize(quantize_tensor(b)) for b in net.biases],
            ))
        return MlpModel(heads, info={"quantized": "int8"})
    tables = []
    for trees in model.to_tables():
        values = np.array([n[5] for tree in trees for n in tree])
        deq = dequantize(quantize_tensor(values)) if len(values) else values
        at = 0
        head = []
        for tree in trees:
            nodes = []
            for n in tree:
                nodes.append((n[0], n[1], n[2], n[3], n[4], float(deq[at])))
                at += 1
            head.append(nodes)
        tables.append(head)
    return GbdtModel([], tables=tables, info={"quantized": "int8"})


def save_quantized(path: Path, model) -> None:
    arrays = {}
    if model.kind == "mlp":
        for h, net in enumerate(model.heads):
            for i, (W, b) in enumerate(zip(net.weights, net.biases, strict=True)):
                for name, x in (("w", W), ("b", b)):
                    q = quantize_tensor(x)
                    arrays[f"{name}{h}_{i}"] = q["q"]
                    arrays[f"{name}{h}_{i}_scale"] = np.array(q["scale"])
                    arrays[f"{name}{h}_{i}_zero"] = np.array(q["zero_point"])
    else:
        for h, trees in enumerate(model.to_tables()):
            values = np.array([n[5] for tree in trees for n in tree])
            q = quantize_tensor(values)
            arrays[f"values{h}"] = q["q"]
            arrays[f"values{h}_scale"] = np.array(q["scale"])
            arrays[f"values{h}_zero"] = np.array(q["zero_point"])
    np.savez(path, **arrays)
