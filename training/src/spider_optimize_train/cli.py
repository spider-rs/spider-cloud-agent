"""`spider-optimize-train synth|validate|train|evaluate|thresholds|gates|eval|tradeoff|export`."""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

from . import compare, report, splits, synth, tradeoff
from . import evaluate as ev
from . import export as ex
from . import features as feat
from . import floors as fl
from . import gates as gt
from . import thresholds as th
from .dataset import Manifest, read_rows, validate
from .models import load_run, make_split, train


def _out(line: str, synthetic: bool) -> None:
    print(report.label(synthetic) + line)


def _corpus(directory: Path, run: Path):
    manifest = Manifest.read(directory)
    doc, models, cals = load_run(run)
    rows = read_rows(directory)
    parts = make_split(rows, doc["split_name"])
    windows = {name: feat.build(parts.window(name), doc["tau"]) for name in splits.WINDOWS}
    return manifest, doc, models, cals, windows


def _thresholds(run: Path, kind: str) -> th.Thresholds:
    return th.Thresholds.from_dict(json.loads((run / f"thresholds-{kind}.json").read_text()))


def cmd_synth(args) -> int:
    manifest = synth.generate(Path(args.out), args.seed, args.pairs, args.scenario)
    _out(f"wrote {manifest.rows} rows in {manifest.pairs} pairs to {args.out} "
         f"({args.scenario} scenario)", True)
    return 0


def cmd_validate(args) -> int:
    problems = validate(Path(args.corpus))
    synthetic = False
    if (Path(args.corpus) / "manifest.json").is_file():
        synthetic = Manifest.read(Path(args.corpus)).synthetic
    for problem in problems:
        _out(problem, synthetic)
    if problems:
        _out(f"{len(problems)} violation(s); the corpus is refused.", synthetic)
        return 1
    print("valid")
    return 0


def cmd_train(args) -> int:
    corpus = Path(args.corpus)
    problems = validate(corpus)
    if problems:
        _out(f"refusing to train: {len(problems)} violation(s), run validate for the list",
             Manifest.read(corpus).synthetic)
        return 1
    result = train(corpus, model=args.model, split=args.split, seed=args.seed,
                   max_epochs=args.max_epochs, max_rounds=args.max_rounds)
    out = Path(args.out)
    result.save(out)
    synthetic = bool(result.manifest and result.manifest.synthetic)
    _out(result.tau_note, synthetic)
    _out(result.split.description, synthetic)
    for kind, cal in result.calibrations.items():
        r = cal.report
        _out(f"{kind}: calibration {r['method']} on {r['rows']} rows, ECE "
             f"{report.fmt(r['ece_before'])} -> {report.fmt(r['ece_after'])}, Brier "
             f"{report.fmt(r['brier_before'])} -> {report.fmt(r['brier_after'])}", synthetic)
    return 0


def cmd_thresholds(args) -> int:
    corpus, run = Path(args.corpus), Path(args.run)
    manifest, doc, models, cals, windows = _corpus(corpus, run)
    md = ["# Confidence and coverage\n\n", report.banner(manifest.synthetic)]
    md.append(
        f"Floors swept from 0.5 to 1.0 in steps of 0.005 on the calibrate window. A floor "
        f"needs a 95th percentile bootstrap risk of at most {args.r_max} over "
        f"{args.resamples} resamples by pair, with at least {args.min_covered} rows covered. "
        f"NaN abstains. A cell is supported with at least {args.min_sites} distinct sites in "
        "train.\n"
    )
    for kind, model in models.items():
        scored = th.score(model, cals[kind], windows["calibrate"], doc["tau"])
        chosen = th.choose(scored, windows["train"], args.r_max, args.min_covered,
                           args.min_sites, args.resamples, args.seed)
        (run / f"thresholds-{kind}.json").write_text(
            json.dumps(chosen.to_dict(), indent=2) + "\n"
        )
        md.append(f"\n## {kind}\n\n")
        rows = []
        for name, s in chosen.sweeps.items():
            if math.isnan(s.threshold):
                rows.append([name, s.rows, "NaN (abstain)", None, None, None])
            else:
                at = s.at(s.threshold)
                rows.append([name, s.rows, s.threshold, int(s.covered[at]), s.risk[at],
                             s.risk_ucb[at]])
            _out(f"{kind} {name}: floor {report.fmt(s.threshold, 3)} over {s.rows} rows",
                 manifest.synthetic)
        md.append(report.table(["edit", "rows", "floor", "covered", "risk", "risk UCB"], rows))
        pooled = chosen.sweeps["pooled"]
        md.append("\nPooled curve:\n\n")
        curve = []
        for t in (0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.99, 1.0):
            at = pooled.at(t)
            curve.append([t, int(pooled.covered[at]), pooled.coverage[at], pooled.risk[at],
                          pooled.risk_ucb[at]])
        md.append(report.table(["t", "covered", "coverage", "risk", "risk UCB"], curve))
        md.append(f"\nSupported cells: {len(chosen.support)}.\n")
    (run / "confidence-coverage.md").write_text("".join(md))
    return 0


