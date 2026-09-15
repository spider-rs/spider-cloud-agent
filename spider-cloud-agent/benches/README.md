# Benchmarks

Five benchmark files, four here and one in `spider-route/benches`. Each group exists to
defend a sentence somebody wrote in a README or a doc comment, so a regression reads as
a broken promise rather than as a number that moved.

## Running them

```sh
cargo bench --workspace                     # everything, about four minutes
cargo bench -p spider-cloud-agent --bench decide
cargo bench --workspace -- --test           # compile and run once, no timing
cargo bench --workspace -- decide/accept    # one group, by name
```

`--test` is the mode CI uses on a pull request. It runs every benchmark once and checks
nothing times out or panics, which is enough to catch the failure that actually kills a
benchmark suite: it stops compiling and nobody notices for six months.

Criterion writes HTML to `target/criterion/report/index.html` after a real run, and
compares against the previous run stored in the same place.

## What each group defends

### decide.rs and allocations.rs

The claim, from the repository README: "answers in microseconds, which means it can run
on every request without anyone thinking about cost."

`decide` times `Policy::decide` on five shapes of attempt: a served page that is
accepted, a refused fetch that climbs the ladder, a rate limited fetch that waits, the
same rate limit once the retry at that step is spent, which jumps to the geographic step
instead of walking, and a fetch that has spent past its credit cap. `step_apply` times writing a ladder step onto
a request, once for the one parameter step and once for the geographic step, which is
the widest one.

`policy_build` times the ladder being constructed, which happens once per operation
rather than once per attempt. It is here as a tripwire: if that work ever moves onto the
per attempt path, this number falls and `decide` rises.

`allocations.rs` asks the second half of the question. It installs a global allocator
that counts, takes the smallest of sixteen runs so another thread cannot inflate the
reading, and asserts the count against a recorded figure. The assertion fails in both
directions, because an allocation disappearing is also worth reading.

### serialize.rs

The claim: every field on `RequestParams` skips itself when it is `None`, so a default
body is `{}` and you pay for nothing you did not ask for.

Four bodies, from empty to most of the documented parameter set. The byte counts are
printed above the timings because the size is the claim. A field that loses its skip
attribute shows up twice over: the minimal body grows, and the time to write it moves
toward the populated one.

### decode.rs

The claim: decoding scales with the response and nothing else.

The inputs are the recorded fixtures in `tests/fixtures`, compiled in rather than read
at run time. `decode_response` reads a whole response into the typed pieces a caller
holds. `multi_body` isolates `MultiBody`, which is the one type with a hand written
visitor rather than a derive, because a body arrives as a bare string in some responses
and as an object of named formats in others.

`Page` itself is built by the transport from types that are private to the crate, so a
benchmark outside the crate cannot reach it. What is timed is everything public on that
path: the json parse, then `MultiBody`, `Costs`, `Metadata` and `SearchResults`.

### spider-route/benches/route.rs

The claim, from that crate's README: it answers locally and in microseconds.

There is no router to call yet. The crate today is the vocabulary a routing answer is
expressed in, and the wire encoding for it, so that is what is timed. When
`HeuristicRouter` lands it gets a group in the same file.

## The numbers

Apple M1 Max, macOS 26.6.2, rustc 1.97.1, release profile, 15 September 2026. Taken on a
laptop with an editor open, so read the shape rather than the third digit. Criterion
reports a range and the middle figure is quoted here.

### Allocations per decision

| case | allocations | bytes |
| --- | --- | --- |
| accept | 0 | 0 |
| blocked, escalates | 1 | 184 |
| rate limited, waits | 0 | 0 |
| rate limited, jumps to geo | 2 | 738 |
| budget cap, stops | 1 | 184 |

The headline number is the first row. Accepting a served page allocates nothing, and
neither does deciding to wait. The two escalation rows allocate because the decision
hands the caller the ladder step it chose, as data they can log and act on, and the
geographic step carries a country code that is a `String`.

The last row is the one worth fixing. Stopping on a credit cap allocates once, for a
step that is cloned and then thrown away, because `Policy::escalate` clones first and
asks the budget second. One allocation on a path that ends an operation is not urgent,
and it is not free either. Reversing that order in `src/policy/engine.rs` would take it
to zero.

### Time per decision

| group | time |
| --- | --- |
| decide/accept | 7.7 ns |
| decide/blocked_escalates | 57.6 ns |
| decide/rate_limited_waits | 36.8 ns |
| decide/rate_limited_jumps_geo | 211 ns |
| decide/budget_stops | 124 ns |
| step_apply/first_rung | 13.1 ns |
| step_apply/residential_geo | 78.7 ns |
| policy_build/standard | 584 ns |
| policy_build/for_target | 1.54 µs |
| attempt_state_record | 3.1 ns |

Nanoseconds throughout, against a network call that takes hundreds of milliseconds at
best. The claim holds with about five orders of magnitude to spare.

### Routing vocabulary

| group | time | allocations |
| --- | --- | --- |
| mode_as_str | 0.99 ns | 0 |
| mode_from_wire (exact) | 32.1 ns | 1 |
| mode_from_wire (alias) | 32.4 ns | 1 |
| country_new | 34.7 ns | 1 |
| country_new (rejected) | 6.4 ns | 0 |
| pool_as_str | 0.66 ns | 0 |

`from_wire` lowercases its input into an owned `String` before it compares, so it
allocates once per call whatever it is given. Reading a mode off a response is not a hot
path today, so this is recorded rather than flagged.

### Request bodies

| body | bytes | time |
| --- | --- | --- |
| empty | 2 | 56 ns |
| minimal, one url | 37 | 92 ns |
| trimmed, markdown of one selector | 122 | 191 ns |
| populated, most of the parameter set | 779 | 1.34 µs |

A default body is two bytes, which is `{}`. The populated body is 21 times larger and
costs 24 times as much to write, so the cost tracks the content rather than the size of
the struct.

### Response decoding

| fixture | bytes | time | throughput |
| --- | --- | --- | --- |
| single page | 544 | 3.28 µs | 158 MiB/s |
| mixed crawl, three pages | 629 | 4.42 µs | 136 MiB/s |
| multi format, three formats at once | 404 | 2.71 µs | 142 MiB/s |
| search results | 316 | 788 ns | 382 MiB/s |
| multi_body/markdown_only | 53 | 295 ns | 171 MiB/s |
| multi_body/three_formats | 112 | 292 ns | 366 MiB/s |
| multi_body/blank | 10 | 101 ns | 95 MiB/s |

Search results decode about twice as fast per byte as a page, which is the difference
between a derive and the rest of the work a page needs. `multi_body` reading three
formats costs the same as reading one, so the visitor is paying for bytes rather than
for fields.

## Keeping this file honest

Numbers in a repo rot. If you rerun these on a different machine, replace the date and
the hardware line above rather than adding a second table. The allocation counts are
different: they are a property of the source, not of the machine, so they belong in the
`EXPECTED` tables in the benchmark files, where a change fails the run.
