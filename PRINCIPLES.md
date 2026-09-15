# spider-cloud-agent principles

The absolutes. Each one carries the reason it exists, because a principle with no
reason attached gets deleted by the first person who finds it inconvenient.
`CONTRIBUTING.md` holds the working conventions. Where the two disagree, this file
is the rule.

Most of these name the lint, the type or the test that holds them up. That named
thing is what to break when you want to know whether the principle is still doing
anything. Where a principle names nothing, it is waiting for someone to build the
check, and it is weaker than the ones that do.

## Credentials

1. **A key never reaches a log line, an error message, a `Debug` output, a fixture
   or a commit.** The bearer header is the only place the key is written
   (`spider-cloud-agent/src/transport.rs:12`), `REDACTED` stands in everywhere else
   (`transport.rs:59`, `client.rs:59`), and `Debug` is hand written rather than
   derived on every type that holds one: `Spider` at `client.rs:116`, `SpiderBuilder`
   at `client.rs:308`, `Credentials` at `auth/store.rs:103`. The OAuth authorization
   code gets its own newtype for this and nothing else, `Code` at
   `auth/oauth.rs:169`, whose `Debug` prints `Code(<redacted>)` at `oauth.rs:177`.
   A `#[derive(Debug)]` on a struct that later gains a key field is the ordinary way
   a credential gets out, and where it goes is a log the caller ships somewhere else.
   The tests that hold this are `client.rs:641` and `oauth.rs:715`.

2. **A recorded response becomes a fixture only by way of
   `cargo run -p xtask -- redact`.** It rewrites every host to `example.com`
   (`xtask/src/redact.rs:12`) and drops the value of every key that could hold a
   secret, authorization and cookie headers and token fields among them
   (`redact.rs:17`). Hand editing a recorded body works until the one time it misses
   a field, and a fixture is the artifact most likely to be read by someone who was
   not there when it was made.

3. **A local `leakcheck` pass is not evidence that anything is safe to publish.**
   The denylist in the repo deliberately holds only publicly safe terms, and its
   categories ship empty, because naming an internal service in a public repo is the
   leak the tool exists to stop (`xtask/src/leak_words.rs:1`, `leak_words.rs:59`).
   The real vocabulary arrives in CI through `SPIDER_LEAKCHECK_WORDS`, written from a
   secret (`.github/workflows/rust.yml:71`). Nothing goes public on a green local run
   alone. The run that counts is the one with the private list pointed at it.

4. **A credential file this crate writes is `0600`, and one it only reads is
   reported rather than quietly changed.** `FILE_MODE` is `0o600` at
   `auth/store.rs:43`, the mode is read at `store.rs:258`, a file readable by others
   warns once at `store.rs:270`, and permissions change only on the path the caller
   asked for (`store.rs:290`). A library that silently rewrites permissions on a file
   it did not create is a worse surprise than the one it set out to fix.

## The two status planes

5. **`ApiStatus` and `PageStatus` never convert into each other.** Neither has a
   public constructor, only the transport mints one, and `ApiStatus` has no
   `Deserialize` at all, so a response body cannot produce one
   (`src/status/api.rs:15`, `src/status/page.rs:11`). Two `compile_fail` doctests
   hold the boundary at `src/status/mod.rs:20` and `mod.rs:28`. The rule table keeps
   them apart as separate triggers (`policy/rule.rs:17`), and the send loop sorts a
   body status the service mirrored onto the envelope back where it belongs
   (`ops/mod.rs:194`).

   How the call to Spider Cloud went and how the target site answered are different
   facts that arrive as identical looking numbers. A 403 on the call plane means the
   key was refused and another attempt buys the same refusal. A 403 on the page plane
   means the site blocked the fetch and a heavier request often gets through. Reading
   one as the other either retries a dead key or abandons a page that would have come
   back, and preventing exactly that is why this crate exists rather than a thin
   binding.

   One hole is deliberate and documented at `policy/engine.rs:63`: `Observed::seen`
   builds an attempt from two plain numbers so recorded history can be replayed with
   no network. Both status constructors stay `pub(crate)` and the live path still
   goes through them.

## What comes back

6. **The crate asks for what the caller named and switches off the rest.** Every
   `Need` but `Need::Raw` turns off metadata, headers, cookies, page links,
   structured data and embeddings unless it is the need asking for them
   (`thrift/plan.rs:19`, and the table at `plan.rs:7`). `Need::Fields` sends
   `return_format=empty` and carries no page bytes at all (`thrift/need.rs:38`).
   The API returns everything unless told otherwise. This crate returns nothing that
   was not asked for, and that inversion is where nearly all of the saving comes from.

7. **An empty body is a failure only when the caller wanted a body.**
   `Need::Metadata` and `Need::Fields` ask for `return_format=empty` on purpose, so
   the page arrives with nothing in it and that is the request working as asked
   (`ops/mod.rs:206`). Reading it as a blank page sent the walk up the whole ladder
   against a request that had already succeeded and billed for every step. That bill
   is the reason principle 12 is written down rather than assumed.

