"""The reversal fixture and its stable control, through `train`, `thresholds`, `gates`,
`eval` and `tradeoff`. Every expected number is the plant the manifest records.

`reversal` proves the evaluation rejects an artifact whose residential edit turned
harmful inside the test window, without asking the model to have foreseen a change no
row before that window carried. `stable` proves a run can pass with edits applied, so
passing cannot consist of abstaining everywhere.
"""

import json
import shutil
from pathlib import Path

import numpy as np
import pytest

from spider_optimize_train import cli, compare, synth, tradeoff
from spider_optimize_train import features as feat
from spider_optimize_train import thresholds as th
from spider_optimize_train.dataset import Manifest, read_rows
from spider_optimize_train.models import load_run, make_split

EVALS = Path(__file__).resolve().parents[1] / "evals"
PAIRS = 20_000
SEED = 7


def build(tmp_path_factory, scenario: str):
    root = tmp_path_factory.mktemp(scenario)
    corpus, run = root / "corpus", root / "run"
    synth.generate(corpus, seed=SEED, pairs=PAIRS, scenario=scenario)
    assert cli.main(["train", str(corpus), "--out", str(run), "--seed", str(SEED)]) == 0
    assert cli.main(["thresholds", str(corpus), "--run", str(run), "--seed", str(SEED)]) == 0
    return corpus, run


@pytest.fixture(scope="module")
def reversal_run(tmp_path_factory):
    return build(tmp_path_factory, "reversal")


@pytest.fixture(scope="module")
def stable_run(tmp_path_factory):
    return build(tmp_path_factory, "stable")


class Loaded:
    """A run and its test window, with the per-code override counts."""

    def __init__(self, corpus: Path, run: Path):
        self.manifest = Manifest.read(corpus)
        self.doc, self.models, self.cals = load_run(run)
        rows = read_rows(corpus)
        parts = make_split(rows, "chronological")
        self.test = feat.build(parts.test, self.doc["tau"])
        self.thresholds = {
            kind: th.Thresholds.from_dict(json.loads((run / f"thresholds-{kind}.json").read_text()))
            for kind in self.models
        }

    def comparison(self, kind: str) -> compare.Comparison:
        return compare.compare(self.models[kind], self.cals[kind], self.test,
                               self.thresholds[kind], self.doc["tau"], resamples=500)

    def overrides_by_wire(self, kind: str) -> dict[str, int]:
        scored = th.score(self.models[kind], self.cals[kind], self.test, self.doc["tau"])
        applied = th.applies(scored, self.thresholds[kind])
        out = {}
        for wire in ("proxy", "wait_for"):
            key = synth.standard_edit(wire).key
            mask = np.array([bool(r["edit"]) and r["edit"]["key"] == key for r in self.test.rows])
            out[wire] = int((applied & mask).sum())
        return out


def run_eval(corpus, run, floors, capsys, resamples=300):
    code = cli.main(["eval", str(corpus), "--run", str(run), "--floors", str(floors),
                     "--resamples", str(resamples)])
    printed = [line.removeprefix("FIXTURE-ONLY: ") for line in capsys.readouterr().out.splitlines()]
    return code, printed, (run / "eval-report.md").read_text()


def blocked_proxy_rows(rows, planted):
    key = synth.standard_edit("proxy").key
    status = planted["residential"]["status"]
    return [r for r in rows if r["edit"] and r["edit"]["key"] == key
            and synth.site_of(r).status == status]


def test_reversal_in_the_test_window_is_rejected(reversal_run, capsys):
    corpus, run = reversal_run
    loaded = Loaded(corpus, run)
    planted = loaded.manifest.planted
    res = planted["residential"]
    assert planted["scenario"] == "reversal"

    # The flip starts on the first test day and no earlier row carries it.
    flip = res["flip_day"]
    assert flip == planted["days"] - res["flipped_last_days"] == synth.FLIP_DAY
    assert loaded.doc["windows"]["test"]["days"][0] == flip
    assert loaded.doc["windows"]["calibrate"]["days"][1] < flip
    rows = blocked_proxy_rows(read_rows(corpus), planted)
    before = [r["success"] for r in rows if r["day"] < flip]
    after = [r["success"] for r in rows if r["day"] >= flip]
    assert len(before) > 1000 and len(after) > 300
    assert np.mean(before) == pytest.approx(res["success_to"], abs=0.03)
    assert np.mean(after) == pytest.approx(res["success_when_flipped"], abs=0.03)

    assert cli.main(["gates", str(corpus), "--run", str(run), "--seed", str(SEED),
                     "--resamples", "500"]) == 1
    report = (run / "regression-report.md").read_text()
    for kind in loaded.models:
        c = loaded.comparison(kind)
        by_wire = loaded.overrides_by_wire(kind)
        # The model applied residential on the evidence it had, as it should have.
        assert by_wire["proxy"] > 0
        assert c.coverage > 0 and c.overrides >= by_wire["proxy"]
        # And the window says the edit lost what the plain fetch had: a blocked site
        # succeeded at `success_from` and now at `success_when_flipped`.
        assert c.success_delta < 0
        assert c.harmful > 0 and c.harmful > c.helpful
        lost = res["success_from"] - res["success_when_flipped"]
        assert c.harmful / by_wire["proxy"] == pytest.approx(lost, abs=0.06)
        assert c.credits_per_correct_delta > 0
        section = report.split(f"# Regression report: {kind}\n", 1)[1].split("\n# ", 1)[0]
        assert "Status: **fail**" in section
        assert "| success delta | fail |" in section
        assert f"| harmful overrides | {c.harmful} | 0 | rate " in section

    code, printed, text = run_eval(corpus, run, EVALS / "synth-reversal-floors.json", capsys)
    assert code == 0, printed
    assert "| gate status | fail | = fail | pass |" in text
    assert "| harmful overrides | " in text and "| >= 1 | pass |" in text
    assert "improve" not in text.lower().replace("claims no improvement", "")


