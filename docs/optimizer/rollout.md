# Optimizer rollout

The client optimizer is off by default behind the `optimize` feature and runs only
when supplied to `SpiderBuilder`. The sequence below is the rollout plan, not a
claim that paid collection or a production canary has happened. This base has no
`optimize-collect` crate or README; the collector is a later task.

## Offline on synthetic data

Run the training pipeline against its planted synthetic effects first. Compare
MLP and LightGBM on the same splits, calibration, content labels and paired gates.
Keep the FIXTURE-ONLY banner on every synthetic report and the label on every
printed numeric result. Synthetic parity and passing gates establish that the
pipeline works on those fixtures; they establish no real savings or content safety.

These are the commands from [training/README.md](../../training/README.md), run
from the repository root and then inside `training` as shown. They send no paid
requests. Setup may need dependency downloads; collection is a separate activity.

```bash
cd training
uv sync
uv run ruff check .
uv run pytest -q

uv run spider-optimize-train synth --out /tmp/opt/corpus --seed 1
uv run spider-optimize-train validate /tmp/opt/corpus
uv run spider-optimize-train train /tmp/opt/corpus --out /tmp/opt/run --seed 1
uv run spider-optimize-train thresholds /tmp/opt/corpus --run /tmp/opt/run
uv run spider-optimize-train evaluate /tmp/opt/corpus --run /tmp/opt/run --quantize int8
uv run spider-optimize-train gates /tmp/opt/corpus --run /tmp/opt/run
uv run spider-optimize-train export /tmp/opt/corpus --run /tmp/opt/run --kind mlp \
  --out /tmp/opt/out/synth-mlp.bin --golden /tmp/opt/out/golden-mlp.json --quantize int8
```

Run the two regression scenarios as well, `training/evals/run.sh` does. Both are
synthetic. `synth --scenario reversal` plants a residential effect that holds through
the days the model learns from and turns harmful inside the test window; the gate
must reject that artifact without the model having had any evidence of the flip,
and the regression report's comparison table must name the harmful overrides.
`synth --scenario stable` is the same plant with no flip, and the gate must pass it
with edits applied, at coverage of at least 0.05, so a run cannot pass by abstaining
on every pair. `spider-optimize-train tradeoff` then shows, per risk budget, what the
floors buy on the test window. Passing both says the evaluation catches a reversal
and does not reward a policy that does nothing; it says nothing about real requests.

The default threshold settings can legitimately abstain on every edit. The
fixture recipe in the training README relaxes coverage, risk and site minimums
to exercise the reader; those settings are not production approval criteria.
The int8 evaluation is Python-only; the Rust artifact is FP32. Domain splits
are diagnostic, and `gates` requires the chronological split.

## Pilot and spend cap

After a collector exists, start with about 200 paired trials. This spends credits
and needs explicit authorisation and a hard total spend cap before execution.
Pin collector, client and service revisions, record the real spend in the
manifest, and stop at the cap including retries and baseline repeats. The pilot
tests collection and labeling; it cannot satisfy the regression gate's minimum
of 300 chronological test pairs or guarantee 200 covered rows for each edit.

For planning only, price every arm with the client's multiplier table relative
to an HTTP base price. These are spend assumptions, not measured service prices.

| Arm | Assumed multiplier |
| --- | --- |
| HTTP | 1.0 |
| Smart | 1.5 |
| Browser | 4.0 |
| Browser with wait | 5.0 |
| Smart residential | 3.0 |
| Browser residential with wait | 8.0 |

With the planned arm mix, assume about five base units per pair: the 200-pair
pilot is about 1,000 base units, 5,000 pairs about 25,000, and 20,000 pairs about
100,000. Add ten percent for repeats and retries to each planning amount. Real
prices come from `Costs.total_cost` on each response, not the multiplier table;
the manifest's `credits_spent` records actual spend. Do not report these planning
amounts as observed costs or savings.

Service-source findings verified on 2026-09-16 matter to the arm design.
`disable_intercept` lets first party scripts through while leaving request
interception and the network lists active. The service merges caller
`network_blacklist` and `network_whitelist` entries with stored hints and existing
site configuration using capped pattern lists, rather than replacing them.
With hints enabled, the blacklist comparison is "caller entries plus hints"
against "hints alone". `disable_hints` removes more adjustments than blacklist
hints, so use the same setting on both arms and record the collection conditions.
The optimizer itself requires hints disabled for blacklist candidates. It never
sets `event_tracker`; the collector must explicitly request its `request_map`
and `response_map` and account for that observation's cost.

## Shadow before applying

Use `Optimizer::shadow` and a `ComparisonRecorder`, with a collection-specific
salt and a day `Clock`. Shadow scores candidates and records the selected edit's
numeric descriptor, its features and planned multiplier alongside the actual
baseline walk's final success, status, wire bytes, attempt count, cost and elapsed
attempt time. The client logs the candidate count, whether it applied, and a keep
reason; the row does not contain predicted scores or a complete candidate ranking.

The baseline request goes out unchanged. Shadow cannot observe the unexecuted
alternative's content, latency or bill. It cannot produce paired causal evidence
by attaching baseline outcomes to a proposed edit. The client writes one row only
when the policy accepts or stops, with `pair: 0` and null pair-relative labels.
Walks cut short by wall, budget or connection failure have no comparison row.
A later collector must execute and join both arms to fill the evidence gaps.

## Bounded canary

Before applying edits, connect artifact per-edit floors and support cells to the
client scorer and verify equivalence with the training policy. The current generic
path does not enforce those floors. Build the canary controls in the caller or a
later integration: the present `Optimizer` has no hash-share selector, per-need
allowlist or rolling regression monitor.

Select a bounded share by a stable hash, with the share, seed, need allowlist,
edit-code allowlist, spend cap and stop conditions recorded before starting.
Only needs with judged content and edit codes that passed their own evidence
gates qualify. Keep `block_stylesheets` and `network_blacklist` shadow-only during
this rollout. Pinning any caller field still wins; pinning mode, pool or country
skips optimization entirely.

Maintain a rolling paired estimate over the last 500 rows of matched evidence,
keeping complete pairs together. If the success-delta lower bound breaks the
-0.005 success gate, automatically flip new traffic to shadow. Do not infer that
bound from unpaired shadow rows. Insufficient matched evidence also keeps traffic
in shadow. This rolling monitor is separate from the offline gate requiring 300
test pairs: 500 rows of two-arm trials do not meet that offline minimum.

The kill switch is to drop the optimizer from the builder and construct the
client without `.optimizer(...)`. No model result can affect new requests through
that client. Removing the `optimize` feature also removes the integration at build
time, but rebuilding should not be required to stop a canary.

## Rollback and follow-up work

On a broken gate or missing evidence, return to shadow or replace the client with
one built without the optimizer. Preserve the artifact identifier, manifest,
threshold report and paired regression report for the failed window. The change
affects future operations, not requests already sent or credits already spent.
Do not widen the share until the failing need/edit cells have new paired evidence
and the chronological gates pass again.

No service, SDK or MCP behavior changes with these documents. The remaining work
is the paid collector, content labeling, runtime artifact-gate integration and
canary controller. Reverify hint merging, interception and response costs against
the service revision used for collection. A service contract change would require
coordinated backend, client-library and both MCP-server review; this task makes
none of those changes.
