# spider-cloud-agent

An agentic client for the [Spider Cloud](https://spider.cloud) API, in Rust.

It picks plain HTTP or a browser locally before spending a call, escalates when a
site refuses, stops on a budget you set, and returns only the fields you asked
for.

Three crates ship from this workspace:

- `spider-cloud-agent`, the client
- `spider-route`, a local model that picks request settings
- `spider-agent-cli`, the `spider-agent` command line tool

## From another program

The tool is built to be called, not just typed. Results go to stdout,
diagnostics to stderr, and the exit code says which of auth, budget, a refusing
site, transport or output stopped the run.

```bash
cargo install spider-agent-cli

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

## From Rust

```toml
[dependencies]
spider-cloud-agent = "0.3"
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

Across a six page crawl, dropping the navigation and footer that repeat on every
page takes 5261 bytes to 2269, and 1333 approximate tokens to 509.

Those are byte counts from running the request planner over recorded responses,
so they measure what this crate asks for and returns, not the service.

## The routing model

Deciding whether a page needs a browser takes no language understanding. It
takes a classifier over the shape of a URL and what happened last time something
like it was fetched. That fits in about a megabyte and answers in microseconds,
so it can run on every request without anyone weighing the cost.

The model never sees a domain name. Every input is a bucketed, enumerated
feature, and the full feature to weight table ships with the weights. That is a
property of the format rather than a promise: the artifact has no string field
in it, so a domain cannot be stored there.

## License

MIT. See [LICENSE](LICENSE).
