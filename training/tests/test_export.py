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


def test_a_planted_ascii_run_or_host_is_broken_and_audited():
    # A long run, then a gap, then a run of seven bytes that reads as a host.
    host = spelled(b"\0ab.cdef\0\0\0\0")
    tables = ex.Tables(
        kind=ex.KIND_MLP,
        thresholds=spelled(b"abcdefghijklmnop") + [0.0] + host,
        support=[],
        calibrations=[(0, []), (0, []), (0, [])],
        mlp=[[(np.zeros((1, 248), np.float32), np.zeros(1, np.float32), 0)]] * 3,
    )
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
    planted = np.array(tables.thresholds, dtype=np.float32)
    assert np.allclose(art.thresholds, planted, rtol=1e-5)


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
    assert len(cases) >= 32
    with np.errstate(over="ignore"):
        # 1e39 stands for a non-finite slot and overflows to infinity here on purpose.
        base = np.array([c["base"] for c in cases], dtype=np.float64).astype(np.float32)
        edit = np.array([c["edit"] for c in cases], dtype=np.float64).astype(np.float32)
        cells = np.array([c["cell"] for c in cases], dtype=np.int64)
        got = predict.predict(blob, base, edit, cells)
    for i, case in enumerate(cases):
        for name in ("p_success", "latency_ms", "credits", "support"):
            want = case["expect"][name]
            have = float(getattr(got, name)[i])
            if want is None:
                assert math.isnan(have), (i, name)
            else:
                assert have == pytest.approx(want, rel=1e-5, abs=1e-5), (i, name)
    return got


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
    assert any(abs(v) > 3.5e38 for v in cases[3]["base"])
    assert cases[4]["expect"]["support"] == 0.0
    art = predict.read(blob)
    codes = {c["cell"] & 0xFF for c in cases}
    assert codes >= set(range(len(art.thresholds)))
    assert got.abstain.any() and not got.abstain.all()
    assert np.isfinite(art.thresholds).any() and np.isnan(art.thresholds).any()


RUST_GOLDEN = REPO / "spider-optimize" / "tests" / "fixtures" / "golden-v1.json"
RUST_ARTIFACT = REPO / "spider-optimize" / "assets" / "spider-optimize-v1.bin"


def test_python_reader_matches_rust_golden():
    if not (RUST_GOLDEN.is_file() and RUST_ARTIFACT.is_file()):
        pytest.skip(
            "the Rust fixture is not on this branch yet: "
            f"{RUST_GOLDEN.relative_to(REPO)} and {RUST_ARTIFACT.relative_to(REPO)}"
        )
    cases = ex.read_golden(RUST_GOLDEN.read_text())
    check_golden(cases, RUST_ARTIFACT.read_bytes())


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
    tables = ex.Tables(
        kind=ex.KIND_MLP, thresholds=[], support=[], calibrations=[platt, iso, (0, [])],
        mlp=[[(np.zeros((1, 248), np.float32), np.zeros(1, np.float32), 0)]] * 3,
    )
    art = predict.read(ex.serialize(tables))
    assert art.calibrations[0][0] == 1 and list(art.calibrations[0][1]) == [0.5, -1.0]
    assert art.calibrations[1][0] == 2
    assert np.allclose(art.calibrations[1][1], [-1.0, 0.1, 2.0, 0.9])
    got = predict.predict(art, np.zeros((1, 152)), np.zeros((1, 96)), [0])
    assert got.p_success[0] == pytest.approx(1 / (1 + math.exp(1.0)), rel=1e-6)
    assert got.latency_ms[0] == pytest.approx(math.expm1(0.1 + 0.8 / 3), rel=1e-6)
    assert got.credits[0] == 0.0 and got.support[0] == 1.0
