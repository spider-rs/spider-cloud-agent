# Evals

**FIXTURE-ONLY.** Every number in this directory, in the floors files and in the
reports `run.sh` writes comes from the synthetic corpus `synth.py` generates with seed 7.
No page was fetched to produce any of it. A green run says the pipeline still recovers
what the generator planted and still writes the committed fixture bytes. It says nothing
about real requests and nothing here claims an improvement.

## What is here

| file | what it holds |
|---|---|
| `run.sh` | the reproducibility eval, from `synth` to a byte comparison with `fixtures/golden` |
| `synth-seed7-floors.json` | floors for the 4000 pair corpus, which `run.sh` checks |
| `synth-quick-floors.json` | floors for the 1500 pair corpus, which `run.sh --quick` and `tests/test_eval.py` check |
| `synth-stable-floors.json` | floors for the `stable` scenario, the control that must pass with edits applied; `run.sh` and `tests/test_scenarios.py` check it |
| `synth-reversal-floors.json` | floors for the `reversal` scenario, whose gate must fail on the test window; the same two check it |

A floors file is what `spider-optimize-train eval CORPUS --run RUN --floors FILE` reads.
Per model kind it names the smallest `auroc` and `pr_auc` and the largest `ece_after`,
`brier_after`, `mae_credits` and `mae_millis` the run may show on the chronological
test window. It also names `expect_applied`, the exact number of edits the gated policy
applies on the test pairs, `expect_gate_status`, the status `gates` lands on, and
`min_planted_uplift`, the smallest predicted uplift the model must show for each planted
effect. Three more keys bound the policy against the heuristic baseline as `compare.py`
reads it: `min_coverage`, the smallest share of test pairs with an override;
`max_harmful_rate`, the largest 95 percent upper bound on the share of overrides that
lost a success or broke content, which a policy with no override cannot meet since its
rate is NaN; and `min_harmful`, the smallest harmful count, for a fixture that expects
harm. `eval` writes `RUN/eval-report.md` with one table per kind and the comparison
table under it, prints one line per missed floor and exits 1 on any miss.

## What run.sh does

From any directory, `training/evals/run.sh` runs in `training/` with `uv`: `synth`
(seed 7, 4000 pairs), `validate`, `train --seed 7`, `thresholds --seed 7` at the default
sweep settings, `evaluate --quantize int8`, `gates`, `eval` with
`synth-seed7-floors.json`, then a second `thresholds` at the relaxed settings the golden
fixtures were made with (`--min-covered 30 --r-max 0.05 --min-sites 20`) into a copy of
the run, and an `export` of both kinds. It compares each artifact's sha256 with the
`sha256` in the committed sidecar and each golden file byte for byte with
`fixtures/golden`. The last line is `eval: ok` or `eval: FAILED <what>`.

`gates` is expected to exit 1 there. At the default sweep settings every floor abstains
on this corpus, so the gated policy applies no edit, its arm is the baseline arm on every
pair, and the gate is `insufficient` with the reason "the policy applied no edit, so
there is nothing to compare". The script asserts that exit code and that reason rather
than treating them as a failure. On the 1500 pair corpus the test window holds 228
pairs, under the 300 the gate needs, so it is `insufficient` for that reason first.

`--quick` uses 1500 pairs and `synth-quick-floors.json`. It still exports both kinds but
does not compare them, since the committed fixtures are the 4000 pair corpus.

Both modes then run the two regression scenarios, `stable` and `reversal`, on 20000
pairs each: `synth --scenario`, `validate`, `train`, `thresholds` at the default sweep
settings, `gates` (exit 0 expected on `stable`, exit 1 on `reversal`, and the report must
carry the comparison with the baseline), `eval` with the scenario's floors file, and
`tradeoff`. Each scenario takes about 30 seconds on this machine, under the minute that
would have kept it out of `--quick`; the full mode with both takes about 80 seconds.

Scratch output goes under `EVAL_SCRATCH`, default `training/data/eval-scratch`, which
git ignores.

### Export reproducibility

The export was byte reproducible before this eval existed. On 2026-09-16, at commit
`25fe815`, rerunning the sequence in `fixtures/golden/README.md` on this machine (Apple
silicon, the `uv.lock` versions of numpy and LightGBM) wrote `synth-mlp.bin` with sha256
`7198d0615a76accda8b6e7d573cea9e3d94bfc368490dd338aadb2032d96a022` and `synth-gbdt.bin`
with `d20fe2163049e9f76117b881d6de3cf9f3342851026b707a5f4c990921bf1bcf`, the digests in
the committed sidecars, and both golden files matched byte for byte. The LightGBM heads
already run with `deterministic`, `num_threads` 1 and a seed per head, the MLP draws
every weight and every minibatch order from `np.random.default_rng(seed)`, and the
golden cases from a seeded generator, so nothing was changed to get there.

A different LightGBM or numpy build can move the last bits of a weight. On such a
machine the digest step reports both digests and `eval: FAILED export drift`. Set
`EVAL_ALLOW_DRIFT=1` to get the same two digests as a warning and a zero exit. Do that
only when the Rust parity test still passes on the regenerated files; a drift that also
fails parity is a real bug in the exporter or the reader.

