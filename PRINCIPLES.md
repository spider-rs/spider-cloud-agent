# spider-cloud-agent principles

The absolutes. Each one carries the reason it exists, because a principle with no reason attached
gets deleted by the first person who finds it inconvenient. `CONTRIBUTING.md` holds the working
conventions. Where the two disagree, this file is the rule.

Each of these names the lint, the type or the test that holds them up. That named thing is what to
break when you want to know whether the principle is still doing anything. The anchor gate checks
every citation against its named symbol or phrase, within two lines, and refuses fewer than 48
citations. Run `scripts/check-anchors.sh` after moving code so these references stay useful.

## Credentials

1. **A key never reaches a log line, an error message, a `Debug` output, a fixture or a commit.**
   The bearer header is the only place the key is written (`spider-cloud-agent/src/transport.rs:696`
   (`fn bearer(`)), `REDACTED` stands in everywhere else (`spider-cloud-agent/src/transport.rs:59`
   (`const REDACTED:`), `spider-cloud-agent/src/client.rs:61` (`const REDACTED:`)), and `Debug` is
   hand written rather than derived on every type that holds one: `Spider` at
   `spider-cloud-agent/src/client.rs:118` (`impl fmt::Debug for Spider {`), `SpiderBuilder` at
   `spider-cloud-agent/src/client.rs:354` (`impl fmt::Debug for SpiderBuilder`), `Credentials` at
   `spider-cloud-agent/src/auth/store.rs:108` (`impl fmt::Debug for Credentials`). The OAuth
   authorization code gets its own newtype for this and nothing else, `Code` at
   `spider-cloud-agent/src/auth/oauth.rs:176` (`struct Code(`), whose `Debug` prints
   `Code(<redacted>)` at `spider-cloud-agent/src/auth/oauth.rs:186` (`Code(<redacted>)`). A
   `#[derive(Debug)]` on a struct that later gains a key field is the ordinary way a credential gets
   out, and where it goes is a log the caller ships somewhere else. The tests that hold this are
   `spider-cloud-agent/src/client.rs:814` (`fn no_error_the_crate_makes_carries_the_key`) and
   `spider-cloud-agent/src/auth/oauth.rs:810` (`fn a_code_never_prints_itself`).

2. **A recorded response becomes a fixture only by way of `cargo run -p xtask -- redact`.** It
   rewrites hosts outside the allowlist to `example.com` (`xtask/src/redact.rs:12`
   (`pub const REPLACEMENT_HOST`)). The allowlist is `ALLOWED_HOSTS` in `xtask/src/fixtures.rs:8`
   (`pub const ALLOWED_HOSTS`): `example.com`, `example.org`, `example.net`, `httpbin.org` and
   `spider.cloud`, including their subdomains. It drops the value of every key that could hold a
   secret, authorization and cookie headers and token fields among them (`xtask/src/redact.rs:18`
   (`const SECRET_KEYS`)). Hand editing a recorded body works until the one time it misses a field,
   and a fixture is the artifact most likely to be read by someone who was not there when it was
   made.

3. **A local `leakcheck` pass is not evidence that anything is safe to publish.** The denylist in
   the repo deliberately holds only publicly safe terms, and its categories ship empty, because
   naming an internal service in a public repo is the leak the tool exists to stop
   (`xtask/src/leak_words.rs:1` (`The word denylist used by`), `xtask/src/leak_words.rs:59`
   (`pub const CATEGORIES`)). The real vocabulary comes from `SPIDER_LEAKCHECK_WORDS`. The release
   mode of `scripts/verify.sh` must run `xtask leakcheck --require-private` against both the
   packaged set and the working tree. The private-list requirement makes a missing list fail.
   Nothing goes public on a green local run alone.

4. **A credential file this crate writes is `0600`, and one it only reads is reported rather than
   quietly changed.** `FILE_MODE` is `0o600` at `spider-cloud-agent/src/auth/store.rs:48`
   (`pub const FILE_MODE`), the mode is read at `spider-cloud-agent/src/auth/store.rs:295`
   (`let mode = std::fs::metadata`), a file readable by others warns once at
   `spider-cloud-agent/src/auth/store.rs:307` (`fn warn_wide_permissions`), and permissions change
   only on the path the caller asked for (`spider-cloud-agent/src/auth/store.rs:327`
   (`std::fs::set_permissions`)). A library that silently rewrites permissions on a file it did not
   create is a worse surprise than the one it set out to fix.

## The two status planes

