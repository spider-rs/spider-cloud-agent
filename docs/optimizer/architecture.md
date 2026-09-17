# Optimizer architecture

`spider-optimize` is a second decision layer after the router and the need's
request plan. It scores candidate edits for success, latency and credits, then
chooses at most one edit set past validation and a gate. Keep is always candidate
zero and the baseline. The client consults it only before the first attempt;
the escalation ladder still answers the statuses that actually come back.

## The crate boundary

`spider-cloud-agent` optionally depends on `spider-optimize`, which depends on
`spider-route`, `serde` and `url`. The optimizer accesses the client's
`RequestParams` through `Params`, avoiding a dependency back into the HTTP client.
Loading a `Compact` artifact allocates owned tables; scoring is synchronous,
does no I/O, reads no clock or environment, and allocates nothing.

The router's host guard confines host reads to `domain.rs`, which exposes shape
and registrable-domain helpers. Its feature vector may never see a host. The
route boundary admits no new dependencies beyond its reviewed closure and no
`include_bytes!`. Putting resource observation or model bytes there would weaken
that boundary. The optimizer must read concrete observed resource identifiers to
propose a blacklist entry. `observe::summarize` retains at most four heavy third
party groups; only their numeric classes and buckets reach features and rows.
The identifier's pattern exists to be written into `network_blacklist`.

## Alternatives rejected

| Alternative | Why it does not fit this boundary |
| --- | --- |
| Model inside `spider-route` | Resource identifiers and embedded weights would cross the router's host and artifact guards. |
| One ranking score | A rank is not a probability and cannot be gated as a success probability. Success, latency and credits need separate heads. |
| LightGBM only | Tree tables are the artifact size risk. Training fits and compares LightGBM and an MLP on the same arrays before choosing an export. |
| Per-site learned lists in `SiteMemory` | Its four fields are observations, success rate, streak and last status. The client packs them into one `u64`; the memory may hold no host and cannot hold a list. |

Magika contributed the design shape: bounded inputs, specialised heads and a
confidence fallback. Its pretrained weights do not apply to Spider request edits.
Here the inputs are 152 used router slots and 96 edit slots, and the three heads
estimate success, `log1p` latency and `log1p` credits. The numeric artifact reader
accepts MLP and GBDT payloads, checks versions, CRC32, dimensions and tree topology,
and refuses artifacts over 2,000,000 bytes. The bundled artifact is synthetic.

## What version one learns

The nine keys are `request`, `proxy`, `wait_for` as an idle network wait,
`disable_intercept`, `full_resources`, `block_ads`, `block_analytics`,
`block_stylesheets` and `network_blacklist`. Modes are HTTP, smart and browser;
pools are ISP and residential. Idle wait buckets are 0, 2,000, 5,000 and 10,000 ms,
but generation never proposes zero because applying it writes nothing. The five
switches toggle their effective service value. Lists are append-only in v1
validation, and generated entries come from observed third parties.

Generation is deterministic, capped at 24 candidates including keep. An edit set
holds at most three distinct keys. Generation offers single-key edits, then the
compatible pairs `(request, wait_for)`, `(request, proxy)`,
`(block_stylesheets, full_resources)` and `(network_blacklist, request)`, then
single blacklist appends until the cap is reached.

`block_stylesheets` and `network_blacklist` are the learnable keys marked content
changing. They stay shadow-only in the rollout policy because a missing resource
can remove the content whose successful delivery is supposed to justify savings.
Paired content labels are required before reconsidering that restriction. This is
an operational restriction, not a property of `ApplyMode::Apply`: the validator
explicitly permits these two exceptions. Other content-changing keys are refused.
The [inventory](parameter-inventory.md) records the exact schema flags, which are
not a claim that every unflagged setting is semantically harmless.

## Gates and code guarantees

The default `Gate` requires success at least 0.9 and no more than 0.01 below keep,
support at least 1, at least a 5 percent gain in predicted credits per success,
and a multiplier no greater than 8. Validation separately checks the caller's
budget. Candidates rank by credits per success, then latency. Keep wins an exact
tie; with `min_gain` zero, equal cost and lower latency can win.

| Invariant | Tests that enforce it |
| --- | --- |
| Keep is first and generation is bounded and repeatable | `keep_is_always_first`, `never_more_than_max_candidates`, `generation_is_deterministic` |
| No weights leave the request unchanged | `no_model_always_keeps`, `no_model_abstains_through_every_wrapper` |
| The caller's snapshot wins, including either network list | `an_edit_never_touches_a_field_the_caller_set`, `a_whitelist_on_the_caller_blocks_a_blacklist_edit`, `a_caller_supplied_blacklist_is_never_appended_to` |
| Pinning mode, pool or country prevents all edits | `a_pinned_request_is_never_edited`, `apply_mode_writes_only_unpinned_keys` |
| Low success, weak support, inadequate gain or NaN keep the baseline | `a_gate_never_applies_below_the_success_floor`, `nan_scores_abstain`, `keep_wins_ties` |
| Dependencies, budget, hints and rate limits constrain edits | `a_dependency_violation_is_rejected`, `over_budget_is_rejected`, `blacklist_needs_disable_hints`, `rate_limit_rejects_every_heavier_candidate` |
| Shadow changes no request; escalation retains an applied blacklist | `shadow_mode_sends_the_baseline_request_unchanged`, `an_applied_blacklist_survives_escalation` |
| Features and rows contain no host or body | `a_host_never_reaches_features_or_rows`, `rows_hold_no_url_host_or_body`, `edit_features_hold_no_text` |
| Invalid artifact structure is refused | `hostile_bytes_never_load_or_panic`, `malformed_tables_with_valid_checksums_are_rejected` |

Artifact thresholds and support cells need a deliberate integration step.
`Compact::threshold` exposes each floor, and `score_in_cell` evaluates a named
support cell. The generic `Scorer::score` used by the current client supplies no
cell: with a nonempty support table it returns support zero. It does not consume
the artifact's per-edit floors. Training's gated policy does consume both.
Do not treat loading an artifact as proof that the client enforces that policy;
connect and verify those checks before a canary. See the
[confidence template](confidence-coverage.md) and [rollout](rollout.md).

## Related repositories

These documents change no API payload or service behavior, so they require no
backend, SDK or MCP release. A collector must pin the backend service revision
and verify interception, hint merging and billed costs against it. Any future
change to those service semantics needs review in `spider-cloud-backend`,
`spider-clients`, `spider-cloud-mcp-server` and `spider-cloud-mcp-v2`; that work is
outside this documentation change.
