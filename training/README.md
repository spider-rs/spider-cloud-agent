# spider-optimize training

This directory turns comparison rows into the scorer that `spider-optimize` loads.
It produces these parts:

- two model kinds trained on the same arrays
- a success calibration
- a per-edit success floor with an abstain table
- a support table
- paired regression gates
- the binary artifact the Rust crate reads

This directory uses no paid traffic. Nothing here sends a request. The only corpus it
has ever run on is the synthetic one it generates.

## The FIXTURE-ONLY rule

There is no real data yet. A corpus whose manifest says `synthetic: true` is
synthetic. Every number the pipeline prints or writes from such a corpus carries the
`FIXTURE-ONLY` label, and every report opens with a banner saying so. Those numbers
show that the pipeline recovers effects planted in `synth.py`. They are not evidence
that an edit helps a real request. Nothing in this directory claims an improvement.
`test_reports_carry_fixture_only_banner` fails if a printed line with a number loses
the label, or if a report loses the banner.

## Setup

```bash
cd training
uv sync
uv run ruff check .
uv run pytest -q
```

The full suite runs in under four minutes, and in about two on this machine. It
trains both kinds once on a 4000 pair synthetic corpus and shares that run across
tests. `tests/test_scenarios.py` trains once more on each of the two 20000 pair
regression scenarios.

## Commands

A corpus is a directory with two files. `rows.jsonl` has one row per line, in the
form `spider_optimize::row::comparison_row` writes. The other file is `manifest.json`.
Real corpora go under `training/data/`, which git ignores.

```bash
# Write a synthetic corpus with planted effects. --scenario reversal flips the residential
# effect inside the test window; --scenario stable is the control with no flip.
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

# Paired gates on the chronological test window, with the policy read against the
# heuristic baseline; write regression-report.md and exit 1 unless every kind passes.
uv run spider-optimize-train gates /tmp/opt/corpus --run /tmp/opt/run

# Choose the floors again at each r_max and read what each risk budget buys on the
# test window; write tradeoff.md.
uv run spider-optimize-train tradeoff /tmp/opt/corpus --run /tmp/opt/run \
  --r-max 0.005,0.01,0.02,0.05,0.1

# Check the run against a floors file: metric floors per kind on the test window, the
# applied count and gate status, and planted effect recovery on a synthetic corpus;
# write eval-report.md and exit 1 on any miss.
uv run spider-optimize-train eval /tmp/opt/corpus --run /tmp/opt/run \
  --floors evals/synth-seed7-floors.json

# The artifact, its sidecar, and 64 golden cases.
uv run spider-optimize-train export /tmp/opt/corpus --run /tmp/opt/run --kind mlp \
  --out /tmp/opt/out/synth-mlp.bin --golden /tmp/opt/out/golden-mlp.json --quantize int8
```

`evals/run.sh` runs that whole sequence on the seed 7 corpus. It compares the export
with `fixtures/golden` byte for byte. `evals/README.md` lists the floors, the measured
reference values and how to refresh them.

`train` refuses a corpus the validator refuses. `train --model lightgbm` or
`--model mlp` fits one kind. `--split domain` holds out sites by `dk % 5` instead of
days, and `gates` refuses a run trained that way.

### What each step does

`validate` refuses a corpus with any of these:

- fewer than 200 rows or 100 pairs
- a pair without exactly one baseline arm
- arms of one pair on different days or sites
- a learnable edit with fewer than 20 candidate rows or a single outcome
- a row whose edit key is not learnable. Such a key has no edit code or cell, so
  building the arrays would fail on it.
- any string that looks like a host or holds `://`
- a feature outside [-1, 1] or not finite
- a feature vector of the wrong length
- versions that differ from the manifest

`train` recomputes `content_ok` from the stored scalars. It sets tau at the 5th
percentile of baseline repeat jaccard. With fewer than 20 repeats, tau is the default
0.80. Tau comes from the train, tune and calibrate rows. The test window shapes nothing
before the gate. `test_the_test_window_is_never_read_before_the_gate` rewrites every
test row and checks that the model, the calibration and the floors come out byte for
byte the same.

The success label is a success whose content was not judged broken. The split is 60,
15, 10 and 15 percent of the days, in order. Both kinds see
`base[152] ++ edit_feats[96] ++ pinned_bucket[4]`, with class weights times a 30 day
recency half-life. On the calibrate window, 500 or more candidate rows get isotonic
regression. Fewer get Platt scaling. Platt is also used when isotonic pools into
fewer than the two knots the artifact needs.

`thresholds` covers a candidate at floor `t` when two things hold. Its calibrated
success is at least `t`. Its predicted credits per correct result are below those of
its own baseline arm. The floor is the smallest `t` that meets both limits below:

- the 95th percentile risk over 1000 pair resamples is at most `--r-max`, default 0.01
- at least `--min-covered` rows are covered, default 200

If no `t` qualifies, the floor is NaN, which abstains. A cell
`need << 24 | ext << 16 | mem << 8 | edit_code` is supported when train saw it on
`--min-sites` distinct sites, default 50.