5. **`ApiStatus` and `PageStatus` never convert into each other.** Neither has a public constructor,
   only the transport mints one, and `ApiStatus` has no `Deserialize` at all, so a response body
   cannot produce one (`spider-cloud-agent/src/status/api.rs:17` (`pub struct ApiStatus`),
   `spider-cloud-agent/src/status/page.rs:17` (`pub struct PageStatus`)). Two `compile_fail`
   doctests hold the boundary at `spider-cloud-agent/src/status/mod.rs:22` (`fn takes_page`) and
   `spider-cloud-agent/src/status/mod.rs:31` (`let _ = PageStatus::from`). The rule table keeps them
   apart as separate triggers (`spider-cloud-agent/src/policy/rule.rs:17` (`pub enum Trigger`)), and
   the send loop sorts a body status the service mirrored onto the envelope back where it belongs
   (`spider-cloud-agent/src/ops/mod.rs:321`
   (`if reply.is_success() || reply.is_mirrored_page_status()`)).

   How the call to Spider Cloud went and how the target site answered are different facts that
   arrive as identical looking numbers. A 403 on the call plane means the key was refused and
   another attempt buys the same refusal. A 403 on the page plane means the site blocked the fetch
   and a heavier request often gets through. Reading one as the other either retries a dead key or
   abandons a page that would have come back, and preventing exactly that is why this crate exists
   rather than a thin binding.

   One hole is deliberate and documented at `spider-cloud-agent/src/policy/engine.rs:72`
   (`pub fn seen(`): `Observed::seen` builds an attempt from two plain numbers so recorded history
   can be replayed with no network. Both status constructors stay `pub(crate)` and the live path
   still goes through them.

## What comes back

6. **The crate asks for what the caller named and switches off the rest.** Every `Need` but
   `Need::Raw` turns off metadata, headers, cookies, page links, structured data and embeddings
   unless it is the need asking for them (`spider-cloud-agent/src/thrift/plan.rs:18`
   (`Every need but`), and the table at `spider-cloud-agent/src/thrift/plan.rs:7`
   (`| need | what it sends |`)). `Need::Fields` sends `return_format=empty` and carries no page
   bytes at all (`spider-cloud-agent/src/thrift/plan.rs:155` (`Need::Fields(spec) =>`)). The API
   returns everything unless told otherwise. This crate returns nothing that was not asked for, and
   that inversion is where nearly all of the saving comes from.

7. **An empty body is a failure only when the caller wanted a body.** `Need::Metadata` and
   `Need::Fields` ask for `return_format=empty` on purpose, so the page arrives with nothing in it
   and that is the request working as asked (`spider-cloud-agent/src/ops/mod.rs:349`
   (`if nothing_came_back(&pages) && !body_was_declined`)). Reading it as a blank page sent the walk
   up the whole ladder against a request that had already succeeded and billed for every step. That
   bill is the reason principle 12 is written down rather than assumed.

## Routing

8. **The routing decision is local and costs nothing before a call goes out.** `spider-route` does
   no network work, runs no async runtime and holds no HTTP client (`spider-route/src/lib.rs:4`
   (`It does no network`)); its only dependencies are `serde` and `url`
   (`spider-route/Cargo.toml:26` (`[dependencies]`)). `scripts/check-route-boundary.sh:16`
   (`allowed = set(`) gates normal dependencies under default and all features against an explicit
   allowlist of `serde`, `url` and their dependencies, and scans production source for I/O. A
   decision that costs a call cannot be made on every call, and one that runs in microseconds needs
   no budget conversation before anyone turns it on.

9. **The router never sees a domain name.** A `FeatureVector` is `[f32; 256]` and nothing else
   (`spider-route/src/features.rs:441` (`bits: [f32; FEATURE_DIM]`)), so there is no string field a
   host could be stored in. A test recursively walks every Rust file under `spider-route/src` and
   fails if `host_str(`, `.host()`, `.domain()` or `Host::Domain` appears anywhere but `domain.rs`,
   which hands out a `HostShape` instead (`spider-route/src/domain.rs:438`
   (`fn the_host_is_read_in_this_module_and_nowhere_else`)). It also asserts the exact module
   inventory visited. The claim is about the shape of the artifact rather than about behaviour, so
   it can be checked by reading the type.

10. **The caller beats the router, and the router beats the default.** A setting the caller made is
    never argued with, and the check reads a snapshot of the request as the caller left it, so a
    value the ladder or the plan wrote later cannot be mistaken for one the caller chose
    (`spider-cloud-agent/src/routing.rs:8` (`The caller beats the router`),
    `spider-cloud-agent/src/routing.rs:53` (`pub(crate) fn pins_from`),
    `spider-cloud-agent/src/routing.rs:66` (`pub(crate) fn apply_decision`)). When a client silently
    overrides a setting the caller wrote down, the request that went out and the request in the code
    no longer match, and nobody can work out which one to trust.

## Escalation

