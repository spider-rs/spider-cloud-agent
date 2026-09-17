# Optimizer architecture

`spider-optimize` is a second decision layer. It runs after the router and the
need's request plan. It scores candidate edits for success, latency and credits.
It then picks at most one edit set that passes validation and a gate. Keep is
always candidate zero and the baseline. The client asks the optimizer only before
the first attempt. The escalation ladder still handles the statuses that come back.

## The crate boundary

`spider-cloud-agent` optionally depends on `spider-optimize`. That crate depends
on `spider-route`, `serde` and `url`. The optimizer reads the client's
`RequestParams` through `Params`, so it needs no dependency back into the HTTP
client. Loading a `Compact` artifact allocates owned tables. Scoring is
synchronous. It does no I/O, reads no clock or environment, and allocates nothing.

The router's host guard keeps host reads inside `domain.rs`. That file exposes
shape and registrable-domain helpers. The router's feature vector may never see a
host. The route crate allows no dependencies beyond its reviewed closure and no
`include_bytes!`. Resource observation or model bytes in that crate would weaken
this boundary. The optimizer, in contrast, must read concrete observed resource
identifiers to propose a blacklist entry. `observe::summarize` keeps at most four
heavy third party groups. Only their numeric classes and buckets reach features
and rows. The identifier's pattern is there only to be written into
`network_blacklist`.

## Alternatives rejected

| Alternative | Why it does not fit this boundary |
| --- | --- |
| Model inside `spider-route` | Resource identifiers and embedded weights would cross the router's host and artifact guards. |
| One ranking score | A rank is not a probability and cannot be gated as a success probability. Success, latency and credits need separate heads. |
| LightGBM only | Tree tables are the artifact size risk. Training fits and compares LightGBM and an MLP on the same arrays before choosing an export. |
| Per-site learned lists in `SiteMemory` | Its four fields are observations, success rate, streak and last status. The client packs them into one `u64`; the memory may hold no host and cannot hold a list. |

The design shape comes from Magika: bounded inputs, specialised heads and a
confidence fallback. Magika's pretrained weights do not apply to Spider request
edits. Here the inputs are 152 used router slots and 96 edit slots. The three
heads estimate success, `log1p` latency and `log1p` credits. The numeric artifact
reader accepts MLP and GBDT payloads. It checks versions, CRC32, dimensions and
tree topology, and it refuses artifacts over 2,000,000 bytes. The bundled artifact
is synthetic.

## What version one learns

The nine keys are `request`, `proxy`, `wait_for` as an idle network wait,
`disable_intercept`, `full_resources`, `block_ads`, `block_analytics`,
`block_stylesheets` and `network_blacklist`. Modes are HTTP, smart and browser;
pools are ISP and residential. Idle wait buckets are 0, 2,000, 5,000 and 10,000 ms.
Generation never proposes zero, because applying it writes nothing. Each of the
five switches toggles its effective service value. V1 validation allows only
appends to lists, and generated entries come from observed third parties.

Generation is deterministic and stops at 24 candidates, keep included. An edit
set holds at most three distinct keys. Generation offers single-key edits first,
then the compatible pairs `(request, wait_for)`, `(request, proxy)`,
`(block_stylesheets, full_resources)` and `(network_blacklist, request)`, then
single blacklist appends until the cap is reached.

`block_stylesheets` and `network_blacklist` are the learnable keys marked content
changing. The rollout policy keeps them shadow-only. A missing resource can remove
the very content whose delivery is meant to justify the savings. Lifting that
restriction needs paired content labels first. The restriction is operational.
`ApplyMode::Apply` does not enforce it, and the validator explicitly permits these
two exceptions. The validator refuses other content-changing keys. The
[inventory](parameter-inventory.md) records the exact schema flags. Those flags do
not claim that every unflagged setting is semantically harmless.

## Gates and code guarantees

The default `Gate` sets these limits. Success must be at least 0.9 and no more
than 0.01 below keep. Support must be at least 1. Predicted credits per success
must improve by at least 5 percent. The multiplier must be no greater than 8.
Validation checks the caller's budget separately. Candidates rank by credits per
success, then by latency. Keep wins an exact tie. With `min_gain` zero, a
candidate with equal cost and lower latency can win.

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

The client needs a deliberate integration step before it uses artifact
thresholds and support cells. `Compact::threshold` exposes each floor, and
`score_in_cell` evaluates a named support cell. The current client calls the
generic `Scorer::score`, which supplies no cell. With a nonempty support table it
returns support zero. It also ignores the artifact's per-edit floors. Training's
gated policy uses both. Loading an artifact does not prove that the client
enforces that policy. Connect and verify those checks before a canary. See the
[confidence template](confidence-coverage.md) and [rollout](rollout.md).
