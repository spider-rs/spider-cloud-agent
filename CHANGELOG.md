# Changelog

Every release gets an entry here. Anything that changes what a request sends, what a
response gives back, or what an escalation costs gets one whether or not it breaks a
signature, because those are the changes that show up on a bill rather than in a compiler
error.

## Unreleased

- Reject redirects in the built-in API transport with `Error::Transport` before
  sending a second request. Until now the client followed them, because F1 landed
  without the `redirect::Policy::none()` an earlier draft carried. The API is a
  JSON POST API, so following a 302 is never what the caller asked for: reqwest
  rewrites the POST as a GET and drops the body, a same-host hop resends the
  bearer to a path nobody named, a cross-host hop strips it and arrives
  unauthenticated, and either way two requests happen inside one attempt, so the
  attempt and cost accounting undercount what was spent. Removing the policy is
  what `a_302_redirect_is_a_transport_error_after_one_request` was shown red
  against: the call came back `Ok` with a page fetched from the `Location`
  address. Callers who pass their own client through `Transport::with_client`
  keep its redirect policy. Truncated bodies and a connection closed mid body
  already fail as `Error::Transport`, chunked bodies decode, and a gzip body is
  not decoded and fails as `Error::Decode`, because the built-in client does not
  enable reqwest's gzip feature.

- The API plane error codes now cross a real socket in `tests/wire.rs`: 401 and
  402 stop after one request, 403 stops after one, and 500 retries once per
  ladder rung until the five-attempt cap, with every attempt keeping its API
  status. The same four codes in a page-shaped body take the page path instead,
  which is the F1 classification held in place. The stub can write raw bytes, so
  a body shorter than its declared length, a connection closed mid body, chunked
  framing, a gzip body and a 302 each have a case that names the terminal result
  and the request count.

- Fixtures for `/links`, `/transform`, `/fetch/{domain}/{path}`, `/data/{table}`
  and for 401, 402, 429 and 503 error envelopes, each the output of
  `cargo run --locked -p xtask -- redact` over a recording held outside the repo.
  The error records carry their HTTP status and the headers the send loop reads,
  so a recorded 429 proves the one-second `retry-after` beats the two-second
  `ratelimit-reset` fallback. `every_fixture` now recurses, so `fixtures/thrift/`
  is inside "every fixture parses" and "documentation hosts only" rather than
  beside it, and a coverage manifest in `tests/endpoints.rs` maps every builder
  route to a fixture and checks the fixture holds that route's answer, by the
  fields only that answer carries.

- Release verification requires a readable private denylist with at least one term
  and cargo-deny. Ordinary verification keeps the private-list note and uses cached
  advisories when cargo-deny is installed. Private-list regression tests run inside
  the unit test executable so concurrent checkouts cannot substitute an older CLI.
- Ship Cargo.lock and declare Rust 1.88, the floor required by the locked ICU crates.
  Test and package with the lockfile, audit dependencies, dry-run all three crates
  in dependency order, and run the client and route allocation baselines.
- Add a release-only live gate that requires credentials, an explicit endpoint and
  the deployed service revision. It accounts for all nine tests and records the
  selector-name service failure as the tracked F7a exception, never as a pass.
  Explicit live test runs without a key now fail.
- Breaking: `Error::Auth` is now `Auth { cause: AuthCause, message }`, with the
  causes `NoKey`, `EmptyKey`, `Refused`, `SignInFailed` and `Local`, so a key
  the service refused can be told from a key that was never there.
  `Error::Exhausted` gained `reason: StopReason` and
  `source: Option<Box<Error>>`, and its message now says why the walk stopped.
  A walk that got a page back and then stopped, on a refusal, a rate limit or
  a failed call, ends in `Exhausted` with that call error as its source. A walk
  that never got a page back still fails with the call error itself. The new
  `Error::recovery()` answers whether to wait, sign in again, or stop.
- A 502, 504 or 408 from the service is retried the way a 503 is: up to three
  times, honoring `Retry-After`, and never climbing the ladder, so a gateway
  failure no longer pays for a heavier page request. The statuses are named once
  by the new `ApiStatus::is_transient` (408, 500, 502, 503, 504), and
  `Error::is_retryable` reads it, so `is_retryable` now includes 408. A test fails
  when a transient status loses its retry rule or the two disagree.
