# spider-agent media

Six recorded scenes of a calling program driving spider-agent 0.7.0. Four of
them put nothing on screen but machine-readable output: NDJSON records, a run
report, an exit code. The two that drive a coding agent CLI also show the
sentence the agent was given. The measured payload table is the one scene
written for a human eye.

## Re-record

```sh
bash media/render.sh                              three takes per scene
TAKES=5 bash media/render.sh                      five
SCENES="agent-calls-agent" bash media/render.sh   one scene
AGENT_TAKES=2 bash media/render.sh                two takes of the CLI scenes
```

The two scenes that drive a coding agent CLI run once per pass instead of three
times, because every take of those spends model tokens as well as credits.
`AGENT_TAKES` raises that on its own, and `TAKES` sets every scene at once. If
`claude` or `codex` is missing from PATH or signed out, render.sh prints why it
passed the scene over and leaves the standing recording alone.

It needs vhs, ffmpeg, jq, python3, Cargo, the Menlo font, and a release binary
at `target/release/spider-agent` reporting version 0.7.0. It puts that directory
first on PATH, so the older binary in `~/.local/bin` is never used. The five
live scenes use whatever key the CLI already has stored. No tape, argument or
environment dump carries a key, and no scene reads the account balance.

## Takes

The live scenes call the real service, so a single run says little. The first
call pays for DNS, a new TLS session and a cold path on the other side, and any
run can land behind a slow one. render.sh throws one warmup call away, then
records each scene several times and keeps the quickest take that did what the
scene claims.

A take is discarded when the scene exits nonzero, or when it served fewer pages
than the scene shows. A crawl that returned two pages instead of three is a
different scene, not a faster one. The scenes that time themselves are scored on
their own elapsed milliseconds, written to `out/<scene>.summary.json`. The local
payload table has nothing to time, so it is scored on how long its video runs.
Every take of this run stays under `out/takes/<scene>/<n>/` for comparison.

The run's best take only replaces what is already in `out/` if it beat it.
`out/<scene>.take.json` records the standing time, so an hour when the service
is slow cannot overwrite a good recording, and running render.sh again is always
worth it. That defence lasts only while the tape and the script behind it are
unchanged: edit either and the next run replaces the recording whatever the
clock says.

Each scene also records its own exit status into `out/<scene>.exit`, so no tape
types a check on screen.

## Encoding

vhs writes PNG frames here rather than video. Its own encoder calls ffmpeg with
flags that ffmpeg 9 rejects, and it fails without printing anything, so
`demo/encode.py` does the work: 12 fps and a 64 colour palette for the GIF,
libx264 and yuv420p for the MP4, and the last frame copied out as a still.

It also cuts the dead air. A tape sleeps a fixed stretch after Enter so a slow
run still fits, which leaves a frozen tail on a fast one, and that tail goes.
Any wait longer than 1.5 seconds is shortened to 1.5 seconds, and the two
coding agent scenes cut that to 0.6. Those two wait on a model, twelve seconds
and up with nothing landing on screen, so their gap holds no output to read.
The other four keep the 1.5 seconds they were recorded under.
Nothing between the first and last visible change is dropped, and the
millisecond stamps on screen are whatever that run measured.

The tapes share Menlo at 20, a dark theme, 24 fps capture and 70 ms typing, and
each one is only as tall as its output. The two coding agent scenes type at
10 ms throughout. Their command is the longest thing in the six scenes, and at
70 ms typing it ran to three quarters of the video. The commands also set the
tape width, 129 columns at the widest for Claude Code and 134 for Codex, since
a line that wraps inside a flag is worse than a wide frame. GIFs must stay
under 2,000,000 bytes.

## Scenes

### Agent calls agent

`tapes/agent-calls-agent.tape` runs `python3 -u media/demo/consume.py`, a
stdlib program that launches:

```sh
./target/release/spider-agent scrape https://example.com https://example.org \
  --ndjson --budget 2 --wall 10 --no-update --no-router --quiet
```

It reads stdout one line at a time. A nonempty page with a 2xx status goes
straight into its in-memory index, anything else is marked for review, and each
decision is flushed with the milliseconds since launch. It stops reading at the
`report` record, waits for the child and exits with the CLI's exit code. It
rounds `cost_credits` to six decimals for display. Needs a live key.

### Route and cost

`tapes/route-and-cost.tape` runs `bash media/demo/route-and-cost.sh`:

```sh
env -u SPIDER_API_KEY -u SPIDER_CLOUD_API_KEY \
  spider-agent route https://docs.python.org/3/_sources/tutorial/index.rst.txt \
  https://app.notion.so --ndjson --quiet | jq -c '{url,mode,source}'

spider-agent scrape https://example.com --json --budget 2 --wall 10 \
  --no-router --quiet | jq -c '...'
```

