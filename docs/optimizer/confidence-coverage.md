# Confidence and coverage template

Generate this report with `spider-optimize-train thresholds` on the calibration
window, separately for MLP and GBDT. If the corpus is synthetic, place the
[FIXTURE-ONLY banner](regression-report.md) below the heading before reporting
numbers. The templates here contain no measured risk or coverage.

## Curve for each edit code

Record corpus and run identifiers, calibration method, tau, seed, `r_max`,
`min_covered` and support minimum. Plot coverage on the horizontal axis and both
observed regression risk and its bootstrap upper bound on the vertical axis.
Mark the chosen threshold and the `r_max` line. Keep the underlying table so
an empty or unsupported edit cannot disappear from a plot.

| Edit code | Key | Candidate rows | Threshold | Covered rows | Coverage | Risk | 95 percent risk upper bound | Decision |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `request` | Fill | Fill | Fill | Fill | Fill | Fill | Apply eligible or abstain |
| 2 | `proxy` | Fill | Fill | Fill | Fill | Fill | Fill | Apply eligible or abstain |
| 3 | `wait_for` | Fill | Fill | Fill | Fill | Fill | Fill | Apply eligible or abstain |
| 4 | `disable_intercept` | Fill | Fill | Fill | Fill | Fill | Fill | Apply eligible or abstain |
| 5 | `full_resources` | Fill | Fill | Fill | Fill | Fill | Fill | Apply eligible or abstain |
| 6 | `block_ads` | Fill | Fill | Fill | Fill | Fill | Fill | Apply eligible or abstain |
| 7 | `block_analytics` | Fill | Fill | Fill | Fill | Fill | Fill | Apply eligible or abstain |
| 8 | `block_stylesheets` | Fill | Fill | Fill | Fill | Fill | Fill | Shadow only in rollout |
| 9 | `network_blacklist` | Fill | Fill | Fill | Fill | Fill | Fill | Shadow only in rollout |
| Pooled | All learnable codes | Fill | Fill | Fill | Fill | Fill | Fill | Diagnostic only |

Repeat the following table for every threshold point of every edit code, plus
the pooled curve. Code 0 is keep and is the comparison baseline, not an edit to
approve from the pooled curve.

| Success floor t | Candidate rows | Covered rows | Coverage | Regressed covered rows | Risk | Bootstrap upper bound |
| --- | --- | --- | --- | --- | --- | --- |
| Fill from sweep | Fill | Fill | Fill | Fill | Fill | Fill |

## Choosing a threshold

The trainer sweeps t from 0.5 through 1.0 in steps of 0.005. A candidate is
covered when calibrated success is at least t and predicted credits per correct
result are strictly below its own baseline arm's prediction. Coverage divides
covered rows by candidate rows for that code. Risk is the fraction of covered
rows whose pair regressed: lost baseline success or successful but broken content.

The risk bound is the 95th percentile over 1,000 bootstrap resamples by pair.
A resample with no covered rows contributes risk 1. Choose the largest coverage
whose upper bound is at most `r_max` (default 0.01) with at least 200 covered rows.
Because coverage decreases as t rises, the implementation chooses the first,
smallest qualifying t. If none qualifies, abstain for that edit code. The JSON
table stores null, restored to NaN for the artifact. A pooled result cannot rescue
an edit code with no qualifying threshold.

## Abstention

Training's policy abstains when the edit code has no finite floor, calibrated
success does not reach it, the candidate is not cheaper per correct result, or
its cell has inadequate support. Cells pack
`need << 24 | ext << 16 | mem << 8 | edit_code`; support requires at least 50
distinct `dk` values in the training window by default. Record unsupported cells
and the number of distinct sites, not just the number of rows.

The runtime gate additionally keeps requests with mode, proxy or country pinned,
no scorer version, a NaN baseline or candidate score, insufficient support or gain,
or no admissible candidate. Validation excludes caller-set fields, incompatible
settings, over-budget edits, heavier edits under rate limits and blacklist edits
without `disable_hints`. All content-changing edits remain shadow-only under the
rollout policy, regardless of a qualifying curve.

The artifact reader returns no threshold for absent or NaN entries and abstains
on invalid input dimensions or non-finite inputs. A nonempty support table needs
`score_in_cell`; generic `Scorer::score` supplies support zero. An empty artifact
support table means unrestricted support to the Rust reader, whereas training's
policy supports no cells when its table is empty. The current client also does
not read per-edit artifact thresholds. Resolve and verify those integration
differences before treating a training threshold report as a deployment gate.
