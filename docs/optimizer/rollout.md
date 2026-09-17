# Optimizer rollout

The client optimizer is compiled in by default through the `optimize` feature. It
runs only when supplied to `SpiderBuilder`. The sequence below is the rollout
plan. It does not claim that paid collection or a production canary has happened.
The paired collector is `optimize-collect`. Its flags and dataset output are in
`optimize-collect/README.md`.

## Offline on synthetic data

Run the training pipeline against its planted synthetic effects first. Compare
MLP and LightGBM on the same splits, calibration, content labels and paired gates.
Keep the FIXTURE-ONLY banner on every synthetic report and the label on every
printed numeric result. Synthetic parity and passing gates show that the pipeline
works on those fixtures. They show no real savings or content safety.

These commands come from [training/README.md](../../training/README.md). Run them
from the repository root, then inside `training` as shown. They send no paid
requests. Setup may need dependency downloads. Collection is a separate activity.

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

Run the two regression scenarios as well, as `training/evals/run.sh` does. Both
are synthetic. `synth --scenario reversal` plants a residential effect. The effect
holds through the days the model learns from and turns harmful inside the test
window. The gate must reject that artifact, even though the model had no evidence
of the flip. The regression report's comparison table must name the harmful
overrides. `synth --scenario stable` is the same plant with no flip. The gate must
pass it with edits applied, at coverage of at least 0.05, so a run cannot pass by
abstaining on every pair. `spider-optimize-train tradeoff` then shows what the
floors buy on the test window for each risk budget. Passing both shows that the
evaluation catches a reversal and does not reward a policy that does nothing. It
says nothing about real requests.

The default threshold settings can correctly abstain on every edit. The fixture
recipe in the training README relaxes the coverage, risk and site minimums to
exercise the reader. Those settings are not production approval criteria. The
int8 evaluation is Python-only, and the Rust artifact is FP32. Domain splits are
diagnostic, and `gates` requires the chronological split.

## Pilot and spend cap

Once a collector exists, start with about 200 paired trials. This spends
credits. It needs explicit authorisation and a hard total spend cap before it
runs. Pin the collector, client and service revisions. Record the real spend in
the manifest. Stop at the cap, counting retries and baseline repeats. The pilot
tests collection and labeling. It cannot meet the regression gate's minimum of
300 chronological test pairs, and it cannot guarantee 200 covered rows for each
edit.

For planning only, price every arm with the client's multiplier table, relative
to an HTTP base price. These are spend assumptions, not measured service prices.

| Arm | Assumed multiplier |
| --- | --- |
| HTTP | 1.0 |
| Smart | 1.5 |
| Browser | 4.0 |
| Browser with wait | 5.0 |
| Smart residential | 3.0 |
| Browser residential with wait | 8.0 |

With the planned arm mix, assume about five base units per pair. The 200-pair
pilot is then about 1,000 base units, 5,000 pairs about 25,000, and 20,000 pairs
about 100,000. Add ten percent to each planning amount for repeats and retries.
Real prices come from `Costs.total_cost` on each response, not from the multiplier
table. The manifest's `credits_spent` records actual spend. Do not report these
planning amounts as observed costs or savings.

Blacklist arms need `disable_hints` set, as the optimizer requires. Use the same
`disable_hints` setting on both arms and record the collection conditions. The
optimizer never sets `event_tracker`.
The collector must explicitly request its `request_map` and `response_map`, and
account for what that observation costs.

## Shadow before applying

Use `Optimizer::shadow` and a `ComparisonRecorder`, with a collection-specific
salt and a day `Clock`. Shadow scores the candidates. For the selected edit, it
records the numeric descriptor, the features and the planned multiplier. Next to
those, it records the actual baseline walk's final success, status, wire bytes,
attempt count, cost and elapsed attempt time. The client logs the candidate count,
whether it applied, and a keep reason. The row holds no predicted scores and no
complete candidate ranking.

