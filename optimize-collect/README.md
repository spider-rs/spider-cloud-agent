# optimize-collect

The paired comparison collector for `spider-optimize`. It writes the corpus that
`training/README.md` describes: `rows.jsonl`, with one comparison row per line, and
`manifest.json`. It is a workspace member, so clippy and the tests cover it. It is
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
- `--dry-run` sends every request to `--base-url` with a placeholder key. The address
  has to be a loopback address. No key is looked up.
- `--spend` needs `--max-credits` and takes no `--base-url`. The key comes from the
  usual `Credentials::resolve` order and is never printed.
- `--max-credits` is copied into `Budget::credits` of every run, so the service also
  holds each request to it. After every run the collector adds what the run cost. It
  stops once the total reaches the cap. The pair that run finished is still written.

| Exit | Meaning |
|------|---------|
| 0 | Every url was collected. |
| 1 | Something failed while running, such as a write. |
| 2 | Usage: bad flags, an unreadable or invalid input, or an existing `rows.jsonl`. |
| 3 | The credit cap was reached. `rows.jsonl` holds every pair finished so far and `manifest.json` is written. |

An existing `DIR/rows.jsonl` is never overwritten, because it may have cost money.

## The candidate file

The candidate file is a JSON array. Each entry is one edit, or two edits that form one
of the optimizer's `PAIRS`. Each edit names the field by its wire name, the operation
and the value:

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

Every entry is checked with `spider_optimize::EditSet::new` and the schema's learnable
kinds before any request goes out. An invalid file exits 2 and names the entry, counted
from zero.

The three blocking switches are on at the service when unset. Setting one to `true`
sends the request as it already goes out, so the optimizer never lists that edit. The
collector still sends such an entry, and any other entry the optimizer does not list for
a url. It then drops the pair and prints a line on stderr, because the candidate arm
carried the baseline request.

## What one url costs

1. One baseline run. This is the client's own request, with the optimizer in
   `ApplyMode::Apply` and `NoModel`, which keeps it. It sets
   `event_tracker.responses = Some(true)` so the page's resources are observed.
2. One run per candidate entry. The optimizer runs in `ApplyMode::Apply` with a
   `ForcedScorer`. That scorer scores exactly that edit set as cheaper and surer than
   keep, and everything else as keep. A blacklist entry also sets
   `disable_hints = Some(true)`, which the optimizer requires. Afterwards the written
   row is checked against the entry. Its arm has to be `candidate`, and its edit
   descriptor and edit features have to match the entry.
3. A second baseline run for a `--repeat-share` of urls, drawn from `--seed`.

Each run is paired with the baseline run. The two rows share three values:

- a `pair` nonce
- a `day`, counted in whole days since 2026-01-01 by the wall clock, with one reading
  per url
- a `dk`, the client's FNV-1a of the registrable domain XOR the salt

The baseline row is written again for each of its pairs.

The client writes each arm's row with the pair-relative labels empty. The collector
keeps both bodies in memory until the pair is done. It then fills in these fields on
the candidate row:

- `shingle_jaccard` and `byte_ratio`, from the two bodies
- `fields_ok`, for a fields need
- `content_ok`, by the trainer's rule at tau 0.80

A repeat pair's second baseline run is written with arm `candidate` and `edit: null`.
Every pair then has exactly one baseline arm. `labels.repeat_jaccards` reads tau from
pairs of that shape.

If the client ends a walk without a row, the pair is dropped. A budget stop or a
failed connection can cause this. The walk's cost still counts.

## The salt

`--salt-file` holds 8 bytes. If the file is missing, the collector creates it from
`/dev/urandom` at mode 0600. Keep the file. The same salt gives the same `dk` for the
same site, so corpora collected with one salt can be joined. Corpora with different
salts cannot. The salt never enters `rows.jsonl` or `manifest.json`.
`manifest.salt_id` is the first 8 hex characters of its sha256.

## The manifest

The manifest holds these fields:

- `schema_version`, `feature_version`, `edit_feature_version` and `edit_dim`, from the
  crate constants
- `rows`, `pairs`, `day_min` and `day_max`
- `collector_rev`, the commit the binary was built from
- `client_version`
- `service_revision`, from the flag, or `unattested`
- `credits_spent`, `tau`, `salt_id` and `synthetic: false`

## Checking a corpus

`tests/dry_run.rs` runs the binary against a loopback stub. It checks, in Rust, the
pair, host and shape rules of `training/src/spider_optimize_train/dataset.py::validate`.
Run the validator itself by hand:

```bash
cd training
uv run spider-optimize-train validate /path/to/DIR
```

A dry run of 30 urls, with one entry per learnable key, passed every pair, host,
version and shape rule. A stub that ends up serving every arm a page leaves each edit
code with one success outcome. That is the one rule such a run cannot meet.

## Measured on 2026-09-17, fixture model

Taken with `scripts/measure-optimize.sh` at 20260917T031555Z on Darwin arm64, rustc 1.97.1, commit 493d4fae0ffe. The only model in `spider-optimize/assets` is the synthetic fixture the trainer exports from its planted corpus. Every number here shows what carrying a model of that shape costs. None of them says anything about a trained model. A wall time of 0.0 means under the 10 ms resolution of `/usr/bin/time`.

The command line tool never constructs an optimizer. Its size difference is what compiling the feature in adds, before the linker drops what nothing calls. The two examples make one scoring call each. One goes through the bundled artifact and the reader, and the other goes through `NoModel`. Their difference is the reader and the artifact.

### Model bytes, synthetic fixture model, not a trained one

| Asset | Bytes |
|---|---:|
| `spider-optimize-v1.bin` | 153229 |

### Stripped release binaries, synthetic fixture model, not a trained one

| Binary | Bytes |
|---|---:|
| spider-agent, without the feature | 2711728 |
| spider-agent, with `spider-cloud-agent/optimize` (compiled, not called) | 2728416 |
| spider-agent difference | 16688 |
| `keep_no_model` example | 302528 |
| `score_embedded` example, reader and artifact linked | 467808 |
| example difference | 165280 |

### One scoring call from a cold start, 20 runs each, synthetic fixture model, not a trained one

| Program | Peak RSS min (bytes) | Peak RSS median (bytes) | Wall min (s) | Wall median (s) |
|---|---:|---:|---:|---:|
| `keep_no_model` | 1523712 | 1523712 | 0.0 | 0.0 |
| `score_embedded` | 1998848 | 2015232 | 0.0 | 0.0 |

### Criterion means, `cargo bench -p spider-optimize --bench optimize`, synthetic fixture model, not a trained one

| Benchmark | Mean |
|---|---:|
| `generate` | 3.672 µs |
| `featurize_edit` | 35.7 ns |
| `score_mlp` | 22.613 µs |
| `score_gbdt` | 7.205 µs |
| `choose_no_model` | 9.7 ns |