def cmd_evaluate(args) -> int:
    corpus, run = Path(args.corpus), Path(args.run)
    manifest, doc, models, cals, windows = _corpus(corpus, run)
    for kind, model in models.items():
        thresholds = _thresholds(run, kind)
        result = ev.evaluate(model, cals[kind], windows["test"], thresholds, doc["tau"])
        quantized = None
        if args.quantize == "int8":
            quantized = ev.evaluate(ex.quantize(model), cals[kind], windows["test"], thresholds,
                                    doc["tau"])
        text = ev.render(kind, result, manifest.synthetic, quantized, cals[kind].report)
        (run / f"evaluation-{kind}.md").write_text(text)
        for name, value in result["success"].items():
            extra = ""
            if quantized:
                extra = f" (int8 {report.fmt(quantized['success'][name])})"
            _out(f"{kind} {name}: {report.fmt(value)}{extra}", manifest.synthetic)
    return 0


_gate_pairs = compare.gate_pairs


def cmd_gates(args) -> int:
    corpus, run = Path(args.corpus), Path(args.run)
    manifest, doc, models, cals, windows = _corpus(corpus, run)
    if doc["split_name"] != "chronological":
        print("gates read the chronological test window; train with --split chronological")
        return 1
    texts, failed = [], False
    for kind, model in models.items():
        pairs = _gate_pairs(model, cals[kind], windows["test"], _thresholds(run, kind),
                            doc["tau"])
        result = gt.run(pairs, args.resamples, args.seed)
        against = compare.from_outcomes(pairs, args.resamples, args.seed)
        texts.append(gt.render(result, manifest.synthetic, kind) + "\n"
                     + compare.render(against, manifest.synthetic, kind))
        why = f" ({result.reason})" if result.reason else ""
        _out(f"{kind}: gate {result.status} on {result.pairs} pairs, applied "
             f"{result.applied}{why}", manifest.synthetic)
        _out(f"{kind}: {compare.summary(against)}", manifest.synthetic)
        failed |= not result.passed
    (run / "regression-report.md").write_text("\n".join(texts))
    return 1 if failed else 0


def cmd_eval(args) -> int:
    corpus, run = Path(args.corpus), Path(args.run)
    floors = fl.Floors.read(Path(args.floors))
    manifest, doc, models, cals, windows = _corpus(corpus, run)
    if doc["split_name"] != "chronological":
        print("eval reads the chronological test window; train with --split chronological")
        return 1
    missing = sorted(set(floors.kinds) - set(models))
    if missing:
        print(f"the floors name {', '.join(missing)}, which the run did not train")
        return 1
    test = windows["test"]
    results = []
    for kind, model in models.items():
        if kind not in floors.kinds:
            continue
        thresholds = _thresholds(run, kind)
        metrics = ev.evaluate(model, cals[kind], test, thresholds, doc["tau"])["success"]
        pairs = _gate_pairs(model, cals[kind], test, thresholds, doc["tau"])
        gate = gt.run(pairs, args.resamples, args.seed)
        against = compare.from_outcomes(pairs, args.resamples, args.seed)
        result = fl.KindResult(kind, fl.metric_checks(kind, metrics, floors)
                               + fl.gate_checks(gate, floors)
                               + fl.comparison_checks(against, floors), comparison=against)
        if manifest.synthetic and manifest.planted:
            scored = th.score(model, cals[kind], test, doc["tau"])
            result.planted = fl.planted_checks(manifest.planted, scored, floors)
        results.append(result)
    (run / "eval-report.md").write_text(
        fl.render(results, manifest.synthetic, args.floors, len(test))
    )
    failed = fl.failed_lines(results)
    for line in failed:
        _out(line, manifest.synthetic)
    for r in results:
        gated = [c for c in r.checks + r.planted if c.passed is not None]
        bad = len(r.failed())
        _out(f"{r.kind}: {len(gated) - bad} of {len(gated)} floors met", manifest.synthetic)
        _out(f"{r.kind}: {compare.summary(r.comparison)}", manifest.synthetic)
    return 1 if failed else 0