- Request and builder `Debug` output hides caller cookies, every header value,
  proxy credentials and other free-form credential input. Header names and proxy
  endpoints remain visible. Serialization still sends the original values.
  Regression tests cover all seven operation builders and nested request types.
- Site memory capacity 0 disables the store without allocating a table. Reads
  return no record and writes do nothing. Unit tests cover disabled storage, and
  a loopback routing test compares repeated calls with capacities 0 and 16.

- `spider-agent run --plan` now validates typed plan values before making a call.
  Explicit flags override the plan, including `--goal markdown` and `--expand 0`.
  A quoted or negative budget leaves with code 2 instead of dropping the cap.
  Plan addresses accumulate with positional URLs and `--urls-from`.
- `spider-agent schema` now emits clap's command tree with argument types, arity,
  defaults, choices, required flags, conflicts and requirements, plus the plan
  format. Command prose lives under `command_notes`; `commands` contains structured
  subcommands. The exit table uses the labels from the process exit code type.
- `spider-agent transform` leaves with code 1 and names the failed conversion
  when its attempts produce no usable document. It previously used the site
  refusal code even though transform fetches no site.

### Breaking schema output changes

`commands` now maps names to structured command objects instead of prose. Readers
that need the old command descriptions must use `command_notes`. `exit_codes`
now maps numbers to bare labels such as `budget`, replacing descriptions such as
`budget: a cap stopped the run`. These labels match error records.

- Re-anchor every principle to its named code or check. The anchor gate checks
  symbols within two lines and requires at least 48 citations.
  Moving the bearer anchor to line 1 was rejected; restoring it passed all 52 anchors.
- Gate the router's normal dependencies and production source, and the policy's
  freedom from I/O. Each source check counts the files it visits.
- Check every ladder rung and standard step for exactly its documented changed
  keys, and make curated method membership and its vocabulary rows explicit.
- Walk router modules recursively for host access and assert the exact inventory.
- Correct the fixture host allowlist promise and document that no model
  configuration exists yet. Principle 3 names F7a's private release gate.
- Every operation ends. `Budget::wall` defaults to fifteen minutes, which
  leaves room for five browser attempts and the waits between them, and the
  wall is one deadline over every send and every sleep in the walk rather
  than a check between calls. Account reads keep their own minute, or the
  client's wall when it is shorter. `SpiderBuilder::without_wall` and
  `Budget::without_wall` are the opt-out; only the builder's lifts the read
  wall. `Spider::raw` is unchanged and has no wall.
- The API base must be `https`. A base that is not is refused at build time
  with `Error::Config`, except a literal loopback address, which is what a
  test stub listens on, and a base the builder was told to accept with
  `allow_insecure_http(true)`. `SPIDER_API_URL` is held to the same rule.
- Nothing caps the size of an answer unless the client is built with
  `max_response_bytes`. An answer past that cap ends the operation in
  `Error::Exhausted` with `Error::ResponseTooLarge` as its source and the
  cut-off call among its attempts, so what the walk spent before it is still
  on the error. Raw calls are never capped.
- An empty 204 is an empty `Pages` from `send_all` rather than a decode
  failure. `send` has no single page to return and ends in `Error::Exhausted`
  with no last page.
- A page with no status of its own reports an unknown status, code zero,
  rather than a copy of the call's status. On a page route that is a failed
  page. On `transform`, whose documents were never fetched, it is the
  document, and `Page::status` says so in its doc.
- A 401 or 402 whose body is a page, with a `url` and a `status`, is the site
  refusing the fetch: it walks the ladder and keeps its cost. A 401 or 402
  with an error envelope and no page is still the account being refused,
  and stops after one request. An attempt whose status was mirrored from the
  page records an unknown API status rather than one made from the page.
- `Page::cookies` is a map from name to value. The service sends cookies as
  a map, and the crate read them as a string, so asking for cookies dropped
  the page and its cost. A string on the wire is still accepted and split
  into the map.

## 0.3.1 (2026-09-16)

