# Golden fixtures

**FIXTURE-ONLY.** These are synthetic models trained on a corpus that `synth.py`
generated with planted effects. No page was fetched to make them, and no real request
is behind any number here. They prove that the Python trainer and the Rust reader agree
on the artifact format, byte for byte and within 1e-5 on every score. They say nothing
about accuracy. Do not ship them as a scorer for real traffic.

| file | what it is |
|---|---|
| `synth-mlp.bin` | MLP artifact, three heads of `248 -> 48 -> 16 -> 1` |
| `synth-mlp.sidecar.json` | its versions, size, sha256, calibration report, floors and sweeps |
| `golden-mlp.json` | 64 cases scored by `predict.py` from `synth-mlp.bin` |
| `synth-gbdt.bin` | GBDT artifact, 79, 300 and 300 trees, at most 29 nodes each |
| `synth-gbdt.sidecar.json` | the same record for the GBDT artifact |
| `golden-gbdt.json` | 64 cases scored by `predict.py` from `synth-gbdt.bin` |

The crate carries copies. A test on each side fails if a copy differs by a byte. On
the Rust side it is `fixtures_are_the_trainer_exports_byte_for_byte` in
`spider-optimize/tests/parity.rs`. On the Python side it is
`test_the_crate_ships_the_trainer_fixtures_byte_for_byte` in `tests/test_export.py`.

| here | in `spider-optimize/` |
|---|---|
| `synth-mlp.bin` | `assets/spider-optimize-v1.bin` |
| `golden-mlp.json` | `tests/fixtures/golden-v1.json` |
| `synth-gbdt.bin` | `tests/fixtures/gbdt-v1.bin` |
| `golden-gbdt.json` | `tests/fixtures/golden-gbdt-v1.json` |

A golden file is a JSON array of `{"base", "edit", "cell", "expect": {"p_success",
"latency_ms", "credits", "support"}}`. It holds only numbers, `null` and those keys.
`null` in an input is a non-finite slot. `null` in `p_success` means the reader must
return NaN.

## How they were produced

They were produced from `training/` on 2026-09-16 with seed 7. The base commit was
`ec75b64`, plus the exporter fixes in the same change as this file.

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

Then each file was copied to the crate path in the table above. Git ignores `data/`,
and nothing under it is committed.

`synth` wrote 8000 rows in 4000 pairs. `train` chose tau 0.8692 and Platt calibration
for both kinds. The calibrate window had 756 rows, 378 of them candidates, which is
under the 500 that isotonic needs.

That tau was read from the repeat pairs of the whole corpus. Since 2026-09-17, `train`
reads it from the train, tune and calibrate rows only, so the test window shapes no
label. On this corpus the new rule gives tau 0.8610 and moves one train label. The
committed files were regenerated with that rule on 2026-09-17. `synth-mlp.bin` now has
sha256 `b51910d8501e6e307a622790526224e500c68e3330cc1a32fc658012b37d706f`, and
`synth-gbdt.bin` has
`f1d59d4d3895170f22151bfde45666d0e234134e9f1972f274bea9b06dfe0b7a`. The crate's four
copies were replaced in the same change.

The floors are relaxed on purpose. At the defaults, `--min-covered 200 --r-max 0.01
--min-sites 50`, this corpus abstains on every edit code. An artifact whose floors are
all NaN would leave the floor comparison in the golden cases untested. With the relaxed
settings, the MLP has a floor of 0.5 on code 3, which is `wait_for`. The GBDT has the same
floor on codes 1 and 3, where code 1 is `request`. Every other code is NaN, and each support
table holds 14 cells.

`gates` exits 1 on these runs. Success and content pass, but credits per correct
result and p90 latency fail for both kinds. A rerun writes the full table to
`data/run-v1/regression-report.md`. Relaxed floors on a synthetic corpus are expected to
fail this way, and it does not matter for a format fixture. It is one more reason these
are not a model to ship.

## Sizes

Measured on the files committed here:

| artifact | bytes | limit |
|---|---|---|
| `synth-mlp.bin` (`spider-optimize-v1.bin`) | 153,229 | 2,097,152 in `xtask/artifact-baselines.json`, 2,000,000 in the reader |
| `synth-gbdt.bin` (`gbdt-v1.bin`) | 259,206 | 1,500,000 GBDT export cap, 2,000,000 in the reader |

`cargo run -p xtask -- leakcheck` audits the shipped MLP artifact. It checks that the
artifact is under its baseline, has no printable run over 8 bytes after the header, and
has no domain-like string.

## Regenerating

Rerun the sequence above and copy the four files. A different LightGBM or numpy build
can move the last bits of a weight, so a rerun may not reproduce these bytes. That is
fine. Replace all four copies and both sidecars together. Then run
`cargo test -p spider-optimize --all-features` and `uv run pytest -q`. The Rust reader
in `spider-optimize/src/artifact.rs` is the contract. If parity fails, fix the side that
disagrees with it, and keep the 1e-5 tolerance.