11. **`Policy::decide` performs no input or output.** It opens no connections, starts no timers and
    sleeps for nothing; the send loop owns all of that (`spider-cloud-agent/src/policy/mod.rs:3`
    (`This is the decision layer`), `spider-cloud-agent/src/policy/engine.rs:466`
    (`pub fn decide(`)). `scripts/check-policy-purity.sh:11` (`boundary.scan(`) gates production
    policy source against an explicit allowlist and rejects filesystem, network, process,
    environment, timer and logging use, including imports and macro paths. An escalation strategy
    that only exists inside a network loop can be argued about but not checked. This one is driven
    from a table of scripted responses in `tests/policy_sim.rs`, so a change in the rules shows up
    as a change in the asserted moves and in the total spend.

12. **An escalation answers an observed status, and never runs without room in the budget.** Every
    move comes from a rule matched against what the attempt saw
    (`spider-cloud-agent/src/policy/engine.rs:473` (`let Some(rule) = self.rules.iter().find`)), and
    the step is priced and checked against every cap before it is returned
    (`spider-cloud-agent/src/policy/engine.rs:595` (`let estimate = step.estimate`)). The budget is
    enforced twice, mirrored onto the request so the service refuses to run past the cap even if
    this client stops asking, and checked here before every retry and every escalation
    (`spider-cloud-agent/src/policy/budget.rs:3` (`The budget is enforced twice`)). A rate limit
    from the service is retried and never climbed, because it is about how often the account is
    calling rather than how hard the page is, and a heavier request buys the same refusal at a
    higher price (`spider-cloud-agent/src/policy/rule.rs:39` (`pub fn retry_only`)).

13. **Every ladder step sets documented request parameters and nothing else.** The exact changed
    keys are checked for every rung and standard step on default and populated requests at
    `spider-cloud-agent/src/policy/ladder.rs:368`
    (`fn every_rung_and_standard_step_changes_exactly_its_documented_keys`), matching the contract
    at `spider-cloud-agent/src/policy/mod.rs:31` (`Every step sets documented request parameters`).
    An escalation is therefore always a change the caller could have made by hand and can read back
    off the request afterwards. Nothing in the ladder selects the software that performs a rendered
    fetch, and nothing in it touches the measures the service applies on the caller's behalf, which
    the service decides better than a client can.

## The process it runs in

14. **No panic, no lock, no blocking sleep, no unsafe, in library code.** Denied at the lint level
    rather than left to review: `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`,
    `unimplemented!`, `exit` and `mem::forget` in `[workspace.lints.clippy]` (`Cargo.toml:36`
    (`[workspace.lints.clippy]`)), `Mutex` and `RwLock` from both `std` and `tokio` plus
    `thread::sleep` in `clippy.toml`, and `#![forbid(unsafe_code)]` at
    `spider-cloud-agent/src/lib.rs:15` (`#![forbid(unsafe_code)]`). This crate runs inside someone
    else's process and someone else's async runtime. A panic takes down a program that merely wanted
    a web page, and a lock can stall a task its author never wrote. The way to not deadlock is to
    have nothing to deadlock on. Test modules opt out one at a time, because a test that cannot set
    itself up should stop loudly.

15. **The crate works with no model compiled in.** With no weights, `HeuristicRouter` answers from
    rules alone, and that path is tested on its own (`spider-route/src/heuristic.rs:622`
    (`fn the_rules_answer_with_no_weights_anywhere_in_the_build`)). The model configuration does not
    exist yet and the no-default build is what ships, so this test covers the only path.
    `cargo test -p spider-cloud-agent --no-default-features` is not an optional line in the
    pre-pull-request list. Someone who wants the client and not the classifier has to be able to
    have it, and the only way to know that build still works is to run it.

16. **A check that cannot fail is not a check.** A `cargo test` name filter matching nothing exits
    zero and reads as a pass, and a guard that looks for a string by matching its own source text
    passes for the wrong reason. The host guard counts the files it read and fails below five, so a
    directory walk that returned nothing cannot pass it (`spider-route/src/domain.rs:496`
    (`checked >= 5`)). Break the thing a new guard protects and watch it go red before you trust it.

17. **A new knob on the curated surface needs an argument, not a use case.** The curated set is
    request mode, proxy pool, country, wait condition, profile, timeout, session, budget, need and
    max tokens, plus `page_links` on page operations. `params_mut()` reaches the full parameter set.
    The exact method membership is checked at `spider-cloud-agent/src/ops/mod.rs:1044`
    (`fn curated_surface_membership_is_explicit`), and each method has a reason in
    `docs/action-vocabulary.md`. Every promotion is a label the router has to learn and a column the
    trainer has to carry, and a new routing action lands in `spider-route` first
    (`docs/action-vocabulary.md`). The extra step is there to be paid, not routed around.