`gates` compares two arms pair by pair: the arm the gated policy would have run, and
the baseline arm. It checks these limits:

- the success and content lower bounds at -0.005
- the 95 percent upper bound of the policy's credits per correct result minus the
  baseline's, resampled by pair, at or under zero
- p50 within 10 percent
- p90 within 20 percent

Fewer than 300 pairs is `insufficient`, which fails. A policy that applied no edit on
the window is also `insufficient`. Its arm is the baseline on every pair, so there is
nothing to compare.

The report ends with the table from `compare.py`. It reads the policy against the
heuristic baseline on the same pairs. The table shows overrides and coverage, success
and content_ok rates, and credits per correct result. It shows harmful overrides with
their rate and that rate's upper bound. A harmful override is a success the baseline
had and the policy lost, or broken content. It also shows helpful overrides, which are
correct where the baseline failed, and the net success delta.

`eval` reads a JSON floors file. For each kind, it checks:

- the test window metrics against the file
- the number of applied edits and the gate status against exact expectations
- the policy's coverage and harmful override rate, where the file bounds them
- on a synthetic corpus, the model's predicted uplift on each planted effect the row
  data can locate

`evals/README.md` has the file format and the effects it reaches.

`tradeoff` chooses the floors again at each `--r-max` on the calibrate window. For
each resulting table, it reads the policy against the baseline on the test window. It
writes one row per risk budget into `tradeoff.md`. `evals/README.md` shows the tables
for the two regression scenarios and explains why they are flat.

`synth --scenario reversal` plants a residential effect. The effect holds through the
days the model is fitted, stopped and calibrated on, then flips inside the test window.
The gate has to reject an artifact that applied the edit on the evidence it had.
`--scenario stable` is the same plant with no flip. It is the control that a passing
run must clear with edits applied. `evals/README.md` describes both and the numbers
they measure.

A pair counts as regressed (`labels.regressed`) when the baseline succeeded and the
candidate did not, or when the candidate succeeded and its content was judged broken.
The content clause applies only to a candidate that succeeded. A failed candidate
beside a failed baseline lost nothing, whatever its content scalars say.

## Edit codes and cells

Edit code 0 is keep. A learnable key's code is `1 + rank` among the learnable keys of
`fixtures/schema-v1.json`, in key order. `spider_optimize::schema::edit_code` follows
the same rule, so `request` is 1 and `network_blacklist` is 9. `schema-v1.json` is
generated from the crate's constants. It is the only source this package reads offsets
and codes from.

## The artifact

`export.py` documents the layout at the top of the module. The heads are the success
logit, `log1p` millis and `log1p` credits. `spider-optimize/src/artifact.rs` is the
contract. If the two ever disagree, this package changes.

The Rust input has no pinned count, and the optimizer only edits fields the caller
left alone. Export therefore folds the pinned bucket in at zero pins. The reader
refuses a non-finite threshold, so a split on a pinned column becomes a split on slot
0 instead. That split sits at 2.0 to always go left, or at -2.0 to always go right,
with the missing bit to match.

A calibration is written as `(kind, knots)`. `(0, 0)` is identity, `(1, 0)` is Platt
and `(2, n >= 2)` is isotonic. The exporter refuses an identity calibration on the
success head, because the reader would pass it through as the probability.

After the 16 byte header, no run of printable ASCII is longer than 8 bytes, and none
looks like a host. To break such a run, the writer moves the lowest byte of a float,
never a calibration float. `audit` refuses the blob if a run remains, then reads the
final bytes with `predict.read`. The size limit is 2,000,000 bytes. A GBDT export
drops trailing trees round robin to stay under 1,500,000.

`predict.py` is the reference reader. It parses the bytes, never a model object. It
refuses what the Rust reader refuses. It computes the heads, support and abstain flags
in float32, in the Rust reader's order, as its module docstring describes. The golden
files are written from its output.

`--quantize int8` writes a Python-only variant with a per-tensor scale and zero point.
`evaluate --quantize int8` reruns every metric on it next to the FP32 column. This
variant is not the artifact format. The Rust reader stays FP32.

## Fixtures

`fixtures/golden/` holds `synth-mlp.bin`, `synth-gbdt.bin`, their sidecars and 64
golden cases each. `fixtures/golden/README.md` gives the exact commands, the seed and
the sizes. The crate ships the same files. The MLP pair is
`spider-optimize/assets/spider-optimize-v1.bin` and `tests/fixtures/golden-v1.json`.
The GBDT pair is `tests/fixtures/gbdt-v1.bin` and `golden-gbdt-v1.json`. A test on each
side fails if the copies differ by a byte.

A golden case stores a non-finite input slot as `null`, which is how the Rust parity
test reads it. The reader also accepts the older `1e39`, which overflows to infinity in
float32.

`test_python_reader_matches_rust_golden` and its GBDT twin read the crate's fixtures.
They require `predict.py` to match within 1e-5, the tolerance of the Rust parity test.
A missing fixture fails the test and never skips it.

`docs/mutations.md` lists the guards broken by hand and the tests that went red.
