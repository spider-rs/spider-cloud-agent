# Evals

**FIXTURE-ONLY.** Every number in this directory comes from the synthetic corpus that
`synth.py` generates with seed 7. That includes the floors files and the reports `run.sh`
writes. No page was fetched to produce any of it. A green run shows that the pipeline
still recovers what the generator planted and still writes the committed fixture bytes.
It says nothing about real requests, and nothing here claims an improvement.

## What is here

| file | what it holds |
|---|---|
| `run.sh` | the reproducibility eval, from `synth` to a byte comparison with `fixtures/golden` |
| `synth-seed7-floors.json` | floors for the 4000 pair corpus, which `run.sh` checks |
| `synth-quick-floors.json` | floors for the 1500 pair corpus, which `run.sh --quick` and `tests/test_eval.py` check |
| `synth-stable-floors.json` | floors for the `stable` scenario, the control that must pass with edits applied; `run.sh` and `tests/test_scenarios.py` check it |
| `synth-reversal-floors.json` | floors for the `reversal` scenario, whose gate must fail on the test window; the same two check it |

`spider-optimize-train eval CORPUS --run RUN --floors FILE` reads a floors file. For
each model kind, the file sets limits on the chronological test window. It names the
smallest `auroc` and `pr_auc` the run may show. It names the largest `ece_after`,
`brier_after`, `mae_credits` and `mae_millis`. It also names these keys:

- `expect_applied`, the exact number of edits the gated policy applies on the test
  pairs
- `expect_gate_status`, the status `gates` lands on
- `min_planted_uplift`, the smallest predicted uplift the model must show for each
  planted effect

Three more keys bound the policy against the heuristic baseline, as `compare.py` reads
it:

- `min_coverage`, the smallest share of test pairs with an override
- `max_harmful_rate`, the largest 95 percent upper bound on the share of overrides that
  lost a success or broke content. A policy with no override cannot meet it, since its
  rate is NaN.
- `min_harmful`, the smallest harmful count, for a fixture that expects harm

`eval` writes `RUN/eval-report.md` with one table per kind and the comparison table
under it. It prints one line per missed floor and exits 1 on any miss.

## What run.sh does

`training/evals/run.sh` works from any directory. It runs these steps in `training/`
with `uv`:

1. `synth` with seed 7 and 4000 pairs
2. `validate`
3. `train --seed 7`
4. `thresholds --seed 7` at the default sweep settings
5. `evaluate --quantize int8`
6. `gates`
7. `eval` with `synth-seed7-floors.json`
8. a second `thresholds` into a copy of the run, at the relaxed settings the golden
   fixtures were made with: `--min-covered 30 --r-max 0.05 --min-sites 20`
9. an `export` of both kinds

It compares each artifact's sha256 with the `sha256` in the committed sidecar. It
compares each golden file byte for byte with `fixtures/golden`. The last line is
`eval: ok` or `eval: FAILED <what>`.

`gates` is expected to exit 1 there. At the default sweep settings every floor abstains
on this corpus. The gated policy applies no edit, and its arm is the baseline arm on
every pair. The gate is `insufficient` with the reason "the policy applied no edit, so
there is nothing to compare". The script asserts that exit code and that reason instead
of treating them as a failure. On the 1500 pair corpus the test window holds 228 pairs,
under the 300 the gate needs. That makes it `insufficient` for that reason first.

`--quick` uses 1500 pairs and `synth-quick-floors.json`. It still exports both kinds
but does not compare them, because the committed fixtures come from the 4000 pair
corpus.

Both modes then run the two regression scenarios, `stable` and `reversal`, on 20000
pairs each. The steps are `synth --scenario`, `validate`, `train`, `thresholds` at the
default sweep settings, `gates`, `eval` with the scenario's floors file, and `tradeoff`.
`gates` is expected to exit 0 on `stable` and 1 on `reversal`, and the report must carry
the comparison with the baseline. Each scenario takes about 30 seconds on this machine.
That is under the minute that would have kept it out of `--quick`. The full mode with
both takes about 80 seconds.

Scratch output goes under `EVAL_SCRATCH`, default `training/data/eval-scratch`, which
git ignores.

### Export reproducibility

