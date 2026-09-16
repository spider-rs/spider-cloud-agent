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
  update     install the newest release now
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

## Updates

A release binary keeps itself current. Once every 24 hours, a run starts a
detached `spider-agent update` and goes on with its own command without
waiting. The child's standard streams are closed, so the command's output,
exit code and timing are the same whether a check happened or not.

The child reads the newest release tag from the redirect that
`github.com/spider-rs/spider-cloud-agent/releases/latest` answers with. That
is the web page, not `api.github.com`, so it does not count against the API's
limit of 60 unauthenticated calls an hour. When the tag is newer than the
running binary, the child downloads `SHA256SUMS.txt` and the archive for this
platform from that release. It refuses the archive unless its SHA-256 matches
the line for it. Only then does it open the archive, take out `spider-agent`,
run it with `--version`, and require the tagged version back. It stages the
binary as `.spider-agent.update` beside the installed one and records its
digest in `~/.spider/update.json`, which is mode 0600 like the credentials file.

The next run hashes the staged file. If the digest matches the recorded one,
it renames the file over the binary and starts the new binary in its place
with the same arguments, so that run already uses the new version. A staged
file that no check recorded, or whose bytes changed since, gets deleted and
never runs. The one line this prints goes to stderr, and `--quiet` silences it.

Nothing here can fail your command. An outage, a rate limit or a missing
release is a silent skip until the next day's check. An archive that failed
its checksum, or would not open, gets one line on stderr on the next run, even
under `--quiet`, and nothing is installed.

Some installs are left alone, with one note on stderr per release saying how
to update:

- A binary under a `cargo install` root, one with `.crates.toml` or
  `.crates2.json` next to its `bin`. Replacing it would leave cargo's record
  claiming the old version. Run `cargo install spider-agent-cli --locked`.
- A binary under a Homebrew `Cellar` or `/nix/store`.
- A binary in a directory you cannot write, such as `/usr/local/bin` owned by
  root.

`spider-agent update` does all of this in the foreground and installs at once.
It writes nothing to stdout. It exits 0 when it installed a release or the
binary is already the newest, 1 when the release was refused or the install
failed, 2 when updates are turned off, 6 when the release host could not be
reached or is limiting requests, and 7 when the install is one of those left
alone.

Set `SPIDER_AGENT_NO_UPDATE` to any non-empty value, or pass `--no-update`,
to turn all of it off. Then no check runs, nothing downloads, a file staged
earlier stays uninstalled, and `update` exits 2. Put `--no-update` after the
command name, like every other flag. Debug builds never update themselves.

### What a release must contain

The updater builds every URL from the tag, so a release has to use these
names exactly:

- The tag is `v` followed by three numbers, such as `v0.4.1`, marked as the
  latest release. A tag with a suffix such as `-rc.1` is never picked up.
- One archive per platform, named `spider-agent-<version>-<target>.tar.gz`,
  for the targets `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `aarch64-unknown-linux-gnu` and `x86_64-unknown-linux-gnu`. Other platforms
  do not update themselves.
- Each archive holds a regular file named `spider-agent` at its root, which
  prints `spider-agent <version>` for `--version`. The updater ignores other
  entries. On macOS, build the archive with `COPYFILE_DISABLE=1` to keep the
  `._spider-agent` entry out.
- `SHA256SUMS.txt` in the same release, as `shasum -a 256` writes it, with one
  line for every archive.

Every machine that checks refuses a release with no checksum line for its
archive, or whose binary answers `--version` with another version.

## Environment

| variable | effect |
|---|---|
| `SPIDER_API_KEY` | First environment source for the API key. Empty values after trimming are skipped. |
| `SPIDER_CLOUD_API_KEY` | Fallback API key when `SPIDER_API_KEY` is empty or unset. |
| `SPIDER_API_URL` | Overrides the API base, normally `https://api.spider.cloud`, for every key-bearing API request. The API base must use https. |
| `SPIDER_AGENT_NO_UPDATE` | Any non-empty value turns off the update check, the download and the install of a staged update. The same as `--no-update`. |
| `SPIDER_MCP_SERVER` | Overrides the sign-in discovery server, normally `https://mcp.spider.cloud/mcp`, when OAuth is enabled. |

Set either server override only to a server you trust: one receives API requests
with your key, and the other directs sign-in.

## License

MIT. See [LICENSE](LICENSE).