def test_stable_control_passes_with_overrides_applied(stable_run, capsys):
    corpus, run = stable_run
    loaded = Loaded(corpus, run)
    planted = loaded.manifest.planted
    assert planted["scenario"] == "stable"
    assert planted["residential"]["flipped_last_days"] == 0

    for kind in loaded.models:
        c = loaded.comparison(kind)
        # First: the policy did something. A run that abstains everywhere stops here.
        assert c.coverage >= 0.05, c
        assert c.overrides >= 100, c
        by_wire = loaded.overrides_by_wire(kind)
        assert by_wire["proxy"] >= 100 and by_wire["wait_for"] >= 100
        assert c.harmful_rate_ucb <= th.R_MAX, c
        assert c.policy_credits_per_correct <= c.baseline_credits_per_correct, c
        # The overrides bought what the plant says they buy.
        res, wait = planted["residential"], planted["wait"]
        expected = (by_wire["proxy"] * (res["success_to"] - res["success_from"])
                    + by_wire["wait_for"] * (wait["success_to"] - wait["success_from"]))
        assert c.success_delta == pytest.approx(expected / c.pairs, abs=0.03)
        assert c.helpful / c.overrides == pytest.approx(expected / c.overrides, abs=0.06)

    assert cli.main(["gates", str(corpus), "--run", str(run), "--seed", str(SEED),
                     "--resamples", "500"]) == 0
    report = (run / "regression-report.md").read_text()
    assert report.count("Status: **pass**") == len(loaded.models)

    code, printed, text = run_eval(corpus, run, EVALS / "synth-stable-floors.json", capsys)
    assert code == 0, printed
    assert "| gate status | pass | = pass | pass |" in text
    assert "| coverage | " in text and "| >= 0.0500 | pass |" in text
    assert "| harmful override rate | " in text and "| <= 0.0100 | pass |" in text
    assert "Every floor met." in text


def test_abstaining_everything_does_not_pass_the_control(stable_run, tmp_path, capsys):
    corpus, run = stable_run
    abstain = tmp_path / "abstain"
    shutil.copytree(run, abstain)
    for path in abstain.glob("thresholds-*.json"):
        doc = json.loads(path.read_text())
        doc["per_code"] = [None] * len(doc["per_code"])
        doc["pooled"] = None
        path.write_text(json.dumps(doc))
    code, printed, text = run_eval(corpus, abstain, EVALS / "synth-stable-floors.json", capsys)
    assert code == 1
    for kind in ("mlp", "gbdt"):
        assert f"{kind} coverage: 0.0000 misses >= 0.0500" in printed
        assert f"{kind} harmful override rate: NaN misses <= 0.0100" in printed
        assert f"{kind} gate status: insufficient misses = pass" in printed
    assert "| coverage | 0.0000 | >= 0.0500 | fail |" in text


def test_tradeoff_coverage_is_monotone_in_r_max(stable_run, reversal_run):
    for corpus, run in (stable_run, reversal_run):
        assert cli.main(["tradeoff", str(corpus), "--run", str(run), "--seed", str(SEED),
                         "--resamples", "200", "--gate-resamples", "200"]) == 0
        text = (run / "tradeoff.md").read_text()
        assert text.startswith("# Tradeoff: overrides against regression risk\n\n"
                               "> **FIXTURE-ONLY.**")
        loaded = Loaded(corpus, run)
        windows = {"train": None, "calibrate": None, "test": loaded.test}
        rows = read_rows(corpus)
        parts = make_split(rows, "chronological")
        windows["train"] = feat.build(parts.train, loaded.doc["tau"])
        windows["calibrate"] = feat.build(parts.calibrate, loaded.doc["tau"])
        for kind, model in loaded.models.items():
            sweep = tradeoff.sweep(model, loaded.cals[kind], windows, loaded.doc["tau"],
                                   resamples=200, gate_resamples=200, seed=SEED)
            assert [r.r_max for r in sweep] == list(tradeoff.R_MAX_GRID)
            coverage = [r.comparison.coverage for r in sweep]
            assert coverage == sorted(coverage), (kind, coverage)
            assert all(r.floors >= sweep[0].floors for r in sweep)
            # The row at the default r_max is the run `thresholds` wrote.
            at_default = next(r for r in sweep if r.r_max == th.R_MAX)
            committed = loaded.comparison(kind)
            assert at_default.comparison.overrides == committed.overrides
            assert at_default.comparison.harmful == committed.harmful
            for r in sweep:
                assert f"| {r.r_max:.4f} | {r.floors} | " in text
            # No risk budget sees a flip that starts after the calibrate window.
            if loaded.manifest.planted["scenario"] == "reversal":
                assert len({r.comparison.harmful for r in sweep}) == 1
                assert all(r.gate.status == "fail" for r in sweep)
            else:
                assert all(r.gate.status == "pass" for r in sweep)
