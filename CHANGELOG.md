# Changelog

Every release gets an entry here. Anything that changes what a request sends, what a
response gives back, or what an escalation costs gets one whether or not it breaks a
signature, because those are the changes that show up on a bill rather than in a compiler
error.

## [0.3.0]

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

## [0.2.0]

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

## [0.1.1]

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