Since 2026-09-17 `train` chooses tau from the train, tune and calibrate rows only, where
it used to read the repeat pairs of the whole corpus, test window included. On the seed
7 corpus that moves tau from 0.8692 to 0.8610 and one train label with it, so the
sequence above no longer writes the committed bytes on any machine, and `run.sh` without
`--quick` reports drift until the four golden files and the crate's copies are
regenerated together as `fixtures/golden/README.md` describes. The committed files are
still the format fixtures the parity tests read. Until the regeneration lands, run the
full mode with `EVAL_ALLOW_DRIFT=1`.

## Planted effects

`eval` reads the manifest's `planted` block and, for each effect it can locate from the
row data alone, takes the model's mean calibrated success for the candidate arm minus the
baseline arm over the test rows the effect applies to. An uplift effect must clear
`min_planted_uplift`; a break effect must fall below its negative, since a break lowers
the success label the model is trained on. The report also shows the observed label
difference on the same rows, so a reader can see the plant beside the prediction.

| effect | how its rows are found | gated |
|---|---|---|
| wait on a cold markup site | edit code `wait_for`, `ext` markup, `mem` cold. The generator gives a blocked or empty site a warm memory, so a cold row is an `ok` site, which is the plant's third condition | yes, uplift |
| browser mode on an empty site | edit code `request` and the status class the base features carry for a non-cold site | yes, uplift |
| residential proxy on a blocked site, before the flip | edit code `proxy`, status blocked, day before the flip | only when the window holds such rows |
| residential proxy on a blocked site, after the flip | the same rows on or after the flip | no, reported |
| blocked stylesheets break the page | edit code `block_stylesheets`, `ext` other | yes, drop |
| first party blacklist breaks the page | edit code `network_blacklist` with `third` false in the edit identifier | yes, drop |

On the default corpus the residential plant flips its uplift off for the last 15 of 60
days, and the chronological test window is the last nine days, so no test row is before
the flip. The "before the flip" row shows 0 rows and no verdict. The "after the flip" row
is the one to read: the plant says the uplift is gone there, the observed difference is
about zero, and the model, trained on the days before the flip, still predicts it. That
gap is reported and never gated, whatever the scenario, because no floor in this file
can say what a model should predict about a change it never saw. What rejects such a
model is the gate on the test window, and the `reversal` scenario below exists to prove
that it does.

## The reversal fixture and its stable control

`synth --scenario reversal` and `synth --scenario stable` share one plant, and the two
corpora are drawn with the same seed. The residential edit on a blocked site takes the
success chance from 0.25 to 0.90 at 1.5 times the credits and 1.1 times the time; the
wait edit on a cold markup site takes it from 0.55 to 0.75 at 1.1 times the credits and
200 ms more. Arms of one pair share their success draw on all but one pair in two
hundred, so the risk the sweep bounds is the plant and not luck. The site mix has 800
sites with 35 percent blocked, most tries of the two edits go where their effect is, and
`--pairs 20000` gives the calibrate window about 320 wait rows and 790 residential rows,
which is what lets both floors clear `--min-covered 200` at `--r-max 0.01` with the top
need/ext/mem cell of each edit seen on 50 sites in train. The manifest's `planted` block
records all of it, and the two tests read their expected numbers from there.

In `reversal` the residential effect holds through the train, tune and calibrate days
(0 to 50) and flips on day 51, the first day of the chronological test window
(`planted.residential.flip_day`, computed from the split rule in `splits.window_counts`):
from then on residential takes a blocked site from 0.25 to 0.05, at the same 1.5 times
the credits. No row before day 51 carries any sign of it. A model trained on that
evidence applies the edit, as it should; the test window then shows the edit losing what
the plain fetch had, and `gates` returns `fail` with the success check red. That is
what the fixture proves: the evaluation rejects a regressing artifact without asking the
model to have foreseen a change it had no evidence of. `min_planted_uplift` still gates
the wait, browser and break effects here; the residential rows past the flip are reported
beside the plant and not gated.

In `stable` the same plant holds through every window and nothing flips. `gates` must
return `pass`, coverage must be at least 0.05 with at least 100 overrides, the harmful
rate's upper bound at or under `r_max`, and the policy's credits per correct result at or
under the baseline's. The test asserts coverage before anything else, so a policy that
abstains on every pair fails it there; a floors file with `min_coverage` fails `eval` the
same way (`test_abstaining_everything_does_not_pass_the_control` forces every floor to
NaN and checks that the miss names coverage). Without this control, a run could pass by
switching every learned action off.

Two departures from the plant's first description are deliberate. The residential
credit factor is 1.5 rather than the default corpus's 3.0, because the gate's cost check
is on the whole window's credits per correct result: an override at 3 times the credits
that lifts success from 0.25 to 0.90 buys each extra correct result at about 3.1 credits
against a baseline near 1.7, so no policy that applies it can pass, and the control
would fail for cost rather than for abstaining. And the wait's added time is 200 ms
rather than 4000, because at 4 seconds on a fifth of the pairs the p90 check fails on its
own. Neither changes what the reversal proves; the flip is the only difference between
the two scenarios.

