# spider-agent

The command line tool for [Spider Cloud](https://spider.cloud), built on
`spider-cloud-agent`. It picks transport locally before a call goes out,
escalates on the status a site returned, and stops on a budget you set.

## Install

Install with a stable Rust toolchain; see the workspace's
[Cargo.toml](../Cargo.toml) for the toolchain requirement.

```bash
cargo install spider-agent-cli
spider-agent https://example.com
```

Clap and its dependency tree live here rather than in the library, so a program
that embeds `spider-cloud-agent` carries none of it.

## It is built to be called by another program

Results go to stdout, diagnostics go to stderr, and nothing writes an escape
code, prompts, or draws a spinner. There is no flag for the key, because an
argument is readable in the process list.

`--ndjson` writes one object per line and flushes each one, so a caller reads
the first page of a crawl while the last is still being fetched. The last line
is always a `report`, which says what the run served, refused, and cost.
`--json` writes one document instead, `{"items": [...], "report": {...}}`, and
buffers until the run ends. The shape does not change with how many results
came back. `spider-agent schema` prints the whole record contract as JSON.

Exit codes:

| code | meaning |
|---|---|
| 0 | done |
| 1 | failed, and none of the others describes it |
| 2 | usage, or an input that could not be read |
| 3 | auth: no key, a refused key, or a balance of zero |
| 4 | budget: a cap stopped the run |
| 5 | the site refused every attempt |
| 6 | transport: the call never reached the service, or it failed |
| 7 | output: a destination could not be written |

A site that refused is code 5 and a key that was refused is code 3, which is
the same split the library makes between the two status planes. Retrying the
first from another address can work. Retrying the second buys the same refusal.
`transform` uses code 1 when conversion attempts produce no usable document,
because it works on supplied markup and fetches no site.

## Three invocations worth copying

One page as JSON:

```bash
spider-agent scrape https://example.com --goal markdown --json | jq -r '.items[0].body'
```

A site to NDJSON on disk, streamed as it arrives:

```bash
spider-agent crawl https://example.com --limit 50 --ndjson -o pages.ndjson --budget 25
```

Named fields off a list of addresses piped in:

```bash
cat urls.txt | spider-agent extract --selectors fields.json --ndjson
```

where `fields.json` is `{"price": ".price", "title": ["h1", ".product-title"]}`.
Several selectors under one name are tried in order. With fields, the page body
never crosses the wire.

## Commands

```
spider-agent <url>            scrape, which is what you get with no command
  scrape     one page or a list of them
  fetch      one path under the config the service holds for it
  crawl      a site, following its links
  extract    named fields, and no page bytes
  links      the links on a page
  search     a query
  screenshot a picture, into a file
  transform  convert markup you already hold, fetching nothing
  run        work a goal over addresses until a cap stops it
  credits    the balance
  logs       the record of past crawls
  sites      the sites configured on the account
  keys       the API keys on the account, as metadata
  profile    the account: plan limits, totals and billing caps
  login      sign in through a browser and store the key
  route      what transport would be chosen. Local, no call, no spend
  schema     the command tree and the record contract, as JSON
```

Scrape and fetch are two endpoints, not two spellings. Scrape sends the address
you named and the settings you chose. Fetch names a host and a path, and the
service answers with a config it already worked out for that path: which fields
to pull, whether the page needs a browser, what to wait for. The first fetch on
a path nobody has asked for yet goes away and works one out, which is slow and
can fail while it tries, and the query string is not part of the target. Reach
for scrape unless you want the stored config.

## The account reads

`credits`, `logs`, `sites`, `keys` and `profile` read your own records. None of
them fetches a page, so none of them costs credits. All but `credits` write
NDJSON rows, and the columns belong to the service rather than to this tool, so
a row is passed through as it arrived.

`keys` returns metadata and no key. There is no token, secret or hash column in
that reply, and `token_name` is the label you gave a key rather than any part of
it. It is the read for finding a key nothing has used in months. Revoking one is
done on the dashboard.

The `table <name>` command is gone as of 0.3.0. It read any name under `/data`,
which let a caller ask the service for tables nobody meant to expose. The
commands above cover the same ground with a fixed set of names. The library
still has `Spider::table` for a table this version has no command for.

## What run does that scrape does not

`run` takes a goal and works it. Transport for the first attempt is decided
locally from the shape of the address. A refusal is answered by the escalation
ladder, from the status the site returned rather than from a retry count. What
the run learns about a host is carried to the next address on that host, so the
second page does not pay for the cheap attempt again. The cap is checked
against what is left before every page. With `--expand`, the links off the
pages already read become the frontier, same host only.

```bash
spider-agent run https://example.com --goal markdown --expand 20 --budget 40 --ndjson
```

No large model is called anywhere in this. The decisions above are rules,
a local classifier over the shape of a URL, and arithmetic against a budget.

## Run from a plan file

`spider-agent run --plan plan.json --ndjson` reads a JSON object such as:

```json
{
  "goal": "fields",
  "urls": ["https://example.com/", "https://example.com/products"],
  "expand": 20,
  "max_pages": 50,
  "budget": 10,
  "selectors": {
    "price": ".price",
    "title": ["h1", ".product-title"]
  }
}
```

All six keys are optional. `goal` takes the same names as `run --goal` and
otherwise defaults to `markdown`. `urls` is a list of address strings.
`expand` and `max_pages` are nonnegative integers; expansion defaults to zero
and the page count has no default cap. `budget` is a finite, nonnegative number
of credits, not a quoted string. `selectors` is a nonempty object mapping field
names to a selector string or a list of strings, and implies the fields goal.
Unknown keys and invalid values stop with exit 2 before a request goes out,
even when a flag would replace that value.

Explicit command line flags win over the matching plan values, including
`--goal markdown` and `--expand 0`. `--selectors` replaces the plan's selector
map. Addresses accumulate in order: positional URLs, plan URLs, then
`--urls-from`. With none of these, `run` reads addresses from stdin.
`--plan -` reads the JSON from stdin. A leading UTF-8 byte order mark is accepted.

`spider-agent schema` includes the plan shape under `plan`. Its `command_tree`
comes from clap and includes the bare invocation, subcommands and inherited
flags. `commands` holds the same subcommand map. Each argument names its long
and short flags or positional index, arity (`max: null` means unbounded), value
type, defaults, possible values, required status, conflicts and requirements.
Conflicts and requirements refer to argument IDs in that command. `command_notes`
keeps the command descriptions, and `exit_codes` maps numbers to the labels used
in error records.

## Where results go

`-o FILE` writes one file. `-d DIR` writes one file per page, named from the
address, so a second run over the same list produces the same names and the two
runs diff. `--append` adds to an NDJSON file. A file that is already there
stops the run before anything is fetched, unless you pass `--force`. Parent
directories are created only with `--mkdir`. A name inside a directory is built
from the host and path with everything outside `a-z0-9-` replaced, so it
carries no separator and stays in the directory you named.

## Where the key comes from

The key comes from SPIDER_API_KEY, then SPIDER_CLOUD_API_KEY, then the keychain,
when the binary was built with the keyring feature, then ~/.spider/credentials.
The first non-empty value after trimming wins. There is no flag for the key,
because an argument is readable in the process list.

The CLI enables `oauth` by default and leaves `keyring` off. `spider-agent login`
tries the keyring when enabled, then falls back to the credentials file. On Unix,
the file is created at mode 0600; reading a file with broader permissions logs a
warning once, and `Credentials::tighten_file_permissions` can narrow its mode.
The permission check and tightening are Unix only. On Windows, the file uses
the default ACL; the keyring is tried first if that feature is enabled.
The key is never logged. `spider-agent login --print` writes it to stdout,
including in a key record with `--json`, and does not store it.

## Environment

| variable | effect |
|---|---|
| `SPIDER_API_KEY` | First environment source for the API key. Empty values after trimming are skipped. |
| `SPIDER_CLOUD_API_KEY` | Fallback API key when `SPIDER_API_KEY` is empty or unset. |
| `SPIDER_API_URL` | Overrides the API base, normally `https://api.spider.cloud`, for every key-bearing API request. The API base must use https. |
| `SPIDER_MCP_SERVER` | Overrides the sign-in discovery server, normally `https://mcp.spider.cloud/mcp`, when OAuth is enabled. |

Set either server override only to a server you trust: one receives API requests
with your key, and the other directs sign-in.

## License

MIT. See [LICENSE](LICENSE).