`route` reads no key, sends no request and spends nothing, and the invocation
unsets both key variables to prove it. The text source under docs.python.org
routes to `http` and the app URL routes to `smart`. That is the local heuristic
reading the shape of the address, not recognition of either site. The scrape
needs a live key, and its report carries the served and refused counts and what
the run cost. The jq filter rounds the credits to six decimals.

### Streaming crawl

`tapes/streaming-crawl.tape` runs `bash media/demo/streaming-crawl.sh`:

```sh
spider-agent crawl https://example.com https://httpbin.org/links/3/0 --limit 2 \
  --budget 2 --wall 10 --ndjson --no-router --quiet | python3 -u media/demo/consume.py --stdin
```

The limit applies per seed. The consumer prints real arrival times and adds no
delay of its own, so the gap between the first page and the rest is the crawl's
own timing. Pages that share one API response arrive together. `pipefail`
carries a failed CLI status through the pipe. Needs a live key.

### Claude Code calls agent

The visible line of `tapes/claude-calls-agent.tape` is the invocation itself,
typed at a bare prompt with no wrapper around it:

```sh
claude -p 'spider-agent scrape https://example.com --selectors fields.json --json --budget 2 --wall 20 --no-router.
Print 2 lines, no markdown: the command, then served/refused/cost_credits/elapsed_ms JSON.' --allowed-tools Bash --model sonnet
```

The task is two short sentences because typing it is most of the scene. The
first names the flags. Asked for the fields without them, Claude guessed at
`--documents`, watched it fail and then read the crate source for ten turns,
which is a true recording of something nobody wants to watch. The second makes
`claude -p` end on two lines rather than a paragraph with the command in a
fenced block. Even cut down the prompt runs past one line, so the tape presses
Enter inside the quotes and bash continues it at `>`. Nothing filters what comes
back.

Everything else the scene needs is hidden with vhs `Hide`. `demo/agent-scene.sh`
puts the agent in a scratch directory holding `fields.json`, starts a copy of
whatever is printed there, clears the screen, and hands the copy to
`demo/agent-scene.py` afterwards. That script parses the two lines back rather
than trusting them, so a take is kept only when the command was a `spider-agent
scrape` and the report it read says one page served and none refused. The take
is timed by `PS0` and `PROMPT_COMMAND`, from the keystroke that submits the
invocation to the moment it returns, which leaves out both the typing and the
fixed sleep the tape spends waiting.

Needs a signed-in `claude`, a live key, and the tokens the run costs.

### Codex calls agent

`tapes/codex-calls-agent.tape` types the same task at Codex, under the same
hidden plumbing:

```sh
codex exec -s workspace-write -c sandbox_workspace_write.network_access=true --skip-git-repo-check --json '<same task>' | jq -rs 'map(.item.text? // empty)[-1]'
```

The default sandbox is read only and blocks the network, which kills the scrape
with a budget error and zero wire bytes, so the recording asks for
`workspace-write` and turns network access on. This is the one scene with a
filter on screen. Codex has no quiet mode. It narrates every step and ends on a
usage banner, and `--json` turns that into an event per line, so one jq takes the
last agent message and lets the rest go by. There is no `< /dev/null` on the
line. Codex reads stdin only when it is a pipe or a file, and under vhs it is a
terminal.

Needs a signed-in `codex`, a live key, and the tokens the run costs.

### Payload savings

`tapes/payload-savings.tape` runs `python3 media/demo/payload-savings.py`,
which formats the numbers in `out/measured-numbers.txt`. render.sh regenerates
that file with:

```sh
cargo test -p spider-cloud-agent --test thrift_budget measured_numbers -- --ignored --nocapture
```

These are response bytes over a recorded product page fixture, not live
requests and not a credit comparison. Needs no key.

| Requested | Bytes | Saved |
| --- | ---: | ---: |
| Default | 9,021 | |
| Fields | 187 | 97.9% |
| Metadata | 350 | 96.1% |
| Links | 505 | 94.4% |
| Text | 2,156 | 76.1% |
| Markdown | 2,249 | 75.1% |
| HTML | 4,342 | 51.9% |

Every value matches the root README, and so does the crawl fixture: 5,261 bytes
to 2,269, and 1,333 approximate tokens to 509.

## What is committed

`out/*.gif`, `out/*.mp4` and `out/*.png` are in the repository: the standing take
of each scene, with `out/*.take.json` saying how fast it was and what recipe it
came from. The captured frames, the losing takes and the small text byproducts
are not.

As of 17 September 2026 the standing takes are 1,163 ms for the flagship, 266 ms
for route and cost, 2,632 ms for the crawl, and a 4.6 second video for the local
table. The two CLI scenes are timed on the caller, 14,787 ms for Claude Code and
13,281 ms for Codex, against the 239 and 308 ms the scrape itself took inside
them. Little of that reaches the video. The encoder cuts the wait down, so what
is left is mostly the command being typed, and those two GIFs run 4.8 and 5.9
seconds and weigh 32 KB and 42 KB. The other four run 4.6 to 6.4 seconds and weigh about
31 KB.
