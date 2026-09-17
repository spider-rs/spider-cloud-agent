# Confidence and coverage template

Generate this report with `spider-optimize-train thresholds` on the calibration
window. Run it once for MLP and once for GBDT. If the corpus is synthetic, put the
[FIXTURE-ONLY banner](regression-report.md) below the heading before any numbers.
The templates here contain no measured risk or coverage.

## Curve for each edit code

Record the corpus and run identifiers, calibration method, tau, seed, `r_max`,
`min_covered` and support minimum. Plot coverage on the horizontal axis. Plot
observed regression risk and its bootstrap upper bound on the vertical axis. Mark
the chosen threshold and the `r_max` line. Keep the underlying table, so an empty
or unsupported edit cannot vanish from a plot.

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

Repeat the next table for every threshold point of every edit code, and for the
pooled curve. Code 0 is keep. It is the comparison baseline, not an edit to
approve from the pooled curve.

| Success floor t | Candidate rows | Covered rows | Coverage | Regressed covered rows | Risk | Bootstrap upper bound |
| --- | --- | --- | --- | --- | --- | --- |
| Fill from sweep | Fill | Fill | Fill | Fill | Fill | Fill |

## Choosing a threshold

The trainer sweeps t from 0.5 through 1.0 in steps of 0.005. A candidate is
covered when two things hold. Its calibrated success is at least t. Its predicted
credits per correct result are strictly below the prediction for its own baseline
arm. Coverage is covered rows divided by candidate rows for that code. Risk is the
fraction of covered rows whose pair regressed. A pair regresses when it loses
baseline success, or when it succeeds with broken content.

The risk bound is the 95th percentile over 1,000 bootstrap resamples by pair.
A resample with no covered rows counts as risk 1. Choose the largest coverage
whose upper bound is at most `r_max` and that has at least 200 covered rows. The
default `r_max` is 0.01. Coverage falls as t rises, so the implementation picks the first,
smallest qualifying t. If no t qualifies, abstain for that edit code. The JSON
table then stores null, and the artifact gets NaN back. A pooled result cannot
rescue an edit code with no qualifying threshold.

## Abstention

Training's policy abstains in four cases:

- the edit code has no finite floor
- calibrated success does not reach the floor
- the candidate is not cheaper per correct result
- the candidate's cell has too little support

Cells pack `need << 24 | ext << 16 | mem << 8 | edit_code`. By default, support
needs at least 50 distinct `dk` values in the training window. Record unsupported
cells and the number of distinct sites, not just the number of rows.

The runtime gate also keeps the request unchanged in these cases: mode, proxy or
country is pinned; there is no scorer version; the baseline or candidate score is
NaN; support or gain is too low; or no candidate is admissible. Validation
excludes caller-set fields, incompatible settings, over-budget edits, heavier
edits under rate limits, and blacklist edits without `disable_hints`. The rollout
policy keeps all content-changing edits shadow-only, even with a qualifying curve.

The artifact reader returns no threshold for absent or NaN entries. It abstains
on invalid input dimensions or non-finite inputs. A nonempty support table needs
`score_in_cell`, because the generic `Scorer::score` supplies support zero. The
two sides read an empty support table in opposite ways. The Rust reader treats it
as unrestricted support. Training's policy treats it as no supported cells. The
current client also does not read per-edit artifact thresholds. Resolve and
verify those integration differences before a training threshold report counts
as a deployment gate.
