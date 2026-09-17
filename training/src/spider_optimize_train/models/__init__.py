"""One entry point that trains both model kinds on the same arrays and the same split.

A run directory holds what later steps read: `run.json` (tau, split, seed, fit
statistics), the weights of each kind, and each kind's success calibration.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from .. import calibrate, labels, splits
from .. import features as feat
from ..dataset import Manifest, read_rows
from .lightgbm_head import GbdtModel
from .mlp import MlpModel

KINDS = ("mlp", "gbdt")


def class_weights(y) -> tuple[float, float]:
    """Weights that make each class carry half the total: (negative, positive)."""
    y = np.asarray(y)
    n = len(y)
    positives = float((y > 0.5).sum())
    negatives = n - positives
    if positives == 0 or negatives == 0:
        return 1.0, 1.0
    return n / (2.0 * negatives), n / (2.0 * positives)


def sample_weights(arrays: feat.Arrays, latest_day: int, balance: tuple[float, float]):
    """Per head: class weight times recency for success, recency alone for the two
    regression heads."""
    recency = splits.recency_weight(arrays.day, latest_day)
    neg, pos = balance
    success = recency * np.where(arrays.success > 0.5, pos, neg)
    return [success, recency, recency]


def targets(arrays: feat.Arrays):
    return [arrays.success.astype(np.float64), arrays.log_millis, arrays.log_credits]


@dataclass
class TrainResult:
    split: splits.Split
    windows: dict[str, feat.Arrays]
    tau: float
    tau_note: str
    models: dict = field(default_factory=dict)
    calibrations: dict[str, calibrate.Calibration] = field(default_factory=dict)
    manifest: Manifest | None = None
    seed: int = 0
    split_name: str = "chronological"

    def save(self, out: Path) -> None:
        out = Path(out)
        out.mkdir(parents=True, exist_ok=True)
        for kind, model in self.models.items():
            if kind == "mlp":
                model.save(out / "mlp.npz")
            else:
                model.save(out / "gbdt")
            (out / f"calibration-{kind}.json").write_text(
                json.dumps(self.calibrations[kind].to_dict(), indent=2) + "\n"
            )
        doc = {
            "tau": self.tau,
            "tau_note": self.tau_note,
            "seed": self.seed,
            "split": self.split.description,
            "split_name": self.split_name,
            "windows": {
                name: {"rows": len(self.windows[name]), "days": self.split.days()[name]}
                for name in splits.WINDOWS
            },
            "kinds": sorted(self.models),
            "fit": {kind: getattr(m, "info", {}) for kind, m in self.models.items()},
            "synthetic": bool(self.manifest and self.manifest.synthetic),
        }
        (out / "run.json").write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")


def make_split(rows: list[dict], split: str, fold: int = 0) -> splits.Split:
    if split == "chronological":
        return splits.chronological(rows)
    if split == "domain":
        return splits.domain_folds(rows)[fold]
    raise ValueError(f"unknown split {split!r}")


def train(
    dataset,
    model: str = "both",
    split: str = "chronological",
    seed: int = 1,
    max_epochs: int | None = None,
    max_rounds: int | None = None,
) -> TrainResult:
    """Fit the requested kinds and their success calibrations. `dataset` is a corpus
    directory or a list of rows."""
    manifest = None
    if isinstance(dataset, (str, Path)):
        manifest = Manifest.read(Path(dataset))
        rows = read_rows(Path(dataset))
    else:
        rows = list(dataset)
    parts = make_split(rows, split)
    # Tau is read from the windows the model is fitted, stopped and calibrated on. The
    # test window is held back from it like from everything else, so nothing the gate
    # reads has shaped the labels.
    tau, tau_note = labels.choose_tau(parts.train + parts.tune + parts.calibrate)
    tau_note = f"{tau_note}, outside the test window"
    windows = {name: feat.build(parts.window(name), tau) for name in splits.WINDOWS}
    train_w, tune_w = windows["train"], windows["tune"]
    if len(train_w) == 0 or len(tune_w) == 0 or len(windows["calibrate"]) == 0:
        raise ValueError("a window is empty; the corpus spans too few days")

    latest = int(train_w.day.max())
    balance = class_weights(train_w.success)
    weights = sample_weights(train_w, latest, balance)
    weights_tune = sample_weights(tune_w, latest, balance)

    kinds = KINDS if model == "both" else ({"lightgbm": "gbdt"}.get(model, model),)
    result = TrainResult(parts, windows, tau, tau_note, manifest=manifest, seed=seed,
                         split_name=split)
    for kind in kinds:
        if kind == "mlp":
            fitted = MlpModel.fit(
                train_w.X, targets(train_w), weights, tune_w.X, targets(tune_w), weights_tune,
                seed, **({} if max_epochs is None else {"max_epochs": max_epochs}),
            )
        elif kind == "gbdt":
            fitted = GbdtModel.fit(
                train_w.X, targets(train_w), weights, tune_w.X, targets(tune_w), weights_tune,
                seed, **({} if max_rounds is None else {"max_rounds": max_rounds}),
            )
        else:
            raise ValueError(f"unknown model {model!r}")
        result.models[kind] = fitted
        cal = windows["calibrate"]
        z = fitted.predict(cal.X)[0]
        result.calibrations[kind] = calibrate.fit(z, cal.success, int((~cal.baseline).sum()))
    return result


def load_run(run: Path) -> tuple[dict, dict, dict[str, calibrate.Calibration]]:
    run = Path(run)
    doc = json.loads((run / "run.json").read_text())
    models, cals = {}, {}
    for kind in doc["kinds"]:
        models[kind] = MlpModel.load(run / "mlp.npz") if kind == "mlp" else GbdtModel.load(
            run / "gbdt"
        )
        cals[kind] = calibrate.Calibration.from_dict(
            json.loads((run / f"calibration-{kind}.json").read_text())
        )
    return doc, models, cals
