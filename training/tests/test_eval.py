"""The `eval` command against the committed quick floors, on the small seed 7 corpus
`evals/run.sh --quick` builds."""

import json
from pathlib import Path

import pytest

from spider_optimize_train import cli, floors, synth

EVALS = Path(__file__).resolve().parents[1] / "evals"
QUICK_FLOORS = EVALS / "synth-quick-floors.json"


@pytest.fixture(scope="module")
def quick_run(tmp_path_factory):
    """The corpus and run `evals/run.sh --quick` makes: seed 7, 1500 pairs, default
    sweep settings, so every floor abstains and the policy applies no edit."""
    root = tmp_path_factory.mktemp("quick")
    corpus, run = root / "corpus", root / "run"
    synth.generate(corpus, seed=7, pairs=1500)
    assert cli.main(["train", str(corpus), "--out", str(run), "--seed", "7"]) == 0
    assert cli.main(["thresholds", str(corpus), "--run", str(run), "--seed", "7"]) == 0
    return corpus, run


def run_eval(corpus, run, floors_path, capsys):
    code = cli.main(["eval", str(corpus), "--run", str(run), "--floors", str(floors_path),
                     "--resamples", "200"])
    printed = capsys.readouterr().out.splitlines()
    return code, printed, (run / "eval-report.md").read_text()


def test_eval_passes_on_the_seed_corpus_floors(quick_run, capsys):
    corpus, run = quick_run
    code, printed, text = run_eval(corpus, run, QUICK_FLOORS, capsys)
    assert code == 0, printed
    assert text.startswith("# Eval report\n\n> **FIXTURE-ONLY.**")
    assert "Every floor met." in text
    assert " fail " not in text
    for kind in ("mlp", "gbdt"):
        assert f"\n## {kind}\n" in text
        assert f"{kind}: 12 of 12 floors met" in [
            line.removeprefix("FIXTURE-ONLY: ") for line in printed
        ]
    for line in printed:
        if any(ch.isdigit() for ch in line):
            assert line.startswith("FIXTURE-ONLY: "), line
    # The gate lines the file asks for: no edit applied, insufficient.
    assert "| applied edits | 0 | = 0 | pass |" in text
    assert "| gate status | insufficient | = insufficient | pass |" in text
    assert "improve" not in text.lower().replace("claims no improvement", "")


def test_eval_fails_when_a_floor_is_impossible(quick_run, tmp_path, capsys):
    corpus, run = quick_run
    doc = json.loads(QUICK_FLOORS.read_text())
    doc["kinds"]["mlp"]["min_auroc"] = 1.01
    doc["kinds"]["gbdt"]["max_mae_millis"] = 0
    doc["expect_applied"] = 3
    path = tmp_path / "impossible.json"
    path.write_text(json.dumps(doc))
    code, printed, text = run_eval(corpus, run, path, capsys)
    assert code == 1
    lines = [line.removeprefix("FIXTURE-ONLY: ") for line in printed]
    assert any(line.startswith("mlp auroc: ") and "misses >= 1.0100" in line for line in lines)
    assert any(line.startswith("gbdt mae_millis: ") and "misses <= 0" in line for line in lines)
    assert sum("applied edits: 0 misses = 3" in line for line in lines) == 2
    assert "4 floor(s) missed." in text
    assert "| auroc | " in text and "| >= 1.0100 | fail |" in text


def test_eval_reports_planted_uplift(quick_run, tmp_path, capsys):
    corpus, run = quick_run
    code, printed, text = run_eval(corpus, run, QUICK_FLOORS, capsys)
    assert code == 0
    planted = json.loads((corpus / "manifest.json").read_text())["planted"]
    wait = planted["wait"]["success_to"] - planted["wait"]["success_from"]
    for kind in ("mlp", "gbdt"):
        section = text.split(f"\n## {kind}\n", 1)[1].split("\n## ", 1)[0]
        rows = {line.split(" | ")[0].strip("| "): line for line in section.splitlines()
                if line.startswith("| ") and " rows" in line and "| effect |" not in line}
        assert set(rows) == {
            "wait on a cold markup site",
            "browser mode on an empty site",
            "residential proxy on a blocked site, before the flip",
            "residential proxy on a blocked site, after the flip",
            "blocked stylesheets break the page",
            "first party blacklist breaks the page",
        }
        # The wait uplift the model predicts sits near the planted 0.35, and clears the
        # floor with room.
        cells = rows["wait on a cold markup site"].split(" | ")
        predicted = float(cells[1])
        assert 0.1 <= predicted and abs(predicted - wait) < 0.15
        assert cells[3] == "pass"
        # A break is checked as a drop of the success label.
        assert "| <= -0.1000 | pass |" in rows["blocked stylesheets break the page"]
        # The test window lies past the residential flip, so the pre-flip uplift is
        # unreachable and the post-flip rows are reported without a gate.
        assert "| n/a | n/a |" in rows["residential proxy on a blocked site, before the flip"]
        assert "| reported | n/a |" in rows["residential proxy on a blocked site, after the flip"]

    # An uplift no model reaches fails every gated effect, and nothing else.
    doc = json.loads(QUICK_FLOORS.read_text())
    doc["min_planted_uplift"] = 0.95
    path = tmp_path / "steep.json"
    path.write_text(json.dumps(doc))
    code, printed, text = run_eval(corpus, run, path, capsys)
    assert code == 1
    lines = [line.removeprefix("FIXTURE-ONLY: ") for line in printed]
    assert sum("wait on a cold markup site: " in line for line in lines) == 2
    assert not any("residential" in line for line in lines)
    assert "8 floor(s) missed." in text


def test_floors_refuse_an_unknown_metric(tmp_path):
    path = tmp_path / "floors.json"
    path.write_text(json.dumps({"kinds": {"mlp": {"min_accuracy": 0.9}}}))
    with pytest.raises(ValueError, match="unknown floor"):
        floors.Floors.read(path)
    path.write_text(json.dumps({"expect_applied": 0}))
    with pytest.raises(ValueError, match="kinds"):
        floors.Floors.read(path)
