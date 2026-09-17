# Dataset format

A corpus has `rows.jsonl` and `manifest.json`. Each JSONL line describes one arm
of a paired request in the format written by `spider_optimize::row::comparison_row`.
The collector assigns a pair ID, executes a baseline and a candidate or baseline
repeat on the same site and day, compares their content, and stores only scalars.
The client's comparison recorder alone is not a corpus: it writes `pair: 0` and
leaves all pair-relative labels null. A shadow pick has no executed alternative.

## Comparison row

Integer widths below name the Rust producer types. JSON consumers must preserve
`u64` values exactly, including `pair`, `dk` and `pinned_hi`. The writer emits
non-finite floats as zero and absent optional values as null; zero is therefore
not proof that the original measurement was finite.

| Key | Type | Meaning |
| --- | --- | --- |
| `v` | `u32` | Comparison row version, currently 1 |
| `schema_v` | `u16` | Parameter schema version, currently 1 |
| `feat_v` | `u16` | Base feature version, currently 1 |
| `edit_feat_v` | `u16` | Edit feature version, currently 1 |
| `pair` | `u64` | Collector-assigned trial ID shared by its arms |
| `arm` | Fixed string | `baseline`, `candidate` or `shadow` |
| `fallback` | Boolean | Whether a tripped `Monitor` shadowed a pick the client would otherwise have applied; the arm is then `shadow`. Absent in rows written before the monitor existed, which read as false |
| `day` | `u32` | Day count supplied by the caller's `Clock`; zero without one |
| `dk` | `u64` | Salted site grouping key, never an input feature |
| `need` | Fixed string | `text`, `markdown`, `html`, `links`, `metadata`, `fields`, `screenshot`, `raw`; writer fallback `other` |
| `ext` | Fixed string | `none`, `markup`, `xml`, `json`, `feed`, `text`, `csv`, `pdf`, `office`, `image`, `media`, `archive`, `asset`, `other` |
| `tld` | `u8` | Router host-shape label-group slot; client uses 255 when absent |
| `mem` | Fixed string | `cold` for zero observations, `thin` for 1-2, `warm` for 3-9, `steady` for 10 or more |
| `routed` | Object | Router decision before optimization, expanded below |
| `edit` | Object or null | Numeric edit descriptor, null for keep |
| `pinned` | `u32` | Caller-presence mask over schema key indices 0-31 |
| `pinned_hi` | `u64` | Caller-presence mask starting at key index 32, including all remaining keys |
| `success` | Boolean | Whether the executed arm succeeded; not itself a pair-relative content check |
| `status` | Fixed string | `unknown`, `ok`, `empty`, `bad_request`, `needs_login`, `blocked`, `not_found`, `rate_limited`, `server_error`; writer fallback `other` |
| `millis` | `u32` | Client sums attempt elapsed durations, saturating to this width; not a separate end-to-end wall-clock measurement |
| `bytes` | `u32` | Wire bytes of the final outcome, not all loaded browser resources |
| `credits` | `f64` | Client sums finite positive attempt costs for the settled walk |
| `attempts` | `u8` | Attempt count, capped at 255 |
| `multiplier` | `f32` | Planned multiplier of the described candidate, including an unexecuted shadow pick |
| `fields_requested` | `u8` | Number of extraction groups across requested paths, capped at 255 |
| `fields_present` | `u8` | Nonempty values on the first successful page, capped at 255 |
| `content_ok` | Boolean or null | Collector's pair-relative content judgment; trainer recomputes it |
| `fields_ok` | Boolean or null | Pair-relative requested-field preservation judgment |
| `shingle_jaccard` | `f32` or null | Similarity of normalized baseline and candidate text |
| `byte_ratio` | `f32` or null | Smaller content byte count divided by larger |
| `base` | Array of 152 `f32` | Used router features, finite within [-1, 1] |
| `edit_feats` | Array of 96 `f32` | Initial learnable configuration, edit, need and observation buckets, finite within [-1, 1] |

| Nested key | Type | Meaning |
| --- | --- | --- |
| `routed.mode` | Fixed string | Router's `http`, `smart` or `browser` mode |
| `routed.proxy` | Fixed string | Router's `isp` or `residential` pool |
| `routed.wait_ms` | Integer | Router's wait in milliseconds |
| `routed.start_rung` | Integer | Router's starting ladder rung |
| `routed.source` | Fixed string | `heuristic`, `model`, `caller`, `memory`, `explore`, or fallback `other` |
| `routed.confidence` | `f32` | Router confidence, distinct from the optimizer's predicted success |
| `edit.key` | `u8` | Schema key index, not compact edit code |
| `edit.op` | `u8` | Set 0, append 1, remove 2, replace 3, clear 4; v1 validation permits only set or blacklist append |
| `edit.bucket` | `u8` | Value index within its kind, observed identifier rank for append, or 255 when no value is known |
| `edit.ident` | Object or null | Observed identifier expressed only as buckets |
| `edit.ident.class` | `u8` | Extension-class index in the `ext` order above |
| `edit.ident.share` | `u8` | Share of loaded bytes: under 1%, under 5%, under 15%, under 40%, at least 40% as 0-4 |
| `edit.ident.count` | `u8` | Resource count: 1, 2-3, 4-9, 10-29, at least 30 as 0-4 |
| `edit.ident.third` | Boolean | Whether the resource group is third party; summaries propose only third parties |

