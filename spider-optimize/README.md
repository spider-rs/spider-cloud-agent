# spider-optimize

Edits a [Spider Cloud](https://spider.cloud) request after `spider-route` has picked its
settings, and only when a scorer has the evidence for it.

It lists valid candidate edits to the request parameters, scores each for success,
latency and credits, and applies at most one edit set that passes validation and the
gate. Otherwise the request goes out unchanged. A field the caller set is never edited.

The crate does no network work, holds no clock and allocates nothing while it scores.
With no model compiled in, `NoModel` abstains and every request is kept.

See the [repository README](https://github.com/spider-rs/spider-cloud-agent) for the full
description.

## Numeric models

`Compact::from_bytes` loads owned FP32 tables after checking the 2,000,000 byte
limit, CRC32, versions, dimensions and tree topology. `embedded-model` exposes
`embedded()`. The bundled weights are a synthetic test fixture, not a trained
optimizer. No dependency, network access or runtime is needed for inference.

Version one starts with the 16 byte `SPOPT` header: artifact version, model kind
(1 MLP, 2 GBDT), base feature version, edit feature version, schema version,
maximum width, and a zero reserved byte. Multi-byte numbers are little endian.
Thresholds indexed by compact edit code precede sorted support cells, three
calibrations, three independent head payloads, and the CRC32 of all preceding
bytes. Width includes the dense input layer, or the largest tree node count,
and cannot exceed 256. Each dense head ends in one output. Tree children may
appear in any order but must be in range and acyclic, including unreachable nodes.

Calibration kind 0 has zero knots; kind 1 has zero knots followed by Platt `(a,b)`
with `sigmoid(a*x+b)`; kind 2 has at least two increasing x knots and nondecreasing
y values. Isotonic interpolation is linear between knots and constant outside.
Calibration acts on raw head outputs; latency and credits then use `expm1`.
Tree comparisons use `x <= threshold`. The high feature bit selects the left
branch only for missing (non-finite) inputs.

`score_slices` requires exactly 152 base and 96 edit slots. `Scorer::score` and
`score_in_cell` select the 152 used slots from the router's 256 slot storage.
All non-finite input slots cause NaN success, including unused router slots.
Dense inference substitutes zero for non-finite inputs; trees use their missing
branch. Invalid slice lengths return an unknown score. Scoring allocates nothing
and uses two 256-float scratch arrays.

`cell_id(need, ext, memory_state, edit_code)` packs four bytes from most to least
significant. `KEEP_CODE` is 0; `edit_code(key)` is one plus the rank of a learnable
key in schema order (1 through 9). The trainer must derive that same order from
`schema-v1.json`. Absent and NaN thresholds return `None`.
An empty support table means no restriction. With a nonempty table,
`score_slices` and `score_in_cell` return support 1 only for listed cells.
`Scorer::score` lacks a cell and returns support 0, unless the table is empty.

Run `cargo test -p spider-optimize --lib write_fixture_artifact -- --ignored`
to regenerate both numeric artifacts and their 64-case golden files. JSON null
in an input slot encodes NaN; null expected success must match NaN exactly.
The Rust writer exists only in unit tests. The future Python trainer replaces
it as the producer and must pass both parity tests within absolute error 1e-5.