The export was byte reproducible before this eval existed. On 2026-09-16, at commit
`25fe815`, the sequence in `fixtures/golden/README.md` was rerun on this machine. The
machine is Apple silicon with the `uv.lock` versions of numpy and LightGBM. The rerun
wrote `synth-mlp.bin` with sha256
`7198d0615a76accda8b6e7d573cea9e3d94bfc368490dd338aadb2032d96a022` and `synth-gbdt.bin`
with `d20fe2163049e9f76117b881d6de3cf9f3342851026b707a5f4c990921bf1bcf`. Those were the
digests in the sidecars committed at the time, and both golden files matched byte for
byte. Nothing had to change to get there. The LightGBM heads already run with
`deterministic`, `num_threads` 1 and a seed per head. The MLP draws every weight and
every minibatch order from `np.random.default_rng(seed)`. The golden cases come from a
seeded generator.

A different LightGBM or numpy build can move the last bits of a weight. On such a
machine the digest step reports both digests and `eval: FAILED export drift`. Set
`EVAL_ALLOW_DRIFT=1` to get the same two digests as a warning with a zero exit. Do that
only when the Rust parity test still passes on the regenerated files. A drift that also
fails parity is a real bug in the exporter or the reader.

Since 2026-09-17, `train` chooses tau from the train, tune and calibrate rows only.
Before that it read the repeat pairs of the whole corpus, test window included. On the
seed 7 corpus the change moved tau from 0.8692 to 0.8610, and one train label moved with
it. The four golden files and the crate's copies were therefore regenerated together on
2026-09-17. The sequence in `fixtures/golden/README.md` now writes `synth-mlp.bin` with
sha256 `b51910d8501e6e307a622790526224e500c68e3330cc1a32fc658012b37d706f` and
`synth-gbdt.bin` with
`f1d59d4d3895170f22151bfde45666d0e234134e9f1972f274bea9b06dfe0b7a`. These are the
digests in the committed sidecars. `run.sh` in full mode compares against them with no
allowance.

## Planted effects

`eval` reads the manifest's `planted` block. Some effects can be located from the row
data alone. For each of those, `eval` takes the model's mean calibrated success for the
candidate arm minus the baseline arm, over the test rows the effect applies to. An
uplift effect must clear `min_planted_uplift`. A break effect must fall below its
negative, because a break lowers the success label the model is trained on. The report
also shows the observed label difference on the same rows, so a reader can compare the
plant with the prediction.

| effect | how its rows are found | gated |
|---|---|---|
| wait on a cold markup site | edit code `wait_for`, `ext` markup, `mem` cold. The generator gives a blocked or empty site a warm memory, so a cold row is an `ok` site, which is the plant's third condition | yes, uplift |
| browser mode on an empty site | edit code `request` and the status class the base features carry for a non-cold site | yes, uplift |
| residential proxy on a blocked site, before the flip | edit code `proxy`, status blocked, day before the flip | only when the window holds such rows |
| residential proxy on a blocked site, after the flip | the same rows on or after the flip | no, reported |
| blocked stylesheets break the page | edit code `block_stylesheets`, `ext` other | yes, drop |
| first party blacklist breaks the page | edit code `network_blacklist` with `third` false in the edit identifier | yes, drop |

On the default corpus the residential plant turns its uplift off for the last 15 of 60
days. The chronological test window is the last nine days, so no test row comes before
the flip. The "before the flip" row shows 0 rows and no verdict. Read the "after the
flip" row instead. The plant says the uplift is gone there, and the observed difference
is about zero. The model was trained on the days before the flip and still predicts the
uplift. That gap is reported and never gated, in any scenario. No floor in this file can
say what a model should predict about a change it never saw. The gate on the test window
is what rejects such a model. The `reversal` scenario below exists to prove that it does.

## The reversal fixture and its stable control

`synth --scenario reversal` and `synth --scenario stable` share one plant. The two
corpora are drawn with the same seed. The residential edit on a blocked site raises the
success chance from 0.25 to 0.90, at 1.5 times the credits and 1.1 times the time. The
wait edit on a cold markup site raises it from 0.55 to 0.75, at 1.1 times the credits
and 200 ms more. The arms of a pair share their success draw on all but one pair in two
hundred. The risk the sweep bounds therefore comes from the plant, not from luck.

