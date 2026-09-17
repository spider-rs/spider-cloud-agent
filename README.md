# spider-cloud-agent

An agentic client for the [Spider Cloud](https://spider.cloud) API, in Rust.

It picks plain HTTP or a browser locally before spending a call, escalates when a
site refuses, stops on a budget you set, and returns only the fields you asked
for.

Three crates ship from this workspace:

- `spider-cloud-agent`, the client
- `spider-route`, the local rules that pick request settings
- `spider-agent-cli`, the `spider-agent` command line tool

## Install

```bash
curl -fsSL https://spider.cloud/install/spider-agent.sh | sh
spider-agent --version
```

The script downloads the release build for macOS or Linux on arm64 or x86_64,
checks it against the release's SHA256SUMS.txt, and installs it to the
XDG user bin directory in your home folder. Set `SPIDER_AGENT_INSTALL_DIR`
to install somewhere else, or `SPIDER_AGENT_VERSION` to pin a release. Its source is
[scripts/install.sh](scripts/install.sh).

Or, with a stable Rust toolchain, build it from crates.io:

```bash
cargo install spider-agent-cli
```

See [Cargo.toml](Cargo.toml) for the workspace's toolchain requirement.

## From another program

The tool is built to be called, not just typed. Results go to stdout,
diagnostics to stderr, and the exit code says which of auth, budget, a refusing
site, transport or output stopped the run.

```bash
# one page as JSON
spider-agent scrape https://example.com --json

# a site to NDJSON on disk, one object per line, flushed as each arrives
spider-agent crawl https://example.com --limit 50 --ndjson -o pages.ndjson

# named fields from addresses piped in, no page body over the wire
cat urls.txt | spider-agent extract --selectors fields.json --ndjson

# the command tree and record contract, so a caller can read the surface
spider-agent schema
```

`spider-agent route <url>` prints the transport it would choose without making a
call, spending nothing and needing no key.

## Provider fallback

Store an outside provider once and every command that fetches pages sends it as
the `router` parameter. With `--mode fallback` the provider is tried only when
Spider's own fetch fails. The key is read from stdin, never from an argument.

```bash
printf '%s' "$ZYTE_KEY" | spider-agent router set --provider zyte --mode fallback --funding own --token-stdin
spider-agent router show
```

The caller's own `router` always wins over the stored one. `--no-router`, or
`SPIDER_AGENT_NO_ROUTER` set to any value, skips it for one run, and
`spider-agent router clear` deletes it.

## From Rust

```toml
[dependencies]
spider-cloud-agent = "0.5"
```

```rust
use spider_cloud_agent::{Spider, Need};

let spider = Spider::new()?;

// Markdown, trimmed to fit a context window.
let page = spider.scrape("https://example.com")
    .need(Need::Markdown)
    .max_tokens(4000)
    .send()
    .await?;

// Or just the fields, and the page body never crosses the wire.
let fields = spider.scrape("https://example.com/product/1")
    .need(Need::fields([("price", ".price"), ("title", "h1")]))
    .send()
    .await?;

// The page and the links on it, in one call. `spider-agent --with-links` on
// the command line.
let page = spider.scrape("https://example.com")
    .need(Need::Markdown)
    .page_links(true)
    .send()
    .await?;
let found = page.links.as_deref().unwrap_or_default();
```

## What it does that a plain binding does not

**It chooses the settings.** Plain HTTP is many times cheaper and faster than a
browser, and enough for a lot of pages. Guessing wrong costs you money in one
direction and a failed fetch in the other. `spider-route` decides locally, in
microseconds, from the shape of the request.

**It keeps the two statuses apart.** The API reports how your call went and how
the target site answered, and they share a number. A 403 in the first means your
key is wrong. A 403 in the second means the site blocked you, and there is a
sequence of settings worth trying next.

**It returns less.** The API sends everything unless told otherwise. This crate
sends nothing you did not ask for.

Measured against a recorded 7.8KB product page, against the 9021 bytes you get
by asking for nothing in particular:

| asked for | bytes back | saved |
|---|---|---|
| `Need::fields(...)` | 187 | 97.9% |
| `Need::Metadata` | 350 | 96.1% |
| `Need::Links` | 505 | 94.4% |
| `Need::Text` | 2156 | 76.1% |
| `Need::Markdown` | 2249 | 75.1% |
| `Need::Html` | 4342 | 51.9% |

Regenerate these numbers with the ignored `measured_numbers` test:
`cargo test -p spider-cloud-agent --test thrift_budget measured_numbers -- --ignored --nocapture`,
which reads `spider-cloud-agent/tests/fixtures/thrift/product_page.json` for the
table and `crawl_widgets.json` in the same directory for the crawl below.

Across a six page crawl, dropping the navigation and footer that repeat on every
page takes 5261 bytes to 2269, and 1333 approximate tokens to 509.

Those are byte counts from running the request planner over recorded responses,
so they measure what this crate asks for and returns, not the service.

## How it routes

Deciding whether a page needs a browser takes no language model. `spider-route`
reads the shape of the URL and what happened the last time a similar request
went out, and answers from rules in microseconds. It reads the host once, for its
shape, and keeps no name.

To route another way, implement the `Router` trait.

## License

MIT. See [LICENSE](LICENSE).
