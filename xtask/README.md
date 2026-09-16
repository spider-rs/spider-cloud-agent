# xtask

Repo tasks for spider-agent. This crate is never published.

## leakcheck

```
cargo run -p xtask -- leakcheck            # the packaged file set, what cargo package would ship
cargo run -p xtask -- leakcheck --tree     # the working tree, for fast local iteration
cargo run -p xtask -- leakcheck --require-private # require a readable list with at least one term
cargo run -p xtask -- leakcheck --explain  # what the denylist categories are for
```

It checks, in this order:

- a word denylist, see `src/leak_words.rs`
- host and address patterns: `.internal`, `.local`, `.cluster.local`, `amazonaws.com`,
  RFC1918 literals, loopback literals in shipped source, and any other bare IPv4 that is
  not an RFC5737 documentation address
- fixture hosts, which must be `example.com`, `example.org`, `example.net`, `httpbin.org`
  or `spider.cloud`
- the model artifact under `spider-route/assets`, which must carry no printable ASCII run
  longer than 8 bytes after its 16 byte header, no domain-like string, and no size past
  the baseline in `artifact-baselines.json`
- environment reads in the published crates, which may name only `SPIDER_API_KEY`,
  `SPIDER_CLOUD_API_KEY`, `SPIDER_API_URL` and `SPIDER_MCP_SERVER`. `env!` and
  `option_env!` may also read the `CARGO_` values cargo defines at build time.

Findings print as `path:line: reason` and the command exits non-zero on the first one.
The last line always says how many files were checked, so a pass cannot be confused with
having checked nothing.

The packaged mode is the one that counts before a release, because `include = [...]`
decides what ships and the working tree holds files it leaves out. Use `--tree` while
iterating, and when the workspace does not compile, since `--tree` reads files and never
builds.

### The private denylist

The list compiled into this crate holds only terms that are safe to read in a public
repo. The real list, with internal service, cluster, queue, engine and codename
vocabulary, lives in the private checkout. Point `SPIDER_LEAKCHECK_WORDS` at it:

```
SPIDER_LEAKCHECK_WORDS=/path/to/private/leak-words.txt cargo run -p xtask -- leakcheck
```

One term per line. Blank lines and lines starting with `#` are skipped. A term may carry
a `category:` prefix, which only changes how the finding is labelled.

There is no hosted CI. Before a release, run the release mode of
`scripts/verify.sh` on a developer machine with `SPIDER_LEAKCHECK_WORDS` pointing
to the list in the private checkout. That mode runs both checks with `--require-private`. Missing, unreadable and
zero-term lists fail. Ordinary checks keep the private list optional. Do not
move the private terms into this repo, since that is the leak the tool exists to prevent.

## redact

```
cargo run -p xtask -- redact recorded.json -o spider-cloud-agent/tests/fixtures/scrape.json
```

Rewrites a recorded API response into a fixture that can be published: every host outside
the allowlist becomes `example.com`, authorization headers, cookies and tokens are
replaced with `REDACTED`, anything shaped like an API key is replaced, and the output is
pretty printed. Without `-o` it writes to stdout. This is the only sanctioned way to add
a fixture. Run `leakcheck` on the result before committing it.

## Tests

```
cargo test -p xtask
```

The tests cover the denylist matcher, the address classifier, the fixture host
allowlist, the ASCII run detector and the redactor. A test filter that matches nothing
exits 0, so read the reported count rather than the exit status.