The site mix has 800 sites, 35 percent of them blocked. Most tries of the two edits go
where their effect is. `--pairs 20000` gives the calibrate window about 320 wait rows
and 790 residential rows. Those counts let both floors clear `--min-covered 200` at
`--r-max 0.01`, with the top need/ext/mem cell of each edit seen on 50 sites in train.
The manifest's `planted` block records all of it, and the two tests read their expected
numbers from there.

In `reversal` the residential effect holds through the train, tune and calibrate days,
which are days 0 to 50. It flips on day 51, the first day of the chronological test
window. That day is `planted.residential.flip_day`, computed from the split rule in
`splits.window_counts`. From then on, residential takes a blocked site from 0.25 to 0.05,
at the same 1.5 times the credits. No row before day 51 shows any sign of it. A model
trained on that evidence applies the edit, as it should. The test window then shows the
edit losing what the plain fetch had, and `gates` returns `fail` with the success check
red. That is what the fixture proves. The evaluation rejects a regressing artifact
without asking the model to foresee a change it had no evidence of.
`min_planted_uplift` still gates the wait, browser and break effects here. The
residential rows past the flip are reported next to the plant and not gated.

In `stable` the same plant holds through every window and nothing flips. The run must
meet these conditions:

- `gates` returns `pass`
- coverage is at least 0.05, with at least 100 overrides
- the harmful rate's upper bound is at or under `r_max`
- the policy's credits per correct result are at or under the baseline's

The test asserts coverage before anything else, so a policy that abstains on every pair
fails there. A floors file with `min_coverage` fails `eval` the same way.
`test_abstaining_everything_does_not_pass_the_control` forces every floor to NaN and
checks that the miss names coverage. Without this control, a run could pass by switching
every learned action off.

Two departures from the plant's first description are deliberate. First, the
residential credit factor is 1.5, not the default corpus's 3.0. The gate's cost check
runs on the whole window's credits per correct result. An override at 3 times the
credits that lifts success from 0.25 to 0.90 buys each extra correct result at about 3.1
credits, against a baseline near 1.7. No policy that applies it could pass, and the
control would fail on cost instead of on abstaining. Second, the wait adds 200 ms, not
4000. At 4 seconds on a fifth of the pairs, the p90 check fails on its own. Neither
change affects what the reversal proves. The flip is the only difference between the two
scenarios.

Measured on 2026-09-17 on the test window, at the default sweep settings, seed 7. The
window has 3055 pairs on `stable` and 3009 on `reversal`, because the corpora differ past
day 51. FIXTURE-ONLY:

| scenario | kind | coverage | overrides | helpful | harmful | harmful rate (upper bound) | success delta | credits per correct delta | gate |
|---|---|---|---|---|---|---|---|---|---|
| stable | mlp | 0.3398 | 1038 | 501 | 2 | 0.0019 (0.0042) | +0.1640 | -0.5874 | pass |
| stable | gbdt | 0.3466 | 1059 | 504 | 2 | 0.0019 (0.0041) | +0.1650 | -0.5902 | pass |
| reversal | mlp | 0.3360 | 1011 | 56 | 158 | 0.1563 (0.1751) | -0.0336 | +0.6740 | fail |
| reversal | gbdt | 0.3387 | 1019 | 58 | 158 | 0.1551 (0.1737) | -0.0329 | +0.6681 | fail |

On `reversal` the success check fails with a point estimate of -0.033 and a lower bound
near -0.042. The cost check fails too. The 158 harmful overrides are about 22 percent of
the residential overrides. That share is the planted 0.25 to 0.05 drop, read through the
shared draw. On `stable` the two harmful overrides come from the one pair in two hundred
that draws its own luck.

## The tradeoff between overrides and regression risk

`spider-optimize-train tradeoff CORPUS --run RUN --r-max 0.005,0.01,0.02,0.05,0.1`
chooses the floors again at each `r_max` on the calibrate window. For each resulting
table, it reads the policy against the baseline on the test window. It writes the result
to `RUN/tradeoff.md`, one row per risk budget. The columns are:

- `floors`, how many edit codes got a finite floor
- `coverage`, the share of test pairs with an override
- `helpful` and `harmful`, the override counts as `compare.py` defines them
- the deltas, each policy minus baseline
- `gate`, what `gates` would return for that table

Coverage cannot fall as `r_max` rises, because a larger budget accepts a lower floor.
`test_tradeoff_coverage_is_monotone_in_r_max` checks that on both scenarios. It also
checks that the row at the default 0.01 matches the run `thresholds` wrote.

