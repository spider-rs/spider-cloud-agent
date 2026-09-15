# Contributing

## Setup

You need a stable Rust toolchain. Nothing else.

```bash
git clone https://github.com/spider-rs/spider-cloud-agent
cd spider-cloud-agent
cargo test --workspace
```

Live tests are ignored by default and need a key:

```bash
SPIDER_API_KEY=... cargo test -p spider-cloud-agent --features full -- --ignored live_
```

## Before you open a pull request

```bash
scripts/verify.sh
```

That runs every gate this project ships behind: format, clippy with warnings denied,
the tests with all features and again with none, the docs, both leak checks, and a
packaging dry run. It stops at the first failure and names it.

There is no hosted CI. The checks live in that script so they can be read, and so a
green run means the same thing on your machine as on anyone else's. Nothing merges on
the strength of a badge.

The `--no-default-features` run inside it is not optional. It is what proves the crate
still works with no model compiled in, and that path has to keep working.

The `leakcheck` above checks a shorter list than the one that matters. The denylist in
this repo carries only terms already safe to read in public, because spelling out an
internal service name here is the leak the tool exists to stop. The real list lives in the
private checkout and never travels to this repository, not as a file and not as a CI
secret, because a list held to protect a public repo should not sit inside it. Before a
push that changes what the crate ships, run it against the real list:

```
SPIDER_LEAKCHECK_WORDS=<private-checkout>/tools/leakcheck/words.txt \
  cargo run -p xtask -- leakcheck --tree
```

Anyone without that checkout gets the short list, and their green run does not settle it.

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
