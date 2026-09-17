# Regression report template

Use this template with `spider-optimize-train gates`. Replace the placeholders
with the chronological test-window results for each trained model kind. This
file contains no measured improvement.

If `manifest.synthetic` is true, put this banner immediately below the report
heading, before any result:

> **FIXTURE-ONLY.** Every number below comes from a synthetic corpus with planted effects and no fetched page. It shows whether the pipeline recovers what was planted. It says nothing about real requests and claims no improvement.

Every printed numeric result from that corpus must also carry `FIXTURE-ONLY`.
Do not remove the banner when quoting a synthetic result. The trainer's
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

The gate requires at least 300 test pairs. Fewer is `insufficient`, fails, and
runs no metric checks. That is separate from dataset validation's 100-pair
minimum. A pilot of about 200 pairs cannot qualify an artifact for rollout.
For every test pair, compare the policy's selected arm with its own baseline;
when the policy abstains, its selected arm is the baseline itself.

A policy that applied no edit on the window is also `insufficient` and fails.
Its arm is the baseline on every pair, so there is nothing to compare, and the
report says so in place of the check table. A run whose floors all abstain, which
is what the synthetic corpus gives at the default sweep settings, lands here.

## The three gates

| Gate | Point estimate | Bound | Pass rule | Result |
| --- | --- | --- | --- | --- |
| Success and content | Fill success delta and content_ok delta | Fill both 95 percent lower bounds | Each lower bound at least -0.005 | Fill |
| Credits per correct result | Fill policy minus baseline | Fill the 95 percent upper bound of that difference | Upper bound at or below zero | Fill |
| Latency | Fill baseline and policy p50 and p90 millis | Point quantiles, not bootstrap bounds | p50 at most 1.10 times baseline; p90 at most 1.20 times baseline | Fill |

`gates.py` uses 2,000 bootstrap resamples of pairs, keeping the arms together.
It takes the 2.5th percentile for the lower bounds and the 97.5th for the credit
upper bound. Success delta is policy success minus baseline success.
The credit check is paired: on each resample the statistic is the policy's
credits per correct result minus the baseline's over the same drawn pairs, and
the check passes when the 97.5th percentile of that difference is at or below
zero. A policy that matches the baseline on every pair has a difference of
exactly zero on every resample and passes. Comparing the policy's own ratio
against the baseline point estimate is not paired: that identical policy would
fail it about half the time from resampling noise alone.
The content gate uses the share not broken minus one: its per-pair content flag
is false only when an applied, successful arm has `content_ok == False`.
It is not a difference between two independently measured absolute content scores.

Policy credits per correct result is total policy credits divided by successful
policy arms whose content flag is not broken. The baseline denominator is baseline
success count. Both ratios are recomputed inside every resample. Report missing content judgments alongside these numbers: the
implemented gate does not treat unknown content as broken. Report both model
kinds; the command fails unless every evaluated kind passes.

## What code guarantees and what is empirical

Validation refuses unsupported edits and pinned fields; application rechecks the
caller's snapshot. Pinning mode, pool or country skips the client optimizer.
Budget checks precede spending, `NoModel` keeps the request, NaN scores abstain,
and shadow sends the baseline unchanged. Those are code guarantees. Training's
gated policy also requires a finite per-edit floor and a supported cell. The
artifact exposes those tables, but the client's generic scorer path does not yet
connect per-edit floors and cell selection; see [architecture](architecture.md).
The report must not promote a training-policy check into a client guarantee.

Success, content preservation, credits and latency on future Spider requests are
empirical claims based on finite pairs. Intervals assume the sampled pairs
represent deployment traffic. With zero failures in n independent applied pairs,
the failure-rate upper bound is near 3/n at 95 percent, never zero. If no edit was
applied, no edit failure rate was bounded; if failures occurred, use the measured
intervals rather than the rule of three. Synthetic pairs test the pipeline's
planted effects and cannot supply evidence about production pages.
