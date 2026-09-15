# spider-agent

The command line tool for [Spider Cloud](https://spider.cloud), built on
`spider-cloud-agent`. It picks transport locally before a call goes out,
escalates on the status a site returned, and stops on a budget you set.

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

Up to 0.2.0 there was a `table <name>` command that read any name under
`/data`. The commands above replaced it, because a tool that takes a table name
is a way to ask the service for tables nobody meant to expose. `Spider::table`
in the library is still there for a table this version has no command for.

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

## Where results go

`-o FILE` writes one file. `-d DIR` writes one file per page, named from the
address, so a second run over the same list produces the same names and the two
runs diff. `--append` adds to an NDJSON file. A file that is already there
stops the run before anything is fetched, unless you pass `--force`. Parent
directories are created only with `--mkdir`. A name inside a directory is built
from the host and path with everything outside `a-z0-9-` replaced, so it
carries no separator and stays in the directory you named.

## Where the key comes from

`SPIDER_API_KEY`, then `SPIDER_CLOUD_API_KEY`, then `~/.spider/credentials`,
which `spider-agent login` writes at mode 0600. The key is never printed, never
logged and never put in a record. `spider-agent login --print` is the one thing
that writes it anywhere, and it writes it to stdout and nowhere else.

## License

MIT. See [LICENSE](LICENSE).
