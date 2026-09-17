# Regression report template

Use this template with `spider-optimize-train gates`. Replace the placeholders
with the results from the chronological test window, for each trained model
kind. This file contains no measured improvement.

If `manifest.synthetic` is true, put this banner immediately below the report
heading, before any result:

> **FIXTURE-ONLY.** Every number below comes from a synthetic corpus with planted effects and no fetched page. It shows whether the pipeline recovers what was planted. It says nothing about real requests and claims no improvement.

Every printed numeric result from that corpus must also carry `FIXTURE-ONLY`.
Keep the banner when quoting a synthetic result. The trainer's
`test_reports_carry_fixture_only_banner` checks the generated reports and output.

## Run and evidence

| Item | Fill from the run |
| --- | --- |
| Corpus | Manifest location, synthetic flag, row and pair counts, day range |
| Revisions | Collector revision, client version, service revision |
| Model | MLP or GBDT, artifact identifier, feature/schema versions |
| Labeling | Tau, baseline-repeat count, unjudged content count by need |
| Policy | Threshold table, support table, allowed needs and edit codes |
| Test window | Dates and pair count after the chronological split |
| Applied edits | Count and fraction of pairs on which the policy applied an edit |
| Regressions | Applied pairs that lost baseline success or broke content |
| Result | `pass`, `fail` or `insufficient`, with failing checks named |

The gate requires at least 300 test pairs. With fewer, the result is
`insufficient`, the gate fails, and no metric checks run. This minimum is separate
from the 100-pair minimum in dataset validation. A pilot of about 200 pairs cannot
qualify an artifact for rollout. For every test pair, compare the policy's
selected arm with its own baseline. When the policy abstains, its selected arm is
the baseline itself.

A policy that applied no edit on the window is also `insufficient` and fails.
Its arm is the baseline on every pair, so there is nothing to compare. The report
says so in place of the check table. A run whose floors all abstain lands here.
The synthetic corpus gives that result at the default sweep settings.

## The three gates

| Gate | Point estimate | Bound | Pass rule | Result |
| --- | --- | --- | --- | --- |
| Success and content | Fill success delta and content_ok delta | Fill both 95 percent lower bounds | Each lower bound at least -0.005 | Fill |
| Credits per correct result | Fill policy minus baseline | Fill the 95 percent upper bound of that difference | Upper bound at or below zero | Fill |
| Latency | Fill baseline and policy p50 and p90 millis | Point quantiles, not bootstrap bounds | p50 at most 1.10 times baseline; p90 at most 1.20 times baseline | Fill |

`gates.py` uses 2,000 bootstrap resamples of pairs and keeps each pair's arms
together. It takes the 2.5th percentile for the lower bounds and the 97.5th for
the credit upper bound. Success delta is policy success minus baseline success.

The credit check is paired. On each resample, the statistic is the policy's
credits per correct result minus the baseline's, over the same drawn pairs. The
check passes when the 97.5th percentile of that difference is at or below zero. A
policy that matches the baseline on every pair has a difference of exactly zero
on every resample, so it passes. Comparing the policy's own ratio with the
baseline point estimate would not be paired. That identical policy would then
fail about half the time from resampling noise alone.

The content gate uses the share not broken, minus one. Its per-pair content flag
is false only when an applied, successful arm has `content_ok == False`. It is
not a difference between two independently measured absolute content scores.

Policy credits per correct result is total policy credits divided by the
successful policy arms whose content flag is not broken. The baseline denominator
is the baseline success count. `gates.py` recomputes both ratios inside every
resample. Report missing content judgments next to these numbers, because the
implemented gate does not treat unknown content as broken. Report both model
kinds. The command fails unless every evaluated kind passes.

## The policy against the heuristic baseline

Below the three gates, the report has one more table. It comes from `compare.py`
and reads the same pairs the gates read. It shows what the selected policy did
that the heuristic would not have done, and what that cost.

| Row | What it holds |
| --- | --- |
| overrides | Pairs on which the policy applied the candidate, and their share of the window, called coverage. On every other pair the policy's arm is the baseline arm |
| success rate, content_ok rate | The policy's arms and the baseline arms, with the net success delta |
| credits per correct result | Total credits over correct arms for both, with the delta. A correct arm is a success whose content is not broken |
| harmful overrides | Applied pairs where the baseline succeeded and the policy's arm did not, or the arm came back with broken content. Shows their count, their rate over overrides and that rate's 95 percent bootstrap upper bound |
| helpful overrides | Applied pairs where the policy's arm is correct and the baseline failed |

A floors file may set bounds on this table with `min_coverage`, `max_harmful_rate`
and `min_harmful`. `max_harmful_rate` applies to the upper bound, so a policy with
no override cannot meet it. The `tradeoff` command writes the same columns once
per `r_max`. A reader can then see what each risk budget buys on the test window
before anyone chooses a budget.

## The two synthetic regression scenarios

`synth --scenario reversal` and `synth --scenario stable` are fixtures. The
generator makes both with the same seed and no fetched page. They prove two things
about the evaluation and nothing about real requests.

The reversal plants a residential effect on blocked sites. The effect holds on
every day the model is fitted, stopped and calibrated on. It flips on the first
day of the chronological test window. No earlier row shows any sign of it. The
trained model applies the edit, as the evidence says it should. The test window
then shows the edit losing what the plain fetch had. `gates` returns `fail` with
the success check red. The comparison table names the harmful overrides. `eval`
with `training/evals/synth-reversal-floors.json` expects exactly that. The model
does not have to foresee the flip. The evaluation has to reject the artifact.

The stable control is the same plant with no flip. `gates` must return `pass`
with edits applied. Coverage must be at least 0.05. The harmful rate's upper bound
must be at or under `r_max`. The policy's credits per correct result must be at or
under the baseline's. A policy that abstains on every pair fails this control on
coverage first. Switching every learned action off therefore cannot pass.

`training/evals/README.md` has the plant, the measured numbers and the tradeoff
tables for both scenarios.

## What code guarantees and what is empirical

Validation refuses unsupported edits and pinned fields. Application rechecks the
caller's snapshot. Pinning mode, pool or country skips the client optimizer.
Budget checks run before spending. `NoModel` keeps the request, NaN scores
abstain, and shadow sends the baseline unchanged. Those are code guarantees.
Training's gated policy also requires a finite per-edit floor and a supported
cell. The artifact exposes those tables. The client's generic scorer path does not
yet connect per-edit floors and cell selection, as
[architecture](architecture.md) explains. The report must not present a
training-policy check as a client guarantee.

Success, content preservation, credits and latency on future Spider requests are
empirical claims based on finite pairs. Intervals assume that the sampled pairs
represent deployment traffic. With zero failures in n independent applied pairs,
the failure-rate upper bound at 95 percent is near 3/n. It is never zero. If no
edit was applied, no edit failure rate was bounded. If failures occurred, use the
measured intervals, not the rule of three. Synthetic pairs test the pipeline's
planted effects. They cannot supply evidence about production pages.