Both tables are fixture-only. Both are flat, and the flatness is the finding. On the
calibrate window the two planted edits regress on no covered row at all. In
`confidence-coverage.md`, the sweep's risk and its upper bound are 0.0000 for `proxy`
and `wait_for`. The smallest floor with 200 covered rows therefore already meets the
tightest budget. No other code reaches 200 covered rows at any budget. On `reversal`
the harm is the same at every `r_max`, because every floor is chosen on days before the
flip. A risk budget cannot see a reversal that starts after the calibrate window. Only
the gate on the test window can. Measured 2026-09-17, seed 7, 20000 pairs.

`stable`, mlp:

| r_max | floors | coverage | overrides | helpful | harmful | harmful rate | success delta | credits per correct delta | gate |
|---|---|---|---|---|---|---|---|---|---|
| 0.0050 | 2 | 0.3398 | 1038 | 501 | 2 | 0.0019 | 0.1640 | -0.5874 | pass |
| 0.0100 | 2 | 0.3398 | 1038 | 501 | 2 | 0.0019 | 0.1640 | -0.5874 | pass |
| 0.0200 | 2 | 0.3398 | 1038 | 501 | 2 | 0.0019 | 0.1640 | -0.5874 | pass |
| 0.0500 | 2 | 0.3398 | 1038 | 501 | 2 | 0.0019 | 0.1640 | -0.5874 | pass |
| 0.1000 | 2 | 0.3398 | 1038 | 501 | 2 | 0.0019 | 0.1640 | -0.5874 | pass |

`stable`, gbdt:

| r_max | floors | coverage | overrides | helpful | harmful | harmful rate | success delta | credits per correct delta | gate |
|---|---|---|---|---|---|---|---|---|---|
| 0.0050 | 2 | 0.3466 | 1059 | 504 | 2 | 0.0019 | 0.1650 | -0.5902 | pass |
| 0.0100 | 2 | 0.3466 | 1059 | 504 | 2 | 0.0019 | 0.1650 | -0.5902 | pass |
| 0.0200 | 2 | 0.3466 | 1059 | 504 | 2 | 0.0019 | 0.1650 | -0.5902 | pass |
| 0.0500 | 2 | 0.3466 | 1059 | 504 | 2 | 0.0019 | 0.1650 | -0.5902 | pass |
| 0.1000 | 2 | 0.3466 | 1059 | 504 | 2 | 0.0019 | 0.1650 | -0.5902 | pass |

`reversal`, mlp:

| r_max | floors | coverage | overrides | helpful | harmful | harmful rate | success delta | credits per correct delta | gate |
|---|---|---|---|---|---|---|---|---|---|
| 0.0050 | 2 | 0.3360 | 1011 | 56 | 158 | 0.1563 | -0.0336 | 0.6740 | fail |
| 0.0100 | 2 | 0.3360 | 1011 | 56 | 158 | 0.1563 | -0.0336 | 0.6740 | fail |
| 0.0200 | 2 | 0.3360 | 1011 | 56 | 158 | 0.1563 | -0.0336 | 0.6740 | fail |
| 0.0500 | 2 | 0.3360 | 1011 | 56 | 158 | 0.1563 | -0.0336 | 0.6740 | fail |
| 0.1000 | 2 | 0.3360 | 1011 | 56 | 158 | 0.1563 | -0.0336 | 0.6740 | fail |

`reversal`, gbdt:

| r_max | floors | coverage | overrides | helpful | harmful | harmful rate | success delta | credits per correct delta | gate |
|---|---|---|---|---|---|---|---|---|---|
| 0.0050 | 2 | 0.3387 | 1019 | 58 | 158 | 0.1551 | -0.0329 | 0.6681 | fail |
| 0.0100 | 2 | 0.3387 | 1019 | 58 | 158 | 0.1551 | -0.0329 | 0.6681 | fail |
| 0.0200 | 2 | 0.3387 | 1019 | 58 | 158 | 0.1551 | -0.0329 | 0.6681 | fail |
| 0.0500 | 2 | 0.3387 | 1019 | 58 | 158 | 0.1551 | -0.0329 | 0.6681 | fail |
| 0.1000 | 2 | 0.3387 | 1019 | 58 | 158 | 0.1551 | -0.0329 | 0.6681 | fail |