`spider-agent --budget` and `--wall` now cap the whole run on every command. They were
handed to each operation afresh, and a command that works a list of addresses runs one
operation per address, so `scrape` over a hundred addresses under `--budget 1` paid for a
hundred pages and left with code 0. Only `run` carried the spend across pages. The check
is now made before every page on `scrape`, `crawl`, `extract`, `links` and `screenshot`,
what is left is handed to the next operation as its cap, and a run a cap stopped leaves
with code 4 and `stopped` set on the report. The first page still goes out under a credit
cap, because a price is not known until it is asked for. A wall that is already spent
stops the run before anything is sent.

- A reader that closes stdout early, `spider-agent ... | head -1`, ends the run quietly
  with code 0 and no further page is fetched. It was reported as an output failure, code
  7, with `Broken pipe` on stderr.
- A progress note written to a stderr nobody is reading is dropped. It panicked, and the
  release profile turns a panic into an abort.
- `-o -` writes to stdout. It created a file called `-`.
- A byte order mark at the start of an address list, a selector map or a plan is dropped.
- `--budget` and `--max-per-page` refuse `nan`, `inf` and a negative number as a usage
  failure. `nan` was read as a cap that never stopped anything.

Every call is now held to the wall budget. A service that accepted a request and never
answered held `scrape`, `crawl`, `fetch`, `search` and the account reads for as long as
the socket stayed open, because the wall was only looked at between calls. A call that
runs out of wall returns `BudgetExceeded` with kind `Time`. The account reads fall back to
60 seconds when no wall is set, and the default client gives up on a connection that has
not opened after 30 seconds.

- The wait before a retry comes from the answer being retried. It came from the client's
  last rate limit snapshot, so one `ratelimit-reset: 20` made every later retry on that
  client sleep 20 seconds.
- A base URL with a path and no trailing slash keeps its path. `https://proxy.example/spider`
  sent requests to `https://proxy.example/scrape`.
- A rate limit count past `u32::MAX` saturates. It wrapped to zero.
- A negative or unreadable cost no longer lowers what a walk has spent. A negative cost
  read as a refund and let the walk keep escalating past its credit cap.
- A cost line that is `null` or a quoted number reads as a cost. Either one failed the
  whole page, which had already been paid for. A search result with a `null` url no
  longer loses the rest of the list.
- Exploration is skipped on a call where the caller pinned a setting. The wire kept the
  pin, but the outcome and the recorded row reported the explored mode.
- The session step on the ladder can fire. Nothing outside the tests marked whether a
  request carried cookies, so a login wall never got its second call.
- A trim never cuts inside a zero width joiner sequence or a flag, and `max_tokens` never
  reads below `approx_tokens` on whitespace outside ASCII.
- `TokenBudget::split` no longer overflows on large costs. In a release build it handed
  out shares far larger than the total.
- `Pages::digest` counts its own headings against the total, measures pages the way it
  caps them, and honours a per page cap set without a total. It went over its total, and
  cut pages that fit.
- Signing in reads callback connections side by side, so a local connection that sends
  nothing no longer holds up the browser.
- The credentials file is written to a temporary file and renamed into place. Two writers
  could mix their keys, a crash could leave the file empty, and a symlink at the path
  was written through. A credentials file over 4 KiB is not read.
- `spider-agent login --print --json` carries the key. It printed a record without one,
  and `--print` does not store the key, so it was lost.
- `spider-route` reads a NaN success rate as no history and a NaN confidence as zero. The
  first landed in the highest success band.

## 0.3.0 (2026-09-15)

A page and the links on it are one call. `return_page_links` was set in two places inside
the crate and was nowhere on the curated surface, so a caller who wanted both paid for two
calls. The service never asked for that. Measured against https://spider.cloud on
2026-09-15, one scrape asking for both came back with 4,704 bytes of markdown and 91 links
for 3.5393 credits, where the same page without the links cost 3.5180 and a links call on
its own costs about 0.36. A crawl that is already paying for pages can now read the link
graph for half a percent more.

- `page_links(bool)` is on `scrape`, `crawl`, `fetch`, `screenshot` and `search`. It is not
  on `links`, which asks for them already, and not on `transform`, which fetches nothing.
- `spider-agent --with-links` does the same from the command line, on `scrape`, `crawl`,
  `fetch`, `extract`, `screenshot` and `search`. The links are written on the `page` record
  beside the content.
- `Pages::links()` reads the whole set as one list, in the order the links were found and
  with repeats removed. `Spider::links` returns that same list and is unchanged.