## Routing

8. **The routing decision is local and costs nothing before a call goes out.**
   `spider-route` does no network work, runs no async runtime and holds no HTTP
   client (`spider-route/src/lib.rs:1`); its only dependencies are `serde` and `url`
   (`spider-route/Cargo.toml:22`). A decision that costs a call cannot be made on
   every call, and one that runs in microseconds needs no budget conversation before
   anyone turns it on.

9. **The router never sees a domain name.** A `FeatureVector` is `[f32; 256]` and
   nothing else (`spider-route/src/features.rs:440`), so there is no string field a
   host could be stored in. A test walks every file in the crate and fails if
   `host_str(`, `.host()`, `.domain()` or `Host::Domain` appears anywhere but
   `domain.rs`, which hands out a `HostShape` instead
   (`spider-route/src/domain.rs:438`). The claim is about the shape of the artifact
   rather than about behaviour, so it can be checked by reading the type.

10. **The caller beats the router, and the router beats the default.** A setting the
    caller made is never argued with, and the check reads a snapshot of the request
    as the caller left it, so a value the ladder or the plan wrote later cannot be
    mistaken for one the caller chose (`src/routing.rs:8`, `routing.rs:43`,
    `routing.rs:65`). When a client silently overrides a setting the caller wrote down, the
    request that went out and the request in the code no longer match, and nobody can
    work out which one to trust.

## Escalation

11. **`Policy::decide` performs no input or output.** It opens no connections, starts
    no timers and sleeps for nothing; the send loop owns all of that
    (`src/policy/mod.rs:3`, `policy/engine.rs:3`). An escalation strategy that only
    exists inside a network loop can be argued about but not checked. This one is
    driven from a table of scripted responses in `tests/policy_sim.rs`, so a change in
    the rules shows up as a change in the asserted moves and in the total spend.

12. **An escalation answers an observed status, and never runs without room in the
    budget.** Every move comes from a rule matched against what the attempt saw
    (`policy/engine.rs:455`), and the step is priced and checked against every cap
    before it is returned (`engine.rs:584`). The budget is enforced twice, mirrored
    onto the request so the service refuses to run past the cap even if this client
    stops asking, and checked here before every retry and every escalation
    (`policy/budget.rs:1`). A rate limit from the service is retried and never
    climbed, because it is about how often the account is calling rather than how
    hard the page is, and a heavier request buys the same refusal at a higher price
    (`policy/rule.rs:33`).

13. **Every ladder step sets documented request parameters and nothing else.**
    (`src/policy/mod.rs:29`.) An escalation is therefore always a change the caller
    could have made by hand and can read back off the request afterwards. Nothing in
    the ladder selects the software that performs a rendered fetch, and nothing in it
    touches the measures the service applies on the caller's behalf, which the
    service decides better than a client can.

## The process it runs in

14. **No panic, no lock, no blocking sleep, no unsafe, in library code.** Denied at
    the lint level rather than left to review: `unwrap`, `expect`, `panic!`,
    `unreachable!`, `todo!`, `unimplemented!`, `exit` and `mem::forget` in
    `[workspace.lints.clippy]` (`Cargo.toml:28`), `Mutex` and `RwLock` from both
    `std` and `tokio` plus `thread::sleep` in `clippy.toml`, and
    `#![forbid(unsafe_code)]` at `spider-cloud-agent/src/lib.rs:14`. This crate runs
    inside someone else's process and someone else's async runtime. A panic takes
    down a program that merely wanted a web page, and a lock can stall a task its
    author never wrote. The way to not deadlock is to have nothing to deadlock on.
    Test modules opt out one at a time, because a test that cannot set itself up
    should stop loudly.

15. **The crate works with no model compiled in.** With no weights, `HeuristicRouter`
    answers from rules alone, and that path is tested on its own
    (`spider-route/src/lib.rs:8`). `cargo test -p spider-cloud-agent
    --no-default-features` is not an optional line in the pre-pull-request list. Someone
    who wants the client and not the classifier has to be able to have it, and the only
    way to know that build still works is to run it.

16. **A check that cannot fail is not a check.** A `cargo test` name filter matching
    nothing exits zero and reads as a pass, and a guard that looks for a string by
    matching its own source text passes for the wrong reason. The host guard counts
    the files it read and fails below five, so a directory walk that returned nothing
    cannot pass it (`spider-route/src/domain.rs:483`). Break the thing a new guard
    protects and watch it go red before you trust it.

17. **A new knob on the curated surface needs an argument, not a use case.** The
    curated set is request mode, proxy pool, country, wait condition, profile,
    timeout and session; everything else the API documents is reachable through
    `params_mut()`. Every promotion is a label the router has to learn and a column
    the trainer has to carry, and a new routing action lands in `spider-route` first
    (`docs/action-vocabulary.md`). The extra step is there to be paid, not routed
    around.