The baseline request goes out unchanged. Shadow cannot see the content, latency
or bill of the alternative it never ran. Attaching baseline outcomes to a proposed
edit does not make paired causal evidence. The client writes one row, and only
when the policy accepts or stops. That row has `pair: 0` and null pair-relative
labels. Walks cut short by wall, budget or connection failure get no comparison
row. To fill these evidence gaps, a later collector must run both arms and join
them.

## Bounded canary

Before applying edits, connect the artifact's per-edit floors and support cells
to the client scorer. Verify that it matches the training policy. The current
generic path does not enforce those floors. Build the share selector and the
allowlists in the caller or in a later integration. The present `Optimizer` has
no hash-share selector and no per-need allowlist. The rolling monitor does exist,
as described below.

Select a bounded share by a stable hash. Before starting, record the share,
seed, need allowlist, edit-code allowlist, spend cap and stop conditions. Only
needs with judged content qualify, and only edit codes that passed their own
evidence gates. Keep `block_stylesheets` and `network_blacklist` shadow-only
during this rollout. A pin on any caller field still wins. Pinning mode, pool or
country skips optimization entirely.

Set a `Monitor` on the optimizer with `Optimizer::with_monitor`, defined in
`spider-optimize/src/monitor.rs`. The client feeds the monitor every settled
outcome, edited or kept. The monitor holds the last `window` of them. The window
ranges from 64 to 8,192 and defaults to 1,024. The check starts once the window
has at least `min_applied` edited outcomes and `min_kept` kept outcomes, and both
default to 100. The monitor takes the kept success rate minus the applied one. It
subtracts `z` standard errors of that difference, using a one sided normal
approximation with `z` defaulting to 1.645. If the result is above `max_drop`,
which defaults to 0.02, the monitor trips.

A tripped monitor latches. Every later request in `Apply` mode goes out as a
shadow request, and its row says `"fallback":true`. The client logs one warning
with the counts and the drop. Nothing is applied again until `Monitor::reset`.
Below the minimums the monitor is warming and has no say in the decision. A canary
that never reaches `min_applied` edits is therefore never judged.

Detection has a cost. The monitor says nothing before `min_applied` edited
outcomes have settled. A change part way through a window is diluted by the
healthy outcomes still in it. The losses before a trip are therefore up to
`min_applied` degraded outcomes, plus however many the window hides. The unit test
`degradation_trips_within_the_window_and_the_losses_are_counted` measures both on
a seeded stream. In that stream applied success drops from 0.9 to 0.6, the window
is 1,024 and the minimum is 64. The trip came 272 outcomes after the drop. Of
those, 141 were edited, and 35 more of the edited ones failed than the kept rate
would give. The wire test `a_tripped_monitor_falls_back_to_shadow` sees exactly
`min_applied` failed edits before the trip when every edit fails.

The comparison is unpaired. The gate chose which requests to edit, so the two
groups hold different requests. A drop can come from the edit, from the model's
choice of requests, or from which sites happened to be fetched. The monitor is a
tripwire that stops further losses. It does not measure the edit's effect. That
measurement comes from the paired rows and the offline gate. The gate still
requires 300 chronological test pairs, and the monitor does not replace it.

To kill the canary, drop the optimizer from the builder and construct the client
without `.optimizer(...)`. No model result can affect new requests through that
client. Removing the `optimize` feature also removes the integration at build
time, but stopping a canary should not require a rebuild.

## Rollback and follow-up work

If a gate breaks or evidence is missing, return to shadow, or replace the client
with one built without the optimizer. Keep the artifact identifier, manifest,
threshold report and paired regression report for the failed window. The change
affects future operations. It does not undo requests already sent or credits
already spent. Do not widen the share until the failing need/edit cells have new
paired evidence and the chronological gates pass again.

The remaining work is the paid collector, content labeling, runtime artifact-gate
integration and the canary controller.