- `ThriftReport` counts the links it handed back as payload. It counted the text, the
  extractions and the metadata, so a request that returned ninety addresses and no body
  read as a hundred per cent saving, which is a true number and a dishonest one.

A mode the caller named now survives an escalation. Every rung of the ladder renders the
page, and a rung was written over the request after the caller's own settings, so someone
who asked for plain HTTP to hold the bill down was sent up to a browser and billed for it.
The router already respected that pin, twice over, which is what made the gap in the
ladder easy to miss. The three modes reach the wire as `http`, `smart` and `browser`, and
there are tests on the socket for all of it now. An unrecognised `--mode` is a usage
failure rather than a value the service quietly reads as `http`.

`spider-agent table <name>` is gone. It took any name and read it under `/data`, which
made the tool a way to ask the service which tables it holds and what is in them. That is
a wider reach than a command line tool needs, and nothing anybody used it for needed the
name to be free.

What to use instead, if you are on 0.2.0:

- `spider-agent table websites` is now `spider-agent sites`. One row per site configured
  on the account, with the settings the service holds for it: anti_bot, blacklist, cache,
  crawl_budget, created_at, domain, fts, full_resources, gpt_config, headless, id,
  last_checked_at, metadata, proxy_enabled, return_format, scheme, smart_mode, subdomains,
  tld, updated_at, url, user_id, whitelist.
- `spider-agent table api_keys` is now `spider-agent keys`. Metadata about the keys on the
  account and no key: created_at, id, last_used, team_id, team_member_id, token_name,
  updated_at, user_id. There is no token, secret or hash column in that reply, and
  token_name is the label you gave a key rather than any part of it.
- `spider-agent table profiles` is now `spider-agent profile`. One row: plan limits, usage
  totals, billing caps and the account email. `credits` is still the shorter answer for
  what is left to spend.
- `spider-agent table crawl_logs` was already `spider-agent logs`, and did not change.

All three take `--json`, `--ndjson`, `-o` and `-d` like the rest, and all three write
`row` records. `profile` takes no `--limit` or `--page`, because one row does not page.
`spider-agent schema` names the three and no longer names `table`.

- `Spider::table(name)` stays. The command was the problem, not the endpoint: a client
  library that can reach an endpoint the service has is what a client library is for, and
  `Spider::raw` could reach `/data/{table}` whether or not the builder existed, so removing
  it would have cost a caller the paging and the row unwrapping and taken away nothing
  else. It is documented as the escape hatch for `/data`. What the caller owns is written
  down there: the name is a constant at a call site rather than a string that arrived from
  outside the program, the columns belong to the service, and whether to read a given table
  at all is a decision the library does not make.
- A path argument carrying a `.` or `..` segment is refused before a request is built.
  Slashes survive encoding so that one argument can be a whole path, and the price of that
  was a dot segment: `Url::join` resolves it, so `/data/{table}` filled in with
  `../v1/scrape` called `/v1/scrape`. The route is supposed to decide which part of the API
  a call reaches. `/fetch/{domain}/{path}` had the same hole and is closed by the same
  check. A dot inside a segment is still a filename, so `docs/a.b.html` is unaffected.
- A `/data` read that answers with one object rather than a list unwraps to one row with
  its own columns. `/data/profiles` sends `{"data": {...}}` where the crawl record sends
  `{"data": [...]}`, and the second shape was the only one being unwrapped, so a profile
  arrived as one row whose only column was `data`.

## 0.2.0 (2026-09-15)

The operation called `fetch` was posting to `/scrape`. It came back with pages and nobody
noticed, because a scrape is what most callers wanted. Two things were wrong underneath
it. There was no way to say `scrape`, which is the crate's main operation and now has its
own name. And `/fetch`, a different endpoint the route table already listed, could not be
reached at all: no builder named it, and it was declared a get, which the server answers
with a 400 before the handler runs. The route as shipped on 0.1.0 and 0.1.1 had never
worked and could not have.

What to rename if you are on 0.1.1:

- `Spider::fetch(url)` is now `Spider::scrape(url)`, and `ops::fetch::Fetch` is
  `ops::scrape::Scrape`. Same endpoint, same parameters, same result. Replacing `.fetch(`
  with `.scrape(` is the whole migration for most callers.