Two plants are not checked as uplift, because they do not touch success. The third
party blacklist with a share of three or more changes only bytes and credits. The
stylesheets and blacklist plants also change millis and credits. `eval` reads those
through `mae_millis` and `mae_credits` on the whole window, not per effect.

## Measured reference values

Measured on the chronological test window of the seed 7 corpus, at the default sweep
settings. FIXTURE-ONLY, like everything else here. The 4000 pair values are from
2026-09-17, after `train` started choosing tau outside the test window. Tau is 0.8610
here, and one train label moved. The floors below were set on 2026-09-16 at commit
`a003cf6`, from the values under the earlier rule, and they still hold. The 1500 pair
corpus has no repeat in its test window whose jaccard lies at the percentile, so its
values did not move.

4000 pairs, 1222 test rows, 611 test pairs, 0 applied, gate `insufficient`:

| metric | mlp | gbdt |
|---|---|---|
| auroc | 0.8646 | 0.8714 |
| pr_auc | 0.8608 | 0.8673 |
| ece_after | 0.0514 | 0.0557 |
| brier_after | 0.1475 | 0.1447 |
| mae_credits | 0.1842 | 0.2977 |
| mae_millis | 524.22 | 597.83 |
| wait on a cold markup site, predicted (61 rows, observed 0.3443) | 0.2938 | 0.3408 |
| browser mode on an empty site, predicted (48 rows, observed 0.7292) | 0.5943 | 0.6626 |
| residential after the flip, predicted (47 rows, observed -0.0213) | 0.4389 | 0.6256 |
| blocked stylesheets, predicted (60 rows, observed -0.7333) | -0.5344 | -0.6109 |
| first party blacklist, predicted (31 rows, observed -0.5161) | -0.3853 | -0.3835 |

1500 pairs (`--quick`), 456 test rows, 228 test pairs, 0 applied, gate `insufficient`:

| metric | mlp | gbdt |
|---|---|---|
| auroc | 0.8323 | 0.8366 |
| pr_auc | 0.8205 | 0.8148 |
| ece_after | 0.0580 | 0.0751 |
| brier_after | 0.1657 | 0.1644 |
| mae_credits | 0.2699 | 0.3421 |
| mae_millis | 676.70 | 689.38 |
| wait on a cold markup site, predicted (21 rows, observed 0.3810) | 0.3049 | 0.3734 |
| browser mode on an empty site, predicted (15 rows, observed 0.8000) | 0.5613 | 0.6409 |
| residential after the flip, predicted (19 rows, observed -0.0526) | 0.3898 | 0.5515 |
| blocked stylesheets, predicted (19 rows, observed -0.7895) | -0.4713 | -0.5643 |
| first party blacklist, predicted (19 rows, observed -0.5789) | -0.3302 | -0.1720 |

The floors sit 0.03 under the measured `auroc` and `pr_auc`. They sit 0.02 over the
measured `ece_after` and `brier_after`, and 20 percent over the measured `mae_credits`
and `mae_millis`. Each is rounded away from the measurement. `min_planted_uplift` is 0.2
on the full corpus and 0.1 on the quick one. On the quick corpus the smallest gated
magnitude is the GBDT's 0.172 on 19 blacklist rows. `min_planted_rows` is 20 on the full
corpus and 12 on the quick one, whose browser effect has 15 test rows.

## Refreshing the floors

The floors describe one generator and one seed. The measured values move, and the
floors must be set again, when any of these change: what `synth.py` plants, how many
rows it writes, how it draws them, the split, or the label rule. Do not loosen a floor to
make a red run green without reading the report first. This eval exists to catch a miss
on one metric while the others stay unchanged.

1. Run `training/evals/run.sh` and `training/evals/run.sh --quick`. The `evaluate` lines
   and `RUN/eval-report.md` under the scratch directory give the new measured values.
2. Set each floor from the measurement with the margins above.
3. Replace both tables in this file with the new values, the date and the commit.
4. If the export digests moved as well, follow the regeneration steps in
   `fixtures/golden/README.md`. Copy the four files to the crate, so
   `fixtures_are_the_trainer_exports_byte_for_byte` keeps passing.
5. Run `uv run pytest -q`. `tests/test_eval.py` checks the quick floors against a fresh
   quick run.
