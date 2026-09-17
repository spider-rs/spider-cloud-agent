# spider-agent

The command line tool for [Spider Cloud](https://spider.cloud), built on
`spider-cloud-agent` and meant to be run by an agent rather than typed by a
person. It picks transport locally before a call goes out, escalates on the
status a site returned, and stops on a budget you set. Output is JSON, NDJSON or
the page content itself, and `spider-agent schema` hands a caller the whole
command tree and record contract as JSON.

## Install

```bash
curl -fsSL https://spider.cloud/install/spider-agent.sh | sh
spider-agent https://example.com
```

The script downloads the release build for macOS or Linux on arm64 or x86_64,
checks it against the release's SHA256SUMS.txt, and installs it to the
XDG user bin directory in your home folder. Set `SPIDER_AGENT_INSTALL_DIR`
to install somewhere else, or `SPIDER_AGENT_VERSION` to pin a release.

Or, with a stable Rust toolchain, build it from crates.io:

```bash
cargo install spider-agent-cli
```

See the workspace's [Cargo.toml](../Cargo.toml) for the toolchain requirement.

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
  router     store, show or clear a provider fallback every page request carries
  optimize   store, show or clear how the parameter optimizer runs
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

## Provider fallback

`spider-agent router set` stores the `router` request parameter and
`provider_options` in `~/.spider/router.json`, readable by the owner only. Every
command that fetches pages sends it, unless the request names a router of its own.
The command then prints
`using stored router: provider zyte, mode fallback, funding own` once on stderr.

```bash
printf '%s' "$ZYTE_KEY" | spider-agent router set --provider zyte --mode fallback --funding own --token-stdin
printf '%s' "$OXYLABS_PASSWORD" | spider-agent router set --provider oxylabs --mode fallback \
  --credential oxylabs_username=NAME --credential oxylabs_password-stdin
spider-agent router set --option zyte.geolocation=US
spider-agent router show
spider-agent router clear
```

`set` merges into what is stored, so a flag you leave out keeps its value.
`--no-token`, `--no-credential NAME` and `--no-option PROVIDER.KEY` each remove one
value. Keys come from stdin or `SPIDER_ROUTER_TOKEN`. `--token VALUE` is refused,
because a key on the command line lands in shell history. For the same reason, a
`--credential` whose name says it is a password, secret, token or key is refused.
`show` prints every credential and setting value as `<redacted>`.

`--no-router`, or `SPIDER_AGENT_NO_ROUTER` set to any non-empty value, sends no stored
router for one run. If the router file cannot be read or does not validate, a page
command stops with code 2 before any call.

## The parameter optimizer

The optimizer is a second decision layer. After the router picks the mode, the
pool and the wait, it lists the valid edits to the request, scores each one
against leaving the request alone, and writes at most one set that clears the
gate. It never edits a field you set, it only touches the first attempt, and a
request that fixes the mode, the pool or the country is not passed to it.

It is off until you ask for it.

```bash
spider-agent optimize set --mode shadow --model ~/.spider/weights.bin
spider-agent optimize show
spider-agent scrape https://example.com --optimize apply
spider-agent scrape https://example.com --no-optimize
spider-agent optimize clear
```

`shadow` scores and records, and sends the request unchanged. `apply` writes the
pick onto the fields you left unset. `off` is a run with no optimizer at all.
The settings live in `~/.spider/optimize.json`, readable by the owner only, and
hold a mode and the path to a weights file. Neither is a key.

Without weights the scorer abstains, so `apply` edits nothing and the run says
so once on stderr. With them, the line names the version and the file:
`optimizer apply: weights version 1 from /path/weights.bin`. A weights file that
cannot be read or does not validate stops the run with code 2 before any call,
and so does a settings file that cannot be used.

`--optimize` and `--optimize-model` override the stored settings for one run.
`--no-optimize`, or `SPIDER_AGENT_NO_OPTIMIZE` set to any non-empty value, leaves
the optimizer out whatever is stored. A binary built with
`--no-default-features` has no optimizer, and asking one for `shadow` or `apply`
fails with code 2.

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
running binary, the child downloads `SHA256SUMS.txt` and
`SHA256SUMS.txt.minisig` from that release. It reads nothing in the sums file
until the minisign signature verifies against a release key built into the
binary, and the signed trusted comment names that exact version. Then it
downloads the archive for this platform and refuses it unless its SHA-256
matches the line for it. Only then does it open the archive, take out `spider-agent`,
run it with `--version`, and require the tagged version back. It stages the
binary as `.spider-agent.update` beside the installed one and records its
digest in `~/.spider/update.json`, which is mode 0600 like the credentials file.

The next run hashes the staged file. If the digest matches the recorded one,
it renames the file over the binary and starts the new binary in its place
with the same arguments, so that run already uses the new version. A staged
file that no check recorded, or whose bytes changed since, gets deleted and
never runs. The one line this prints goes to stderr, and `--quiet` silences it.

Nothing here can fail your command. An outage, a rate limit or a missing
release is a silent skip until the next day's check. A release with a missing
or bad signature, or an archive that failed its checksum or would not open,
gets one line on stderr on the next run, even under `--quiet`, and nothing is
installed.

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

When `CI` is set to a non-empty value, as most CI services set it, runs start
no check and leave a staged update uninstalled, so a pipeline never swaps its
own binary partway through a job. `spider-agent update` still works there, for
a step that updates on purpose.

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
- `SHA256SUMS.txt.minisig`, the minisign signature over that file, whose
  trusted comment is exactly `spider-agent v<version> SHA256SUMS.txt`, as in
  `spider-agent v0.4.1 SHA256SUMS.txt`.

Every machine that checks refuses a release with no signature, a signature by
any other key, a comment naming another version, no checksum line for its
archive, or a binary that answers `--version` with another version.

Releases are signed with the minisign key `43EBB605B452BC13`:

```
untrusted comment: minisign public key 43EBB605B452BC13
RWQTvFK0BbbrQ5Fi5QICN6wkM5EsCGlfeLsTSiY9hTMdZpUjGaAg5t13
```

The binary pins it in `src/update/release-keys.pub`, and nothing at run time
can change that list. To check a download by hand:

```bash
minisign -Vm SHA256SUMS.txt -P RWQTvFK0BbbrQ5Fi5QICN6wkM5EsCGlfeLsTSiY9hTMdZpUjGaAg5t13
shasum -a 256 -c --ignore-missing SHA256SUMS.txt
```

The first command prints the trusted comment. Check that it names the version
you downloaded.

## Environment

| variable | effect |
|---|---|
| `SPIDER_API_KEY` | First environment source for the API key. Empty values after trimming are skipped. |
| `SPIDER_CLOUD_API_KEY` | Fallback API key when `SPIDER_API_KEY` is empty or unset. |
| `SPIDER_API_URL` | Overrides the API base, normally `https://api.spider.cloud`, for every key-bearing API request. The API base must use https. |
| `SPIDER_AGENT_NO_UPDATE` | Any non-empty value turns off the update check, the download and the install of a staged update. The same as `--no-update`. |
| `SPIDER_AGENT_NO_ROUTER` | Any non-empty value sends no stored provider fallback on this run. The same as `--no-router`. |
| `SPIDER_ROUTER_TOKEN` | The provider token `router set` stores, instead of `--token-stdin`. |
| `SPIDER_MCP_SERVER` | Overrides the sign-in discovery server, normally `https://mcp.spider.cloud/mcp`, when OAuth is enabled. |

Set either server override only to a server you trust: one receives API requests
with your key, and the other directs sign-in.

## License

MIT. See [LICENSE](LICENSE).