- `spider-agent fetch <url>` is now `spider-agent scrape <url>`. The bare
  `spider-agent <url>` form still scrapes and did not change.

Neither one keeps a deprecated alias. The name `fetch` is not retired, it is reused for
the endpoint it always named, so the old spelling would leave two things called `fetch`
doing different work. The crates are a day old, and a compiler error naming the
replacement is a better hour than a silent change of endpoint.

- `Spider::fetch(domain, path)` reaches `POST /fetch/{domain}/{path}`. It takes a host and
  a path instead of an address, because that is what the endpoint takes. The service
  answers with a config it holds for that path, which fields to pull, whether the page
  needs a browser, what to wait for, and reads the request body as overrides on top of
  that rather than as the whole request. The first call on a path nobody has asked for yet
  goes away and works one out, which is far slower than a scrape and can fail with a 503
  while it tries. The query string is not part of the target. `spider-agent fetch <domain>
  [path]` is the same thing from the command line, and its goal defaults to raw so the
  stored config decides. Naming a goal argues with that config and generally wins.
  Measured against example.com on 2026-09-15, asking for markdown cost 0.1087 credits and
  still came back as fields, against 0.0090 for the same answer with the goal left alone.
- `route::FETCH` is a post. It was a get, and a get there is a 400.
- The wire tests assert the request line, not only that a call succeeded. A test that
  checked the pages came back passed against this bug for two releases.
- Fourteen routes in the table have no builder: the six `/v1/` twins, the three
  `unlimited` paths and the five `ai` ones. That is on purpose, `tests/endpoints.rs` now
  says so in writing, and it fails on a route that is in neither list. All fourteen are
  reachable through `Spider::raw`.
- `spider-agent schema` lists `scrape` and `fetch` separately and says which is which, and
  a test fails if a command is added without a line there.

## 0.1.1 (2026-09-15)

Four fixes, every one of them found by driving the library from the command line
tool against the live service rather than by reading it. The first one is a bill.

- `links()` made five calls for one answer and then failed. The endpoint returns
  addresses and no page, the send loop read that as a blank page, and the walk
  climbed the whole ladder against an answer it already had. Measured on
  2026-09-15: five attempts, 164 seconds and 0.4446 credits, ending in
  `Error::Exhausted`, for links the first attempt returned in 820 ms for 0.0087
  credits. Anyone calling `links()` on 0.1.0 pays about fifty times the price of
  the call and gets an error instead of the links. The request now says that no
  body is wanted, which is the thing principle 7 is about, and one call is one
  call.
- `transform()` could not read its own answer. The endpoint sends one object
  whose `content` is a list of converted documents, one string per document sent,
  and reading that list as a page body asked for bytes, so every call came back
  as `invalid type: string, expected u8`. Each document now arrives as its own
  result. The endpoint was unusable on 0.1.0.
- A screenshot comes back as a picture. Asked for nothing in particular the
  endpoint sends the image as base64 text, so `Body::Screenshot` was never built
  and a caller writing the body to a file got base64 in something named `.png`.
  The request now asks for bytes and the service sends a PNG. Nothing decodes
  base64 and no dependency was added, because the fix is in the request.
- A search reports what it cost. An answer that is not a page recorded
  `Credits::ZERO` whatever the body said, so a search spent nothing as far as the
  report and the credit budget were concerned. The cost block is now read from
  the body and reported. One thing this does not fix: the live service sends no
  cost block with a result list, and the 10 credits a query costs appear in
  neither search path, so a search still reports less than it spends.

- `spider-agent-cli`, a third crate, producing the `spider-agent` binary. It is
  separate from the library so that clap and its dependency tree do not reach a
  program that only wanted the client. Results go to stdout and diagnostics to
  stderr, `--ndjson` streams a line per result, and the exit code says which of
  auth, budget, a refusing site, transport and output stopped a run.
- The release profile is tuned for the binary: `opt-level = "z"`, fat LTO, one
  codegen unit, `panic = "abort"` and symbols stripped. The stripped binary goes
  from 6,389,704 bytes to 2,378,976, and the build from 43 s to 157 s. Startup
  did not move. `[profile.bench]` keeps the library benches on the settings
  they were recorded under.
