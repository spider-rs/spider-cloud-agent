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

A floors file is what `spider-optimize-train eval CORPUS --run RUN --floors FILE` reads.
Per model kind it names the smallest `auroc` and `pr_auc` and the largest `ece_after`,
`brier_after`, `mae_credits` and `mae_millis` the run may show on the chronological
test window. It also names `expect_applied`, the exact number of edits the gated policy
applies on the test pairs, `expect_gate_status`, the status `gates` lands on, and
`min_planted_uplift`, the smallest predicted uplift the model must show for each planted
effect. `eval` writes `RUN/eval-report.md` with one table per kind, prints one line per
missed floor and exits 1 on any miss.

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

The residential plant flips its uplift off for the last 15 of 60 days, and the
chronological test window is the last nine days, so no test row is before the flip. The
"before the flip" row shows 0 rows and no verdict. The "after the flip" row is the one
to read: the plant says the uplift is gone there, the observed difference is about zero,
and the model, trained on the days before the flip, still predicts it. That gap is
reported and not gated, because no floor in this file can say what a model should
predict about a change it never saw.

Two plants are not checked as uplift because they do not touch success. The third party
blacklist with a share of three or more changes bytes and credits only, and the
stylesheets and blacklist plants also change millis and credits, which `eval` reads
through `mae_millis` and `mae_credits` on the whole window rather than per effect.

## Measured reference values

Measured on 2026-09-16 at commit `a003cf6`, the commit that added `eval`, on the
chronological test window of the seed 7 corpus at the default sweep
settings. FIXTURE-ONLY, like everything else here.

4000 pairs, 1222 test rows, 611 test pairs, 0 applied, gate `insufficient`:

| metric | mlp | gbdt |
|---|---|---|
| auroc | 0.8637 | 0.8718 |
| pr_auc | 0.8592 | 0.8673 |
| ece_after | 0.0580 | 0.0558 |
| brier_after | 0.1479 | 0.1447 |
| mae_credits | 0.1842 | 0.2977 |
| mae_millis | 524.22 | 597.83 |
| wait on a cold markup site, predicted (61 rows, observed 0.3443) | 0.2980 | 0.3353 |
| browser mode on an empty site, predicted (48 rows, observed 0.7292) | 0.6031 | 0.6601 |
| residential after the flip, predicted (47 rows, observed -0.0213) | 0.4432 | 0.6255 |
| blocked stylesheets, predicted (60 rows, observed -0.7333) | -0.5303 | -0.6176 |
| first party blacklist, predicted (31 rows, observed -0.5161) | -0.3838 | -0.3833 |

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
