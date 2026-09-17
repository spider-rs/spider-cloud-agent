import json

from spider_optimize_train import cli, report


def test_reports_carry_fixture_only_banner(corpus_dir, tmp_path, capsys):
    run = tmp_path / "run"
    out = tmp_path / "out"
    corpus = str(corpus_dir)
    assert cli.main(["train", corpus, "--out", str(run), "--seed", "1"]) == 0
    assert cli.main(["thresholds", corpus, "--run", str(run), "--resamples", "200",
                     "--min-covered", "30", "--r-max", "0.05", "--min-sites", "20"]) == 0
    assert cli.main(["evaluate", corpus, "--run", str(run), "--quantize", "int8"]) == 0
    assert cli.main(["gates", corpus, "--run", str(run), "--resamples", "200"]) in (0, 1)
    assert cli.main(["export", corpus, "--run", str(run), "--kind", "mlp",
                     "--out", str(out / "synth-mlp.bin"), "--golden",
                     str(out / "golden-mlp.json"), "--quantize", "int8"]) == 0
    printed = capsys.readouterr().out.splitlines()

    reports = ["confidence-coverage.md", "evaluation-mlp.md", "evaluation-gbdt.md",
               "regression-report.md"]
    for name in reports:
        text = (run / name).read_text()
        assert report.BANNER in text, name
        assert text.count(report.BANNER) == text.count("\n# ") + 1, name
    assert "(the rule of three)" in (run / "regression-report.md").read_text()
    assert "Model-scored, not observed" in (run / "evaluation-mlp.md").read_text()

    assert printed
    for line in printed:
        if any(ch.isdigit() for ch in line):
            assert line.startswith(report.FIXTURE_ONLY + ": "), line

    sidecar = json.loads((out / "synth-mlp.sidecar.json").read_text())
    assert sidecar["synthetic"] is True
    assert (out / "synth-mlp.q8.npz").is_file()
    for text in [(run / n).read_text() for n in reports]:
        assert "improve" not in text.lower().replace("claims no improvement", "")