def _r_max_grid(text: str) -> list[float]:
    grid = [float(v) for v in text.split(",") if v.strip()]
    if not grid or any(not 0 < v <= 1 for v in grid) or grid != sorted(grid):
        raise SystemExit("--r-max wants rising values in (0, 1], comma separated")
    return grid


def cmd_tradeoff(args) -> int:
    corpus, run = Path(args.corpus), Path(args.run)
    manifest, doc, models, cals, windows = _corpus(corpus, run)
    if doc["split_name"] != "chronological":
        print("tradeoff reads the chronological test window; train with --split chronological")
        return 1
    grid = _r_max_grid(args.r_max)
    per_kind = {}
    for kind, model in models.items():
        rows = tradeoff.sweep(model, cals[kind], windows, doc["tau"], grid, args.min_covered,
                              args.min_sites, args.resamples, args.gate_resamples, args.seed)
        per_kind[kind] = rows
        for r in rows:
            _out(f"{kind} r_max {report.fmt(r.r_max, 3)}: floors {r.floors}, "
                 f"{compare.summary(r.comparison)}, gate {r.gate.status}", manifest.synthetic)
    (run / "tradeoff.md").write_text(
        tradeoff.render(per_kind, manifest.synthetic, args.min_covered, args.min_sites)
    )
    return 0