For an edit set, the descriptor chooses the blacklist append if present, otherwise
the first key in schema order. The edit feature bits describe the whole set, but
do not reconstruct exact patterns or a full request. Compact edit code 0 means
keep; codes 1-9 follow learnable-key order in `training/fixtures/schema-v1.json`.
`request` is 1 and `network_blacklist` is 9.

## Manifest

These are the fields in `training/src/spider_optimize_train/dataset.py::Manifest`.
Real corpus files live under the ignored `training/data/` directory.

| Key | Type | Meaning |
| --- | --- | --- |
| `schema_version` | Integer | Parameter schema version matched by `schema_v` |
| `feature_version` | Integer | Base layout version matched by `feat_v` |
| `edit_feature_version` | Integer | Edit layout version matched by `edit_feat_v` |
| `edit_dim` | Integer | Edit vector width, currently 96 |
| `rows` | Integer | Recorded row count |
| `pairs` | Integer | Distinct paired-trial count |
| `day_min` | Integer | First collection day |
| `day_max` | Integer | Last collection day |
| `collector_rev` | String | Revision of the producer assigning pairs and labels |
| `client_version` | String | Client version used to execute the arms |
| `service_revision` | String | Revision of the service that actually served the arms |
| `credits_spent` | Number | Actual billed collection spend, including repeats and retries |
| `tau` | Number | Collection-time text similarity threshold |
| `salt_id` | String | Identifier of the collection salt, not the salt itself |
| `synthetic` | Boolean | Requires the FIXTURE-ONLY banner and numeric output labels when true |
| `planted` | Object | Synthetic effect metadata; defaults to an empty object |

## Validation and splitting

`validate` requires both files and reports the violations it finds. It refuses
fewer than 200 rows or 100 distinct pairs, and requires exactly one baseline arm
per pair. All arms of a pair must have the same `day` and `dk`. Each learnable edit
code needs at least 20 nonbaseline edited rows and both success outcomes. This
checks coverage of every code, not just codes a proposed deployment allowlists.

The validator rejects any row string, including an object key, containing `://`
or matching its host-like pattern. It checks base width 152, edit width against
the manifest, numeric non-Boolean feature values, finiteness and [-1, 1] bounds.
Row schema, base-feature and edit-feature versions must equal the manifest;
manifest schema version and edit width must match the schema mirror.
The current validator does not exhaustively validate every scalar or vocabulary,
does not cross-check manifest count/day/spend summaries, and does not require
exactly two arms. A collector still has to supply truthful measurements and a
usable nonbaseline arm. `train` refuses a corpus that validation refuses.

The chronological split allocates 60, 15, 10 and 15 percent of days to train,
tuning, calibration and test. A diagnostic domain split uses `dk % 5`;
regression gates refuse a run trained with that split. Training adds a four-slot
pinned-count bucket to the 152 base and 96 edit slots. Export folds that bucket
at zero pins because the Rust scorer has no pinned-count input.

## Why a salted key is allowed

The client hashes the lowercase registrable domain with 64-bit FNV-1a, then XORs
the recorder's salt into the result. Addresses without a registrable domain use
zero before salting. `with_salt` sets the salt; the default is zero, so a collector
must supply a collection-specific salt and keep it out of the dataset.

`dk` permits within-corpus grouping, distinct-site support counts and held-out
site checks without storing a host. It is excluded from the feature arrays and
the numeric model. That narrow use makes it acceptable where a host in weights
would not be. FNV plus XOR is not cryptographic anonymization, and the key should
not be described as impossible to reverse or link. Publishing the salt or using
the default zero would weaken even this separation.

## Pair-relative labels

`labels.py::content_ok` recomputes labels in this order. Null means unjudged,
not verified content preservation.

| Need | Exact `content_ok` rule |
| --- | --- |
| `screenshot`, `raw` | Always null, even when status is not `ok` |
| Every other need with status other than `ok` | False |
| `fields` with status `ok` | Stored `fields_ok`, including null if absent |
| `links`, `metadata` with status `ok` | Null if `fields_requested` is zero; otherwise `fields_present / fields_requested >= 0.9` |
| `text`, `markdown`, `html` with status `ok` | Null if either scalar is absent; otherwise `shingle_jaccard >= tau` and `0.5 <= byte_ratio <= 2.0` |

The implementation's fallback for any other need uses the body rule. The Rust
`byte_ratio` helper is symmetric, smaller over larger, so its output cannot
exceed 1; the trainer still tests the literal interval above. Two empty contents
have ratio 1, and one empty content has ratio 0.

`shingle_jaccard` lowercases text and splits whitespace, then compares sets of
five-word shingles. If either side has fewer than five words it compares word
sets. Two empty texts score 1; only one empty scores 0. Tau is the fifth percentile
of baseline-repeat Jaccards when at least 20 repeats exist, otherwise 0.80. A repeat
is a nonbaseline arm whose edit descriptor is null. No text is stored in the row.

`fields_ok` requires every requested field to exist and be nonempty in the
candidate. After lowercasing and collapsing whitespace, a nonempty baseline value
must equal or be contained in the candidate value. If the baseline lacks a value,
the candidate still has to supply one. The first value under each name is compared;
unrequested fields are ignored. Counts alone cannot establish this label.

The training success label is `success && content_ok is not False`. A null label
therefore does not disqualify a successful arm, which is why rollout cannot infer
content safety from that label alone. A regression is a baseline success lost by
the candidate, or a successful candidate whose content is judged broken.
