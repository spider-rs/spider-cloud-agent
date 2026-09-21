# spider-optimize

Scores candidate edits after `spider-route` and the request plan, then applies at
most one edit set that passes a gate. The client compiles it in by default through its
`optimize` feature, and it runs only when passed to `SpiderBuilder::optimizer`. Start with `Optimizer::shadow`. In shadow mode the baseline request
goes out unchanged, and the caller's settings always win. See the
[optimizer architecture](https://github.com/spider-rs/spider-cloud-agent/blob/main/docs/optimizer/architecture.md).

With no artifact compiled in or loaded, `NoModel` keeps every request. Artifacts
hold only numbers. They hold no host, resource pattern or page body. Loading can
allocate. Scoring does no I/O, reads no clock and allocates nothing. The optional
embedded artifact is a synthetic fixture. It is no evidence of savings on real pages.

`Monitor` keeps the last `window` settled outcomes in a ring of atomics. It trips when
the edited requests' success rate falls under the kept requests' rate by more than
`max_drop`, after `z` standard errors of the unpaired difference. Once it trips, the
client stops applying edits until `Monitor::reset`.

## Numeric models

`Compact::from_bytes` checks the 2,000,000 byte limit, CRC32, versions, dimensions
and tree topology, then loads owned FP32 tables. `embedded-model` exposes
`embedded()`. The bundled weights are a synthetic test fixture, not a trained
optimizer. Inference needs no dependency, network access or runtime.

Version one starts with the 16 byte `SPOPT` header. The header holds the artifact
version, the model kind, the base feature version, the edit feature version, the
schema version, the maximum width, and a zero reserved byte. Model kind 1 is MLP and
2 is GBDT. Multi-byte numbers are little endian. After the header come the thresholds,
indexed by compact edit code. Then come the sorted support cells, three calibrations,
three independent head payloads, and the CRC32 of all preceding bytes. Width counts
the dense input layer, or the largest tree node count, and cannot exceed 256. Each
dense head ends in one output. Tree children may appear in any order. They must be in
range and acyclic, and that includes unreachable nodes.

Calibration kind 0 has zero knots. Kind 1 has zero knots followed by Platt `(a,b)`
with `sigmoid(a*x+b)`. Kind 2 has at least two increasing x knots and nondecreasing
y values. Isotonic interpolation is linear between knots and constant outside them.
Calibration acts on raw head outputs, and latency and credits then use `expm1`.
Tree comparisons use `x <= threshold`. The high feature bit selects the left branch
only for missing inputs, which are the non-finite ones.

`score_slices` requires exactly 152 base and 96 edit slots. `Scorer::score` and
`score_in_cell` select the 152 used slots from the router's 256 slot storage. Any
non-finite input slot causes NaN success, even in an unused router slot. Dense
inference substitutes zero for non-finite inputs, and trees take their missing
branch. Invalid slice lengths return an unknown score. Scoring allocates nothing and
uses two 256-float scratch arrays.

`cell_id(need, ext, memory_state, edit_code)` packs four bytes from most to least
significant. `KEEP_CODE` is 0. `edit_code(key)` is one plus the rank of a learnable
key in schema order, so it runs from 1 through 9. The trainer must derive the same
order from `schema-v2.json`. Absent and NaN thresholds return `None`. An empty
support table means no restriction. With a nonempty table, `score_slices` and
`score_in_cell` return support 1 only for listed cells. `Scorer::score` has no cell,
so it returns support 0 unless the table is empty.

Run `cargo test -p spider-optimize --lib write_fixture_artifact -- --ignored`
to regenerate both numeric artifacts and their 64-case golden files. JSON null in an
input slot encodes NaN. A null expected success must match NaN exactly. The Rust
writer exists only in unit tests. The Python trainer in `training/` also exports
artifacts. Its reference reader and the Rust reader must agree within absolute error
1e-5.
