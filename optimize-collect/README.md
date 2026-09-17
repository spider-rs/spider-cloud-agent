# optimize-collect

The paired comparison collector for `spider-optimize`. It writes the corpus
`training/README.md` describes: `rows.jsonl`, one comparison row per line, and
`manifest.json`. It is a workspace member so clippy and the tests cover it, and it is
never published.

Nothing here spends credits unless you pass `--spend` and a cap.

```
optimize-collect --urls urls.txt --candidates spec.json --out DIR --salt-file salt.bin
                 [--dry-run --base-url http://127.0.0.1:PORT] [--spend --max-credits N]
                 [--repeat-share 0.05] [--need markdown|text|fields:SELECTORS.json|links]
                 [--seed N] [--service-revision TEXT]
```

## Modes and exit codes

- Without `--dry-run` or `--spend` the collector refuses to start.
- `--dry-run` sends every request to `--base-url`, which has to be a loopback address,
  with a placeholder key. No key is looked up.
- `--spend` needs `--max-credits` and takes no `--base-url`. The key comes from the
  usual `Credentials::resolve` order and is never printed.
- `--max-credits` is copied into `Budget::credits` of every run, so the service holds
  each request to it too. After every run the collector adds what it cost, and it stops
  once the total reaches the cap. The pair that run finished is still written.

| Exit | Meaning |
|------|---------|
| 0 | Every url was collected. |
| 1 | Something failed while running, such as a write. |
| 2 | Usage: bad flags, an unreadable or invalid input, or an existing `rows.jsonl`. |
| 3 | The credit cap was reached. `rows.jsonl` holds every pair finished so far and `manifest.json` is written. |

An existing `DIR/rows.jsonl` is never overwritten, because it may have cost money.

## The candidate file

A JSON array. Each entry is one edit, or two edits that form one of the optimizer's
`PAIRS`. Each edit names the field by its wire name, the operation and the value:

```json
[
  [{"key": "request", "op": "set", "value": "browser"}],
  [{"key": "block_stylesheets", "op": "set", "value": false}],
  [{"key": "request", "op": "set", "value": "browser"},
   {"key": "wait_for", "op": "set", "value": 5000}],
  [{"key": "network_blacklist", "op": "append", "value": {"observed": 0}}]
]
```

| Key | Op | Value |
|-----|----|-------|
| `request` | `set` | `"http"`, `"smart"` or `"browser"` |
| `proxy` | `set` | `"isp"` or `"residential"` |
| `wait_for` | `set` | an idle network wait in milliseconds: 2000, 5000 or 10000 |
| `disable_intercept`, `full_resources`, `block_ads`, `block_analytics`, `block_stylesheets` | `set` | `true` or `false` |
| `network_blacklist` | `append` | a list of patterns, or `{"observed": N}` for the rank `N` identifier the url's baseline arm observed |

Every entry goes through `spider_optimize::EditSet::new` and the schema's learnable
kinds before any request goes out. An invalid file exits 2 and names the entry, counted
from zero.

The three blocking switches are on at the service when unset, so `true` for them is the
request as it already goes out and the optimizer never lists it. Such an entry, or any
other the optimizer does not list for a url, is sent, and its pair is dropped with a
line on stderr, because the candidate arm carried the baseline request.

## What one url costs

1. One baseline run: the client's own request, with the optimizer in
   `ApplyMode::Apply` and `NoModel`, which keeps it, and
   `event_tracker.responses = Some(true)` so the page's resources are observed.
2. One run per candidate entry. The optimizer runs in `ApplyMode::Apply` with a
   `ForcedScorer` that scores exactly that edit set as cheaper and surer than keep and
   everything else as keep. A blacklist entry also sets `disable_hints = Some(true)`,
   which the optimizer requires. The written row is checked against the entry
   afterwards: its arm has to be `candidate`, and its edit descriptor and edit features
   have to be the entry's.
3. For a `--repeat-share` of urls, drawn from `--seed`, a second baseline run.

Each run is paired with the baseline run: the two rows share a `pair` nonce, a `day`
(whole days since 2026-01-01 by the wall clock, one reading per url) and a `dk`, which
is the client's FNV-1a of the registrable domain XOR the salt. The baseline row is
written again for each of its pairs.

The client writes each arm's row with the pair-relative labels empty. The collector
keeps both bodies in memory until the pair is done, and fills in the candidate row's
`shingle_jaccard` and `byte_ratio` from the two bodies, `fields_ok` for a fields need,
and `content_ok` by the trainer's rule at tau 0.80. A repeat pair's second baseline run
is written with arm `candidate` and `edit: null`, so every pair has exactly one baseline
arm, and that is the shape `labels.repeat_jaccards` reads tau from.

A walk the client ended without a row, such as a budget stop or a failed connection,
drops that pair. What it cost is still counted.

## The salt

`--salt-file` holds 8 bytes. When the file is missing it is created from
`/dev/urandom` at mode 0600. Keep it: the same salt gives the same `dk` for the same
site, so corpora collected with it can be joined, and a different salt cannot be. The
salt never enters `rows.jsonl` or `manifest.json`. `manifest.salt_id` is the first 8
hex characters of its sha256.

## The manifest

`schema_version`, `feature_version`, `edit_feature_version` and `edit_dim` from the
crate constants, `rows`, `pairs`, `day_min`, `day_max`, `collector_rev` (the commit the
binary was built from), `client_version`, `service_revision` (the flag, or
`unattested`), `credits_spent`, `tau`, `salt_id` and `synthetic: false`.

## Checking a corpus

`tests/dry_run.rs` runs the binary against a loopback stub and checks the pair, host
and shape rules of `training/src/spider_optimize_train/dataset.py::validate` in Rust.
The validator itself is run by hand:

```bash
cd training
uv run spider-optimize-train validate /path/to/DIR
```

A dry run of 30 urls and one entry per learnable key passed every pair, host, version
and shape rule. A stub that serves every arm a page in the end leaves each edit code
with one success outcome, which is the one rule such a run cannot meet.
