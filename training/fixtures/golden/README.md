# Golden fixtures

**FIXTURE-ONLY.** These are synthetic models trained on a corpus that `synth.py`
generated with planted effects. No page was fetched to make them and no real request
is behind any number here. They prove that the Python trainer and the Rust reader agree
on the artifact format, byte for byte and within 1e-5 on every score. They say nothing
about accuracy, and nothing should ship them as a scorer for real traffic.

| file | what it is |
|---|---|
| `synth-mlp.bin` | MLP artifact, three heads of `248 -> 48 -> 16 -> 1` |
| `synth-mlp.sidecar.json` | its versions, size, sha256, calibration report, floors and sweeps |
| `golden-mlp.json` | 64 cases scored by `predict.py` from `synth-mlp.bin` |
| `synth-gbdt.bin` | GBDT artifact, 79, 300 and 300 trees, at most 29 nodes each |
| `synth-gbdt.sidecar.json` | the same record for the GBDT artifact |
| `golden-gbdt.json` | 64 cases scored by `predict.py` from `synth-gbdt.bin` |

The crate carries copies, and a test on each side fails if a copy differs by a byte
(`fixtures_are_the_trainer_exports_byte_for_byte` in `spider-optimize/tests/parity.rs`,
`test_the_crate_ships_the_trainer_fixtures_byte_for_byte` in `tests/test_export.py`):

| here | in `spider-optimize/` |
|---|---|
| `synth-mlp.bin` | `assets/spider-optimize-v1.bin` |
| `golden-mlp.json` | `tests/fixtures/golden-v1.json` |
| `synth-gbdt.bin` | `tests/fixtures/gbdt-v1.bin` |
| `golden-gbdt.json` | `tests/fixtures/golden-gbdt-v1.json` |

A golden file is a JSON array of `{"base", "edit", "cell", "expect": {"p_success",
"latency_ms", "credits", "support"}}`. It holds numbers, `null` and those keys, nothing
else. `null` in an input is a non-finite slot; `null` in `p_success` means the reader
must return NaN.

## How they were produced

From `training/`, on 2026-09-16, at base commit `ec75b64` plus the exporter fixes in
the same change as this file, with seed 7:

```bash
uv sync --frozen
uv run spider-optimize-train synth --out data/synth-v1 --seed 7
uv run spider-optimize-train validate data/synth-v1
uv run spider-optimize-train train data/synth-v1 --out data/run-v1 --model both --seed 7
uv run spider-optimize-train thresholds data/synth-v1 --run data/run-v1 --seed 7 \
  --min-covered 30 --r-max 0.05 --min-sites 20
uv run spider-optimize-train gates data/synth-v1 --run data/run-v1 --seed 7
uv run spider-optimize-train evaluate data/synth-v1 --run data/run-v1 --quantize int8
uv run spider-optimize-train export data/synth-v1 --run data/run-v1 --kind mlp --seed 7 \
  --out fixtures/golden/synth-mlp.bin --golden fixtures/golden/golden-mlp.json
uv run spider-optimize-train export data/synth-v1 --run data/run-v1 --kind gbdt --seed 7 \
  --out fixtures/golden/synth-gbdt.bin --golden fixtures/golden/golden-gbdt.json
```

Then each file was copied to the crate path in the table above. `data/` is ignored by
git and nothing under it is committed.

`synth` wrote 8000 rows in 4000 pairs; `train` chose tau 0.8692 and Platt calibration
for both kinds (756 calibrate rows, 378 of them candidates, under the 500 isotonic
needs).

The floors are relaxed on purpose. At the defaults (`--min-covered 200 --r-max 0.01
--min-sites 50`) this corpus abstains on every edit code, and an artifact whose floors
are all NaN would leave the floor comparison in the golden cases untested. With the
relaxed settings the MLP has a floor of 0.5 on code 3 (`wait_for`), the GBDT on codes 1
(`request`) and 3, every other code is NaN, and each support table holds 14 cells.

`gates` exits 1 on these runs: success and content pass, but credits per correct result
and p90 latency fail for both kinds (the full table is in `data/run-v1/regression-report.md`
when you rerun it). That is expected of relaxed floors on a synthetic corpus and does
not matter for a format fixture; it is one more reason these are not a model to ship.

## Sizes

Measured on the files committed here:

| artifact | bytes | limit |
|---|---|---|
| `synth-mlp.bin` (`spider-optimize-v1.bin`) | 153,229 | 2,097,152 in `xtask/artifact-baselines.json`, 2,000,000 in the reader |
| `synth-gbdt.bin` (`gbdt-v1.bin`) | 256,702 | 1,500,000 GBDT export cap, 2,000,000 in the reader |

`cargo run -p xtask -- leakcheck` audits the shipped MLP artifact: under its baseline, no
printable run over 8 bytes after the header, and no domain-like string.

## Regenerating

Rerun the sequence above and copy the four files. A different LightGBM or numpy build
can move the last bits of a weight, so a rerun may not reproduce these bytes. That is
fine: replace all four copies
and both sidecars together, then run `cargo test -p spider-optimize --all-features` and
`uv run pytest -q`. The Rust reader in `spider-optimize/src/artifact.rs` is the
contract; if parity fails, fix the side that disagrees with it and keep the 1e-5
tolerance.
