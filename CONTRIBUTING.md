# Contributing

## Setup

You need Rust 1.88 or newer and Python 3. Release checks also need cargo-deny, the
1.88 toolchain (`rustup toolchain install 1.88 --profile minimal`, so the gate can
check every target under the declared floor) and `uv` for the trainer's evals under
`training/`. An ordinary run skips those two steps with a printed reason when the
tool is absent.

```bash
git clone https://github.com/spider-rs/spider-cloud-agent
cd spider-cloud-agent
cargo test --workspace
```

Live tests are ignored by default. Run them with the release script described below.

## Before you open a pull request

```bash
scripts/verify.sh
```

That runs every gate this project ships behind: format, clippy with warnings denied,
the tests with all features and again with none, the allocation baselines, the docs,
both leak checks, a dependency audit when cargo-deny is installed, and a packaging
dry run of all three publishable crates. It stops at the first failure and names it.

There is no hosted CI. The checks live in that script so they can be read, and so a
green run means the same thing on your machine as on anyone else's. Nothing merges on
the strength of a badge.

The `--no-default-features` run inside it is not optional. It is what proves the crate
still works with no model compiled in, and that path has to keep working.

## Release checks

There is no hosted CI. Run these gates from a developer machine before a release:

```bash
cargo install cargo-deny --locked
SPIDER_LEAKCHECK_WORDS=<private-checkout>/tools/leakcheck/words.txt \
  scripts/verify.sh --release
SPIDER_API_KEY=<key> SPIDER_API_URL=<service-url> \
  SPIDER_SERVICE_REVISION=<deployed-revision> scripts/verify-live.sh --release
```

The private denylist lives in the private checkout and never enters this repository,
not as a file and not as a CI secret. Release mode fails if the variable is unset,
the file cannot be read, or it contains no terms after blank and comment lines are
removed. The same rule is available directly with `cargo run --locked -p xtask --
leakcheck --require-private`, with `--tree` added to check the working tree. An
ordinary run without the list prints a note and only checks the short public list.
That pass is not release evidence.

The lockfile ships with the binary. Tests, allocation gates and packaging use
`--locked`. Release mode requires cargo-deny and refreshes its advisory database.
Ordinary mode skips the audit with a reason if cargo-deny is absent; otherwise it
uses only cached data with `--offline`. Prepare the cache with `cargo deny fetch`.
A missing or stale cache fails the audit rather than silently skipping it.
`deny.toml` checks advisories, licenses and duplicate versions, with exact exceptions
for the older transitive versions already in the lockfile.

The live script spends credits and requires a non-empty key, an explicit API URL and
`SPIDER_SERVICE_REVISION`. Obtain that revision from the deployment serving that URL;
it is an operator attestation, not the client's Git revision. Python 3 checks the
compiled inventory and the results: all nine tests must execute, eight must pass,
and none may skip. No test-name filter is accepted. The selector-name test is the
tracked F7a exception: the backend extraction cache does not include the selector map
in its key. Its failure is recorded separately, never as a pass. A pass from that test
also fails the gate so the exception must be reviewed and removed after the service fix.
The tests' assertions stay intact.

Keep `target/live-verification/*.json` with the release evidence. Each record contains
the service revision, client revision, test inventory and per-test outcomes. A local
release gate and a live gate against the intended service revision are both required.

## Signing a release

Installed `spider-agent` binaries update themselves from the GitHub release marked
latest. The files a release needs are listed under "What a release must contain" in
[spider-agent-cli/README.md](spider-agent-cli/README.md). After writing
`SHA256SUMS.txt`, sign it:

```bash
minisign -S -s <secret key> -m SHA256SUMS.txt -t "spider-agent v<version> SHA256SUMS.txt"
```

Upload `SHA256SUMS.txt.minisig` with the archives. The trusted comment must match
that text exactly, with the version the tag names.

The secret key lives on the release machine, outside every repository. It never goes
into this one, a CI secret, or a chat. The public key is pinned in
`spider-agent-cli/src/update/release-keys.pub`. Every self-updating install refuses a
release without a valid signature from a pinned key, says so on stderr, and stays on
its current version until a correctly signed release is marked latest.

To rotate the key, add the new public key to `release-keys.pub` and sign that release
with the old key. Installs that take it trust both keys, so the release after it can
be signed with the new key. An install that skips the release carrying both keys can
no longer update itself and needs one manual install.

## Rules the build enforces

This crate runs inside someone else's process and someone else's async runtime,
so four things are denied at the lint level rather than left to review. See
`clippy.toml` and `[workspace.lints]` in the root `Cargo.toml`. The full list of
absolutes, and what enforces each one, is [PRINCIPLES.md](PRINCIPLES.md).

**No panics in library code.** `unwrap`, `expect`, `panic!`, `unreachable!`,
`todo!` and `unimplemented!` are denied. Return an error. A panic here takes
down a program that merely wanted a web page. Test modules opt out with an
inner `#![allow(...)]` block, because a test that cannot set itself up should
stop loudly.

**No locks.** `Mutex` and `RwLock` are disallowed types, `std` and `tokio`
alike. The only shared mutable state is the rate limit snapshot and it is
atomics. A lock inside a library can stall a task its author never wrote, and
the way to not deadlock is to have nothing to deadlock on.

**No leaks.** `mem::forget` is disallowed. There are no reference cycles to
leak through: nothing holds a back-pointer to the client.

**No blocking the runtime.** `thread::sleep` is disallowed. Waiting between
attempts uses the async timer.

Two lints are warnings rather than denials: `indexing_slicing` and
`string_slice`. Read them. Slicing a string at a byte offset panics when the
offset lands inside a multibyte character, and this crate handles fetched web
text, where a price in euros is enough to trigger it. That exact bug has
already been fixed once here.

## Things that will get a pull request sent back

**A test that cannot fail.** A `cargo test` name filter that matches nothing exits zero and
looks like a pass, and a guard that checks for a string by matching its own source text
passes for the wrong reason. If you add a check, break the thing it guards and watch it go
red before you trust it.

**A new knob.** The curated surface is deliberately small: request mode, proxy pool,
country, wait condition, profile, timeout, session. Everything else lives behind
`params_mut()`, which hands over the full documented parameter set. Every knob promoted to
the curated surface is a label the router has to learn and a promise we have to keep, so
adding one needs an argument, not just a use case.

**A fixture with a real host in it.** Recorded responses go through
`cargo run -p xtask -- redact` and end up pointed at `example.com`. The leak checker
enforces this and it is not negotiable.

**Prose that reads like every other README.** Sentence case headings, straight quotes, no
em dashes, plain words. If a sentence would be equally true of some other project, cut it.

## Reporting a security problem

Do not open an issue. See [SECURITY.md](SECURITY.md).
