"""Shared corpora and one trained run, built once per session."""

from __future__ import annotations

import copy
import json
from pathlib import Path

import pytest

from spider_optimize_train import synth
from spider_optimize_train.dataset import Manifest, read_rows
from spider_optimize_train.models import train

REPO = Path(__file__).resolve().parents[2]
GOLDEN_DIR = REPO / "training" / "fixtures" / "golden"


@pytest.fixture(scope="session")
def corpus_dir(tmp_path_factory) -> Path:
    out = tmp_path_factory.mktemp("corpus")
    synth.generate(out, seed=1, pairs=4000)
    return out


@pytest.fixture(scope="session")
def manifest(corpus_dir) -> Manifest:
    return Manifest.read(corpus_dir)


@pytest.fixture(scope="session")
def planted(manifest) -> dict:
    return manifest.planted


@pytest.fixture(scope="session")
def corpus_rows(corpus_dir) -> list[dict]:
    return read_rows(corpus_dir)


@pytest.fixture(scope="session")
def small_rows() -> list[dict]:
    return synth.generate_rows(seed=7, pairs=1500)


@pytest.fixture
def valid_rows(small_rows) -> list[dict]:
    """A fresh copy of a small valid corpus, safe to damage."""
    return copy.deepcopy(small_rows)


@pytest.fixture(scope="session")
def small_manifest(tmp_path_factory, small_rows) -> Manifest:
    out = tmp_path_factory.mktemp("small")
    synth.generate(out, seed=7, pairs=1500)
    return Manifest.read(out)


@pytest.fixture(scope="session")
def trained(corpus_dir):
    return train(corpus_dir, model="both", split="chronological", seed=1)


def write_corpus(directory: Path, rows: list[dict], manifest: Manifest) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    with open(directory / "rows.jsonl", "w") as out:
        for row in rows:
            out.write(json.dumps(row) + "\n")
    manifest.write(directory)
    return directory