Measured on 2026-09-17 on the test window (3055 pairs on `stable`, 3009 on `reversal`;
the corpora differ past day 51), default sweep settings, seed 7. FIXTURE-ONLY:

| scenario | kind | coverage | overrides | helpful | harmful | harmful rate (upper bound) | success delta | credits per correct delta | gate |
|---|---|---|---|---|---|---|---|---|---|
| stable | mlp | 0.3398 | 1038 | 501 | 2 | 0.0019 (0.0042) | +0.1640 | -0.5874 | pass |
| stable | gbdt | 0.3466 | 1059 | 504 | 2 | 0.0019 (0.0041) | +0.1650 | -0.5902 | pass |
| reversal | mlp | 0.3360 | 1011 | 56 | 158 | 0.1563 (0.1751) | -0.0336 | +0.6740 | fail |
| reversal | gbdt | 0.3387 | 1019 | 58 | 158 | 0.1551 (0.1737) | -0.0329 | +0.6681 | fail |

On `reversal` the success check fails with a point estimate of -0.033 and a lower bound
near -0.042, and the cost check fails too. The 158 harmful overrides are about 22 percent
of the residential overrides, which is the planted 0.25 to 0.05 drop read through the
shared draw. On `stable` the two harmful overrides are the one pair in two hundred that
draws its own luck.

## The tradeoff between overrides and regression risk

`spider-optimize-train tradeoff CORPUS --run RUN --r-max 0.005,0.01,0.02,0.05,0.1`
chooses the floors again at each `r_max` on the calibrate window and reads the policy
each table gives against the baseline on the test window, into `RUN/tradeoff.md`. Each
row is one risk budget: `floors` is how many edit codes got a finite floor, `coverage`
the share of test pairs with an override, `helpful` and `harmful` the override counts
as `compare.py` defines them, the deltas policy minus baseline, and `gate` what `gates`
would return for that table. Coverage cannot fall as `r_max` rises, since a larger
budget accepts a lower floor, and `test_tradeoff_coverage_is_monotone_in_r_max` checks
that on both scenarios and checks that the row at the default 0.01 is the run
`thresholds` wrote.

Both tables are fixture-only and both are flat, and the flatness is the finding. On the
calibrate window the two planted edits regress on no covered row at all (the sweep's
risk and its upper bound are 0.0000 for `proxy` and `wait_for` in
`confidence-coverage.md`), so the smallest floor with 200 covered rows already meets the
tightest budget, and no other code reaches 200 covered rows at any budget. On
`reversal` the harm is the same at every `r_max` because every floor is chosen on days
before the flip: a risk budget cannot see a reversal that starts after the calibrate
window, only the gate on the test window can. Measured 2026-09-17, seed 7, 20000 pairs.

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

Two plants are not checked as uplift because they do not touch success. The third party
blacklist with a share of three or more changes bytes and credits only, and the
stylesheets and blacklist plants also change millis and credits, which `eval` reads
through `mae_millis` and `mae_credits` on the whole window rather than per effect.

## Measured reference values

Measured on the chronological test window of the seed 7 corpus at the default sweep
settings. FIXTURE-ONLY, like everything else here. The 4000 pair values are from
2026-09-17, after `train` started choosing tau outside the test window (tau 0.8610
here, one train label moved); the floors below were set on 2026-09-16 at commit
`a003cf6` from the values under the earlier rule and still hold. The 1500 pair corpus
has no repeat in its test window whose jaccard lies at the percentile, so its values
did not move.

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

The floors sit 0.03 under the measured `auroc` and `pr_auc`, 0.02 over the measured
`ece_after` and `brier_after`, and 20 percent over the measured `mae_credits` and
`mae_millis`, rounded away from the measurement. `min_planted_uplift` is 0.2 on the full
corpus and 0.1 on the quick one, where the smallest gated magnitude is the GBDT's 0.172
on 19 blacklist rows. `min_planted_rows` is 20 on the full corpus and 12 on the quick
one, whose browser effect has 15 test rows.

## Refreshing the floors

The floors describe one generator and one seed. When `synth.py` changes what it plants,
how many rows it writes, or how it draws them, or when the split or the label rule
changes, the measured values move and the floors must be set again. Do not loosen a
floor to make a red run green without reading the report first; a miss on one metric
with the others unchanged is what this eval exists to catch.

1. Run `training/evals/run.sh` and `training/evals/run.sh --quick`. The `evaluate` lines
   and `RUN/eval-report.md` under the scratch directory give the new measured values.
2. Set each floor from the measurement with the margins above.
3. Replace both tables in this file with the new values, the date and the commit.
4. If the export digests moved as well, follow the regeneration steps in
   `fixtures/golden/README.md` and copy the four files to the crate, so
   `fixtures_are_the_trainer_exports_byte_for_byte` keeps passing.
5. `uv run pytest -q`; `tests/test_eval.py` checks the quick floors against a fresh
   quick run.
