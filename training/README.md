# spider-optimize training

This directory turns comparison rows into the scorer `spider-optimize` loads: two model
kinds trained on the same arrays, a success calibration, a per-edit success floor with
an abstain table, a support table, paired regression gates, and the binary artifact the
Rust crate reads. No paid traffic is involved anywhere in this directory: nothing here
sends a request, and the only corpus it has ever run on is the synthetic one it
generates.

## The FIXTURE-ONLY rule

There is no real data yet. Every number the pipeline prints or writes from a corpus
whose manifest says `synthetic: true` carries the `FIXTURE-ONLY` label, and every
report opens with a banner saying so. Those numbers show that the pipeline recovers
effects planted in `synth.py`. They are not evidence that an edit helps a real request,
and nothing in this directory claims an improvement. `test_reports_carry_fixture_only_banner`
fails if a printed line with a number loses the label or a report loses the banner.

## Setup

```bash
cd training
uv sync
uv run ruff check .
uv run pytest -q
```

The full suite runs in well under three minutes; it trains both kinds once on a 4000
pair synthetic corpus and shares that run across tests.

## Commands

A corpus is a directory with `rows.jsonl`, one row per line as
`spider_optimize::row::comparison_row` writes it, and `manifest.json`. Real corpora go
under `training/data/`, which git ignores.

```bash
# Write a synthetic corpus with planted effects.
uv run spider-optimize-train synth --out /tmp/opt/corpus --seed 1

# List every violation; exit 1 on any.
uv run spider-optimize-train validate /tmp/opt/corpus

# Fit both kinds and each kind's success calibration into a run directory.
uv run spider-optimize-train train /tmp/opt/corpus --out /tmp/opt/run --seed 1

# Sweep the per-edit floors on the calibrate window; write thresholds-<kind>.json
# and confidence-coverage.md.
uv run spider-optimize-train thresholds /tmp/opt/corpus --run /tmp/opt/run

# Metrics, per-category tables and policy value on the test window, with an int8
# column; write evaluation-<kind>.md.
uv run spider-optimize-train evaluate /tmp/opt/corpus --run /tmp/opt/run --quantize int8

# Paired gates on the chronological test window; write regression-report.md and
# exit 1 unless every kind passes.
uv run spider-optimize-train gates /tmp/opt/corpus --run /tmp/opt/run

# The artifact, its sidecar, and 64 golden cases.
uv run spider-optimize-train export /tmp/opt/corpus --run /tmp/opt/run --kind mlp \
  --out /tmp/opt/out/synth-mlp.bin --golden /tmp/opt/out/golden-mlp.json --quantize int8
```

`train` refuses a corpus the validator refuses. `train --model lightgbm` or `--model mlp`
fits one kind; `--split domain` holds out sites by `dk % 5` instead of days, and `gates`
refuses a run trained that way.

### What each step does

`validate` refuses fewer than 200 rows or 100 pairs, a pair without exactly one
baseline arm, arms of one pair on different days or sites, a learnable edit with fewer
than 20 candidate rows or a single outcome, any string that looks like a host or holds
`://`, a feature outside [-1, 1] or not finite, a feature vector of the wrong length,
and versions that differ from the manifest.

`train` recomputes `content_ok` from the stored scalars with tau at the 5th percentile
of baseline repeat jaccard (the default 0.80 when fewer than 20 repeats exist). The
success label is a success whose content was not judged broken. The split is 60, 15,
10 and 15 percent of the days, in order. Both kinds see
`base[152] ++ edit_feats[96] ++ pinned_bucket[4]`, with class weights times a 30 day
recency half-life. The calibrate window chooses isotonic regression at 500 candidate
rows or more and Platt scaling below that.

`thresholds` covers a candidate at floor `t` when its calibrated success is at least `t`
and its predicted credits per correct result are below its own baseline arm's. The
floor is the smallest `t` whose 95th percentile risk over 1000 pair resamples is at most
`--r-max` (0.01) with at least `--min-covered` (200) rows covered; otherwise NaN, which
abstains. A cell `need << 24 | ext << 16 | mem << 8 | edit_code` is supported when train
saw it on `--min-sites` (50) distinct sites.

`gates` compares, pair by pair, the arm the gated policy would have run with the
baseline arm: the success and content lower bounds at -0.005, the credits per correct
result upper bound at or under the baseline point estimate, p50 within 10 percent and
p90 within 20 percent. Fewer than 300 pairs is `insufficient`, which fails.

## Edit codes and cells

Edit code 0 is keep. A learnable key's code is `1 + rank` among the learnable keys of
`fixtures/schema-v1.json` in key order, the rule `spider_optimize::schema::edit_code`
follows, so `request` is 1 and `network_blacklist` is 9. `schema-v1.json` is generated
from the crate's constants and is the only source this package reads offsets and codes
from.

## The artifact

`export.py` documents the layout at the top of the module. The heads are the success
logit, `log1p` millis and `log1p` credits. The Rust input has no pinned count and the
optimizer only edits fields the caller left alone, so export folds the pinned bucket in
at zero pins. No run of printable ASCII after the 16 byte header is longer than 8
bytes, and none looks like a host: the writer moves the lowest byte of a float to break
one, and `audit` refuses the blob otherwise. The size limit is 2,000,000 bytes, and a
GBDT export drops trailing trees round robin to stay under 1,500,000.

`predict.py` is the reference reader. It parses the bytes, never a model object, and
computes the heads, support and abstain flags as its module docstring describes. The
golden files are written from it.

`--quantize int8` writes a Python-only variant with a per-tensor scale and zero point,
and `evaluate --quantize int8` reruns every metric on it beside the FP32 column. It is
not the artifact format. The Rust reader stays FP32.

## Fixtures

`fixtures/golden/` holds `synth-mlp.bin`, `synth-gbdt.bin`, their sidecars and 64
golden cases each, from this sequence:

```bash
uv run spider-optimize-train synth --out /tmp/opt/corpus --seed 1
uv run spider-optimize-train train /tmp/opt/corpus --out /tmp/opt/run --seed 1
uv run spider-optimize-train thresholds /tmp/opt/corpus --run /tmp/opt/run \
  --min-covered 30 --r-max 0.05 --min-sites 20
uv run spider-optimize-train export /tmp/opt/corpus --run /tmp/opt/run --kind mlp \
  --out fixtures/golden/synth-mlp.bin --golden fixtures/golden/golden-mlp.json
uv run spider-optimize-train export /tmp/opt/corpus --run /tmp/opt/run --kind gbdt \
  --out fixtures/golden/synth-gbdt.bin --golden fixtures/golden/golden-gbdt.json
```

The floors there are relaxed on purpose. At the defaults a 4000 pair corpus abstains on
every edit, and an artifact whose floors are all NaN would leave the reader's floor
comparison untested. The sidecars record the settings used.

A golden case stores a non-finite input slot as `1e39`, which JSON can hold and which
overflows to infinity when read into a float32.

`test_python_reader_matches_rust_golden` reads `spider-optimize/tests/fixtures/golden-v1.json`
and `spider-optimize/assets/spider-optimize-v1.bin` and requires `predict.py` to match
within 1e-5. Until the crate's artifact reader and its hand-made fixture are on the
branch, the test skips and says which files are missing.

`docs/mutations.md` lists the guards that were broken by hand and the tests that went
red.
