import hashlib
import json
import math
import struct
import zlib

import numpy as np
import pytest
from conftest import GOLDEN_DIR, REPO

from spider_optimize_train import evaluate as ev
from spider_optimize_train import export as ex
from spider_optimize_train import predict
from spider_optimize_train import schema as sch
from spider_optimize_train import thresholds as th
from spider_optimize_train.calibrate import Calibration
from spider_optimize_train.features import PINNED_W


def artifact_for(trained, kind):
    test = trained.windows["test"]
    per_code = [float("nan"), 0.6, 0.55] + [0.7] * (sch.load().edit_codes - 3)
    support = sorted({int(c) for c in test.cell[: len(test) // 2]})
    tables = ex.tables_for(trained.models[kind], trained.calibrations[kind], per_code, support)
    return ex.serialize(tables), per_code, support


def inputs(arrays):
    edit_dim = sch.load().edit_dim
    return arrays.X[:, :152], arrays.X[:, 152 : 152 + edit_dim]


def zero_pins(X):
    X = X.copy()
    X[:, -PINNED_W:] = 0.0
    X[:, -PINNED_W] = 1.0
    return X


@pytest.mark.parametrize("kind", ["mlp", "gbdt"])
def test_export_roundtrip_matches_model(trained, kind):
    blob, per_code, support = artifact_for(trained, kind)
    test = trained.windows["test"]
    base, edit = inputs(test)
    got = predict.predict(blob, base, edit, test.cell)

    model, cal = trained.models[kind], trained.calibrations[kind]
    raw, log_millis, log_credits = model.predict(zero_pins(test.X))
    assert np.allclose(got.p_success, cal.apply(raw), atol=2e-5)
    assert np.allclose(got.latency_ms, np.expm1(log_millis), rtol=5e-5)
    assert np.allclose(got.credits, np.expm1(log_credits), rtol=5e-5, atol=1e-5)
    assert np.array_equal(got.support, np.isin(test.cell, support).astype(np.float32))
    floor = np.array(per_code)[test.cell & 0xFF]
    expected_abstain = np.isnan(floor) | (got.p_success < floor) | (got.support == 0)
    assert np.array_equal(got.abstain, expected_abstain)
    assert got.abstain.any() and not got.abstain.all()


def test_the_pinned_bucket_folds_in_at_zero_pins(trained):
    """The artifact answers as the model does for a caller who pinned nothing, and
    differs from the model where rows had pins."""
    blob, _, _ = artifact_for(trained, "mlp")
    test = trained.windows["test"]
    pinned = test.X[:, -PINNED_W] == 0.0
    assert pinned.any()
    base, edit = inputs(test)
    got = predict.predict(blob, base, edit, test.cell)
    raw = trained.models["mlp"].predict(test.X)[0]
    p_as_trained = trained.calibrations["mlp"].apply(raw)
    assert np.allclose(got.p_success[~pinned], p_as_trained[~pinned], atol=2e-5)
    assert not np.allclose(got.p_success[pinned], p_as_trained[pinned], atol=2e-5)


@pytest.mark.parametrize("kind", ["mlp", "gbdt"])
def test_export_has_no_ascii_run_and_fits_size(trained, kind):
    blob, _, _ = artifact_for(trained, kind)
    assert len(blob) <= ex.MAX_BYTES
    assert ex.ascii_runs(blob, ex.MAX_ASCII_RUN + 1) == []
    for at, length in ex.ascii_runs(blob, 4):
        assert not ex.DOMAIN_LIKE.search(blob[at : at + length].lower())
    ex.audit(blob)


def spelled(text: bytes) -> list[float]:
    """Floats whose bytes spell `text`, four at a time."""
    return [struct.unpack("<f", text[i : i + 4])[0] for i in range(0, len(text), 4)]


def mlp_tables(first_row=None, calibrations=None, thresholds=None) -> ex.Tables:
    """A one-layer MLP artifact with a Platt success head, optionally with chosen weights
    in the first head's single row."""
    W = np.zeros((1, 248), np.float32)
    if first_row is not None:
        W[0, : len(first_row)] = first_row
    zero = (np.zeros((1, 248), np.float32), np.zeros(1, np.float32), 0)
    return ex.Tables(
        kind=ex.KIND_MLP,
        thresholds=[float("nan")] if thresholds is None else thresholds,
        support=[],
        calibrations=calibrations or [(1, [1.0, 0.0]), (0, []), (0, [])],
        mlp=[[(W, np.zeros(1, np.float32), 0)], [zero], [zero]],
    )


def test_a_planted_ascii_run_or_host_is_broken_and_audited():
    # A long run, then a gap, then a run of seven bytes that reads as a host.
    host = spelled(b"\0ab.cdef\0\0\0\0")
    planted_weights = spelled(b"abcdefghijklmnop") + [0.0] + host
    tables = mlp_tables(first_row=planted_weights)
    raw = bytes(ex._body(tables).buf)
    assert ex.ascii_runs(raw, 9), "the plant must produce a long run"
    with pytest.raises(ValueError, match="printable run"):
        ex.audit(raw + b"\0\0\0\0")
    long_run_fixed = bytearray(raw)
    for at, length in ex.ascii_runs(raw, 9):
        for cut in range(at + 4, at + length, 5):
            long_run_fixed[cut] = 0
    with pytest.raises(ValueError, match="domain-like"):
        ex.audit(bytes(long_run_fixed) + b"\0\0\0\0")
    blob = ex.serialize(tables)
    ex.audit(blob)
    assert ex.ascii_runs(blob, 9) == []
    assert all(not ex.DOMAIN_LIKE.search(blob[a : a + n]) for a, n in ex.ascii_runs(blob, 4))
    # Only the lowest byte of a float moved, by less than 0x60.
    art = predict.read(blob)
    planted = np.array(planted_weights, dtype=np.float32)
    assert np.allclose(art.mlp[0][0][0][0, : len(planted)], planted, rtol=1e-5)


def test_audit_refuses_an_oversized_blob():
    with pytest.raises(ValueError, match="over"):
        ex.audit(b"\0" * (ex.MAX_BYTES + 1))


def test_crc_mismatch_is_refused(trained):
    blob, _, _ = artifact_for(trained, "gbdt")
    predict.read(blob)
    body = bytearray(blob)
    body[len(body) // 2] ^= 0x01
    with pytest.raises(predict.CrcError):
        predict.read(bytes(body))
    tail = bytearray(blob)
    tail[-1] ^= 0x80
    with pytest.raises(predict.CrcError):
        predict.read(bytes(tail))
    (crc,) = struct.unpack("<I", blob[-4:])
    assert crc == zlib.crc32(blob[:-4])


def test_the_reader_refuses_bad_headers_and_truncation(trained):
    blob, _, _ = artifact_for(trained, "mlp")
    with pytest.raises(predict.ArtifactError, match="magic"):
        predict.read(b"SPOPX" + blob[5:])
    with pytest.raises(predict.ArtifactError, match="version"):
        predict.read(blob[:5] + b"\x02" + blob[6:])
    with pytest.raises(predict.ArtifactError, match="too short"):
        predict.read(blob[:12])
    rng = np.random.default_rng(0)
    for cut in rng.integers(20, len(blob) - 1, 40):
        body = blob[: int(cut)]
        body += struct.pack("<I", zlib.crc32(body))
        with pytest.raises(predict.ArtifactError):
            predict.read(body)


def check_golden(cases, blob):
    """Python's reader against a golden file, with the Rust parity test's rule: within
    1e-5 absolute, and NaN exactly where the file says `null`."""
    assert len(cases) >= 32
    base = np.array([ex.golden_slots(c["base"]) for c in cases])
    edit = np.array([ex.golden_slots(c["edit"]) for c in cases])
    cells = np.array([c["cell"] for c in cases], dtype=np.int64)
    got = predict.predict(blob, base, edit, cells)
    for i, case in enumerate(cases):
        for name in ("p_success", "latency_ms", "credits", "support"):
            want = case["expect"][name]
            have = float(getattr(got, name)[i])
            if want is None:
                assert math.isnan(have), (i, name)
            else:
                assert abs(have - np.float32(want)) <= 1e-5, (i, name, have, want)
    return got


def only_numbers(value) -> bool:
    """A golden document holds numbers, `null`, and the fixed keys, nothing else."""
    if isinstance(value, dict):
        return set(value) <= GOLDEN_KEYS and all(only_numbers(v) for v in value.values())
    if isinstance(value, list):
        return all(only_numbers(v) for v in value)
    return value is None or (isinstance(value, (int, float)) and not isinstance(value, bool))


GOLDEN_KEYS = {"base", "edit", "cell", "expect", "p_success", "latency_ms", "credits", "support"}


@pytest.mark.parametrize("kind", ["mlp", "gbdt"])
def test_golden_matches_predict_reference(kind):
    blob = (GOLDEN_DIR / f"synth-{kind}.bin").read_bytes()
    cases = ex.read_golden((GOLDEN_DIR / f"golden-{kind}.json").read_text())
    assert len(cases) == ex.GOLDEN_CASES
    got = check_golden(cases, blob)
    sidecar = json.loads((GOLDEN_DIR / f"synth-{kind}.sidecar.json").read_text())
    assert sidecar["sha256"] == hashlib.sha256(blob).hexdigest()
    assert sidecar["synthetic"] is True
    assert sidecar["bytes"] == len(blob)

    # The required shapes are all there.
    assert all(v == 0 for v in cases[0]["base"] + cases[0]["edit"])
    assert all(v == 1 for v in cases[1]["base"] + cases[1]["edit"])
    assert all(v == -1 for v in cases[2]["base"] + cases[2]["edit"])
    assert cases[3]["expect"]["p_success"] is None
    assert None in cases[3]["base"]
    assert all(None not in c["base"] + c["edit"] for c in cases[:3] + cases[4:])
    assert cases[4]["expect"]["support"] == 0.0
    art = predict.read(blob)
    codes = {c["cell"] & 0xFF for c in cases}
    assert codes >= set(range(len(art.thresholds)))
    assert got.abstain.any() and not got.abstain.all()
    assert np.isfinite(art.thresholds).any() and np.isnan(art.thresholds).any()
    assert art.calibrations[0][0] != 0, "the success head must not be identity"
    assert only_numbers(json.loads((GOLDEN_DIR / f"golden-{kind}.json").read_text()))


RUST = REPO / "spider-optimize"
# (trainer artifact, trainer golden, crate artifact, crate golden)
RUST_FIXTURES = {
    "mlp": ("synth-mlp.bin", "golden-mlp.json", RUST / "assets" / "spider-optimize-v1.bin",
            RUST / "tests" / "fixtures" / "golden-v1.json"),
    "gbdt": ("synth-gbdt.bin", "golden-gbdt.json", RUST / "tests" / "fixtures" / "gbdt-v1.bin",
             RUST / "tests" / "fixtures" / "golden-gbdt-v1.json"),
}


def rust_fixture(kind: str) -> tuple[bytes, list[dict]]:
    """The crate's fixture pair. A missing file fails: a skip here would pass a parity
    check that never ran."""
    _, _, artifact, golden = RUST_FIXTURES[kind]
    for path in (artifact, golden):
        assert path.is_file(), f"the Rust parity fixture {path} is missing"
    return artifact.read_bytes(), ex.read_golden(golden.read_text())


def test_python_reader_matches_rust_golden():
    blob, cases = rust_fixture("mlp")
    check_golden(cases, blob)


def test_python_reader_matches_rust_gbdt_golden():
    blob, cases = rust_fixture("gbdt")
    check_golden(cases, blob)


@pytest.mark.parametrize("kind", ["mlp", "gbdt"])
def test_the_crate_ships_the_trainer_fixtures_byte_for_byte(kind):
    trainer_blob, trainer_golden, artifact, golden = RUST_FIXTURES[kind]
    blob, _ = rust_fixture(kind)
    assert blob == (GOLDEN_DIR / trainer_blob).read_bytes()
    assert golden.read_text() == (GOLDEN_DIR / trainer_golden).read_text()
    assert only_numbers(json.loads(golden.read_text()))


def test_a_missing_rust_fixture_fails_rather_than_skips(monkeypatch, tmp_path):
    missing = tmp_path / "absent.bin"
    monkeypatch.setitem(RUST_FIXTURES, "mlp", ("", "", missing, missing))
    with pytest.raises(AssertionError, match="is missing"):
        rust_fixture("mlp")


@pytest.mark.parametrize("kind", ["mlp", "gbdt"])
def test_quantized_model_is_re_evaluated(trained, kind):
    model = trained.models[kind]
    q = ex.quantize(model)
    if kind == "mlp":
        for W in q.heads[0].weights:
            assert len(np.unique(W)) <= 256
    else:
        values = {n[5] for tree in q.tables[0] for n in tree}
        assert len(values) <= 256
    thresholds = th.Thresholds([float("nan")] * 10, float("nan"), {}, [])
    test = trained.windows["test"]
    cal = trained.calibrations[kind]
    fp32 = ev.evaluate(model, cal, test, thresholds, trained.tau)
    int8 = ev.evaluate(q, cal, test, thresholds, trained.tau)
    assert int8["success"].keys() == fp32["success"].keys()
    assert int8["success"] != fp32["success"]
    assert int8["success"]["auroc"] == pytest.approx(fp32["success"]["auroc"], abs=0.05)
    text = ev.render(kind, fp32, True, int8, cal.report)
    assert "| metric | fp32 | int8 (Python only) |" in text


def test_calibration_serialises_in_the_artifact_layout():
    platt = ex.calibration_entry(Calibration(1, params=[0.5, -1.0]))
    iso = ex.calibration_entry(Calibration(2, knots=[(-1.0, 0.1), (2.0, 0.9)]))
    assert platt == (1, [0.5, -1.0])
    assert iso == (2, [-1.0, 0.1, 2.0, 0.9])
    tables = mlp_tables(calibrations=[platt, iso, (0, [])])
    blob = ex.serialize(tables)
    art = predict.read(blob)
    assert art.calibrations[0][0] == 1 and list(art.calibrations[0][1]) == [0.5, -1.0]
    assert art.calibrations[1][0] == 2
    assert np.allclose(art.calibrations[1][1], [-1.0, 0.1, 2.0, 0.9])
    got = predict.predict(art, np.zeros((1, 152)), np.zeros((1, 96)), [0])
    assert got.p_success[0] == pytest.approx(1 / (1 + math.exp(1.0)), rel=1e-6)
    assert got.latency_ms[0] == pytest.approx(math.expm1(0.1 + 0.8 / 3), rel=1e-6)
    assert got.credits[0] == 0.0 and got.support[0] == 1.0
    # (kind, knots) on the wire, as `artifact.rs` accepts them: Platt and identity
    # carry knots 0, isotonic its knot count.
    at = ex.HEADER_BYTES + 2 + 4 + 4
    assert struct.unpack_from("<HH", blob, at) == (1, 0)
    assert struct.unpack_from("<HH", blob, at + 4 + 8) == (2, 2)
    assert struct.unpack_from("<HH", blob, at + 4 + 8 + 4 + 16) == (0, 0)


def with_crc(body: bytes) -> bytes:
    return body + struct.pack("<I", zlib.crc32(body) & 0xFFFFFFFF)


def test_the_reader_refuses_what_the_rust_reader_refuses():
    blob = ex.serialize(mlp_tables())
    body = bytearray(blob[:-4])
    at = ex.HEADER_BYTES + 2 + 4 + 4
    cases = {
        "Platt with knots 1": (at + 2, struct.pack("<H", 1)),
        "reserved byte": (15, b"\x01"),
        "threshold outside [0, 1]": (ex.HEADER_BYTES + 2, struct.pack("<f", 1.5)),
        "infinite weight": (at + 4 + 8 + 8 + 2 + 5, struct.pack("<f", math.inf)),
    }
    for name, (offset, patch) in cases.items():
        bad = bytearray(body)
        bad[offset : offset + len(patch)] = patch
        with pytest.raises(predict.ArtifactError):
            predict.read(with_crc(bytes(bad)))
            pytest.fail(name)
        with pytest.raises(ValueError):
            ex.audit(with_crc(bytes(bad)))
    one_knot = mlp_tables(calibrations=[(2, [0.0, 0.5]), (0, []), (0, [])])
    with pytest.raises(predict.ArtifactError, match="knots"):
        predict.read(with_crc(bytes(ex._body(one_knot).buf)))
    falling = mlp_tables(calibrations=[(2, [0.0, 0.5, 1.0, 0.4]), (0, []), (0, [])])
    with pytest.raises(predict.ArtifactError, match="order"):
        predict.read(with_crc(bytes(ex._body(falling).buf)))
    with pytest.raises(ValueError, match="two knots"):
        ex.calibration_entry(Calibration(2, knots=[(0.0, 0.5)]))


def test_an_identity_success_head_is_refused_and_passes_through_in_the_reader(trained):
    with pytest.raises(ValueError, match="identity"):
        ex.tables_for(trained.models["mlp"], Calibration(0), [float("nan")], [])
    # The reader, like `Calibration::apply` in Rust, passes an identity head through.
    tables = mlp_tables(calibrations=[(0, []), (0, []), (0, [])])
    tables.mlp[0][0][1][0] = 3.0
    art = predict.read(ex.serialize(tables))
    got = predict.predict(art, np.zeros((1, 152)), np.zeros((1, 96)), [0])
    assert got.p_success[0] == np.float32(3.0)


def test_calibration_floats_are_never_nudged():
    # Two ordered knots whose 16 bytes are all printable. The calibration header bytes
    # on either side are not, so the run holds only knots, and the writer refuses it
    # rather than move a knot out of order.
    knots = spelled(b"aaaabbbbccccdddd")
    assert knots[0] < knots[2] and knots[1] <= knots[3]
    tables = mlp_tables(calibrations=[(2, knots), (0, []), (0, [])])
    with pytest.raises(ValueError, match="no float to adjust"):
        ex.serialize(tables)
    # A run of eight is allowed, and the knots come back exactly.
    short = spelled(b"aaaa\0\0\0\0bbbb\0\0\0\0")
    art = predict.read(ex.serialize(mlp_tables(calibrations=[(2, short), (0, []), (0, [])])))
    assert art.calibrations[0][1].tobytes() == np.array(short, "<f4").tobytes()


def test_folded_pinned_splits_use_finite_sentinels():
    first = 152 + 96
    nodes = [
        (first, False, 0.5, 1, 2, 0.0),      # bucket 0 reads 1: 1 > 0.5 goes right
        (first + 1, True, 0.5, 3, 4, 0.0),   # bucket 1 reads 0: 0 <= 0.5 goes left
        (0, False, 0.0, ex.LEAF, ex.LEAF, 1.0),
        (0, False, 0.0, ex.LEAF, ex.LEAF, 2.0),
        (0, False, 0.0, ex.LEAF, ex.LEAF, 3.0),
    ]
    folded = ex.fold_pinned_tree(nodes, 96)
    assert folded[0][:3] == (0, False, ex.ALWAYS_RIGHT)
    assert folded[1][:3] == (0, True, ex.ALWAYS_LEFT)
    for x in (-1.0, 0.0, 1.0, math.nan, math.inf):
        for node, left in ((folded[0], False), (folded[1], True)):
            goes_left = node[1] if not math.isfinite(x) else x <= node[2]
            assert goes_left is left


def test_no_exported_threshold_is_non_finite(trained):
    blob, _, _ = artifact_for(trained, "gbdt")
    art = predict.read(blob)
    tables = [t for _, trees in art.gbdt for t in trees]
    assert tables
    for table in tables:
        assert np.all(np.isfinite(table["threshold"]))
        assert np.all(np.isfinite(table["value"]))
    tables = ex.tables_for(trained.models["gbdt"], trained.calibrations["gbdt"],
                           [float("nan")], [])
    folded = [n for _, trees in tables.gbdt for tree in trees for n in tree]
    assert all(math.isfinite(n[2]) for n in folded)


def test_golden_cases_write_null_for_a_non_finite_slot_and_read_both(trained):
    blob, _, _ = artifact_for(trained, "mlp")
    cases = ex.golden_cases(blob, seed=3)
    assert None in cases[3]["base"]
    assert only_numbers(json.loads(ex.golden_json(cases)))
    legacy = json.loads(ex.golden_json(cases))
    legacy[3]["base"] = [ex.LEGACY_NON_FINITE if v is None else v for v in legacy[3]["base"]]
    for doc in (cases, legacy):
        slots = ex.golden_slots(doc[3]["base"])
        assert not np.all(np.isfinite(slots))
        check_golden(doc, blob)