def cmd_export(args) -> int:
    corpus, run = Path(args.corpus), Path(args.run)
    manifest, doc, models, cals, windows = _corpus(corpus, run)
    model = models[args.kind]
    thresholds = _thresholds(run, args.kind)
    tables = ex.tables_for(model, cals[args.kind], thresholds.per_code, thresholds.support)
    if args.kind == "gbdt":
        knots = len(cals[args.kind].knots)
        dropped = model.cap_bytes(
            ex.GBDT_CAP,
            lambda: ex.artifact_size(model, len(thresholds.per_code), len(thresholds.support),
                                     knots),
        )
        tables = ex.tables_for(model, cals[args.kind], thresholds.per_code, thresholds.support)
        if dropped:
            _out(f"dropped {dropped} trees to fit {ex.GBDT_CAP} bytes", manifest.synthetic)
    blob = ex.serialize(tables)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(blob)
    info = {
        "synthetic": manifest.synthetic,
        "rows": manifest.rows,
        "pairs": manifest.pairs,
        "tau": doc["tau"],
        "split": doc["split"],
        "windows": doc["windows"],
        "calibration": cals[args.kind].report,
        "pooled_threshold": None if math.isnan(thresholds.pooled) else thresholds.pooled,
        "sweeps": json.loads((run / f"thresholds-{args.kind}.json").read_text())["sweeps"],
    }
    sidecar = out.with_name(out.stem + ".sidecar.json")
    ex.write_sidecar(sidecar, blob, tables, info)
    _out(f"wrote {len(blob)} bytes to {out}, sha256 {ex.sha256(blob)}", manifest.synthetic)
    if args.golden:
        cases = ex.golden_cases(blob, windows["test"], seed=args.seed)
        Path(args.golden).parent.mkdir(parents=True, exist_ok=True)
        Path(args.golden).write_text(ex.golden_json(cases))
        _out(f"wrote {len(cases)} golden cases to {args.golden}", manifest.synthetic)
    if args.quantize == "int8":
        q = out.with_name(out.stem + ".q8.npz")
        ex.save_quantized(q, model)
        _out(f"wrote the Python-only int8 variant to {q}", manifest.synthetic)
    return 0


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="spider-optimize-train", description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)

    s = sub.add_parser("synth", help="write a synthetic corpus with planted effects")
    s.add_argument("--out", required=True)
    s.add_argument("--seed", type=int, default=1)
    s.add_argument("--pairs", type=int, default=4000)
    s.add_argument("--scenario", choices=tuple(synth.SCENARIOS), default="default",
                   help="default is the fixture corpus; reversal flips the residential "
                        "effect inside the test window; stable is the control without a flip")
    s.set_defaults(func=cmd_synth)

    s = sub.add_parser("validate", help="list every violation in a corpus; exit 1 on any")
    s.add_argument("corpus")
    s.set_defaults(func=cmd_validate)

    s = sub.add_parser("train", help="fit both kinds and their calibrations")
    s.add_argument("corpus")
    s.add_argument("--out", required=True)
    s.add_argument("--model", choices=("both", "lightgbm", "mlp"), default="both")
    s.add_argument("--split", choices=("chronological", "domain"), default="chronological")
    s.add_argument("--seed", type=int, default=1)
    s.add_argument("--max-epochs", type=int, default=None)
    s.add_argument("--max-rounds", type=int, default=None)
    s.set_defaults(func=cmd_train)

    s = sub.add_parser("thresholds", help="sweep floors, write the abstain and support tables")
    s.add_argument("corpus")
    s.add_argument("--run", required=True)
    s.add_argument("--r-max", type=float, default=th.R_MAX)
    s.add_argument("--min-covered", type=int, default=th.MIN_COVERED)
    s.add_argument("--min-sites", type=int, default=th.MIN_SITES)
    s.add_argument("--resamples", type=int, default=th.RESAMPLES)
    s.add_argument("--seed", type=int, default=0)
    s.set_defaults(func=cmd_thresholds)

    s = sub.add_parser("evaluate", help="metrics, categories and policy value on test")
    s.add_argument("corpus")
    s.add_argument("--run", required=True)
    s.add_argument("--quantize", choices=("int8",), default=None)
    s.set_defaults(func=cmd_evaluate)

    s = sub.add_parser("gates", help="the paired regression gates; exit 1 unless all pass")
    s.add_argument("corpus")
    s.add_argument("--run", required=True)
    s.add_argument("--resamples", type=int, default=gt.RESAMPLES)
    s.add_argument("--seed", type=int, default=0)
    s.set_defaults(func=cmd_gates)

    s = sub.add_parser("eval", help="check the run against a floors file; exit 1 on any miss")
    s.add_argument("corpus")
    s.add_argument("--run", required=True)
    s.add_argument("--floors", required=True)
    s.add_argument("--resamples", type=int, default=gt.RESAMPLES)
    s.add_argument("--seed", type=int, default=0)
    s.set_defaults(func=cmd_eval)

    s = sub.add_parser("tradeoff", help="re-choose the floors at several r_max and read each "
                                        "policy against the baseline on the test window")
    s.add_argument("corpus")
    s.add_argument("--run", required=True)
    s.add_argument("--r-max", default=",".join(str(v) for v in tradeoff.R_MAX_GRID),
                   help="comma separated, rising")
    s.add_argument("--min-covered", type=int, default=th.MIN_COVERED)
    s.add_argument("--min-sites", type=int, default=th.MIN_SITES)
    s.add_argument("--resamples", type=int, default=th.RESAMPLES,
                   help="bootstrap resamples for the floor sweep")
    s.add_argument("--gate-resamples", type=int, default=gt.RESAMPLES,
                   help="bootstrap resamples for the gate and the comparison")
    s.add_argument("--seed", type=int, default=0)
    s.set_defaults(func=cmd_tradeoff)

    s = sub.add_parser("export", help="write the artifact, its sidecar and golden cases")
    s.add_argument("corpus")
    s.add_argument("--run", required=True)
    s.add_argument("--kind", choices=("mlp", "gbdt"), required=True)
    s.add_argument("--out", required=True)
    s.add_argument("--golden", default=None)
    s.add_argument("--quantize", choices=("int8",), default=None)
    s.add_argument("--seed", type=int, default=0)
    s.set_defaults(func=cmd_export)
    return p


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
