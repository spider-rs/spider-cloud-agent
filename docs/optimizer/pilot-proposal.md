# Pilot proposal: the first paired collection

This is a proposal. Nothing in it has run. Nothing runs until someone with
authority over the account approves the hard cap below and runs the command
themselves. Every number marked "assumed" is a planning figure, not a price.

## What the pilot decides

The pilot answers one question. Is the collection and labelling machinery sound
enough on real pages to justify the 5,000 pair collection? It answers through five
measurements. The decision rule is at the end.

1. The pair relative labels behave. On repeated baseline pairs the shingle
   Jaccard should sit near one. Its fifth percentile sets `tau`.
2. The event tracker returns resource maps often enough that a useful share of
   sites has blacklist candidates.
3. The real credit price of each arm. Measured prices can then replace the
   multiplier table in `rollout.md`.
4. Every learnable edit code gets rows with both outcomes. The validator's per
   code rule can then be met at scale.
5. Retry and error rates stay low enough that a longer run would not burn the
   budget on failures.

The pilot cannot train a usable model. Two hundred pairs is below the gate's
300 pair minimum. It is also far below the 200 covered rows an edit code needs for
a floor. Every code will abstain, and that is expected.

## Requests

| Item | Count |
| --- | --- |
| Distinct URLs | 120, on at least 60 registrable domains |
| Baseline runs | 120 |
| Candidate runs | 200, round robin over the configurations below |
| Baseline repeats for `tau` | 10 (about five percent) |
| Requests in total | 330 |
| Pairs written | 200, plus 10 repeat pairs |

The operator picks the URLs from public pages that need no sign in. The picks
follow these strata:

- by need: markdown 60, text 20, fields 20, links 20. The fields pages come with a
  selector file naming two to four fields per page.
- by extension class: none, markup and other, in roughly equal thirds.
- at least a fifth of the domains are known to serve a block or an empty page to
  plain HTTP. That gives the residential and browser arms something to fix.

No URL comes from Spider's own sites. The list goes in
`training/data/pilot-1/urls.txt`, which git ignores, and its sha256 goes into
the manifest.

## Candidate configurations

`training/data/pilot-1/candidates.json` has seven entries. Each is a single edit,
so its effect can be attributed to it.

| Code | Edit | Why it is in the pilot |
| --- | --- | --- |
| request | `request = http` | The cheapest arm; measures the base price |
| request | `request = browser` | The heaviest common arm |
| wait_for | idle network 5,000 ms | The wait effect the fixture plants |
| proxy | `proxy = residential` | The residential effect, and the reversal risk |
| block_stylesheets | `false` | The content changing switch |
| full_resources | `true` | The dearest resource setting |
| network_blacklist | append the top observed third party | Only where a baseline observation exists, with `disable_hints` on both arms |

The extraction and trimming edits stay out of the pilot. They change content,
and their label pipeline has not yet run on real pages.

## Credits

The assumed base unit is what one HTTP page costs in credits. The multiplier
table in `rollout.md` prices each candidate. The baseline is the router's own
choice, assumed at 1.5 on average.

| Arm mix | Assumed base units |
| --- | --- |
| 120 baselines at 1.5 | 180 |
| 200 candidates at an average of 3.4 | 680 |
| 10 repeats at 1.5 | 15 |
| Event tracker maps on every baseline, assumed 0.2 | 24 |
| Retries and escalation, ten percent | 90 |
| Estimated total | about 990 base units |

The service prices in credits. A live check with the collector itself ran on
2026-09-17. It covered three public pages and six pairs, and spent 1.51 credits in
total. It measured 0.11 to 0.23 credits per page in `smart` mode, and 0.11 to 0.19
for the same pages over plain HTTP. The base unit is therefore about 0.1 to 0.2
credits. The estimate above comes to roughly 100 to 200 credits, or about 0.02 USD
at the client's 10,000 credits per dollar.

Heavier pages, rendered arms and the residential pool will cost more. The hard
cap keeps a wide margin at 2,500 credits, or 0.25 USD, passed as
`--max-credits 2500`. The collector copies the cap into every request's budget.
It stops with exit 3 as soon as the running total reaches the cap. The cap holds
even if the estimate is off by an order of magnitude.

## Concurrency and timing

Concurrency is one. The collector sends one request at a time. A URL's arms go
out back to back, so a site sees its baseline and candidate close together. This
is deliberate for a pilot, where the order of arms matters more than speed. Each
request keeps the client's own timeouts and its 15 minute wall per walk. The shell
bounds the whole run with `timeout 45m`. At about five seconds a request, the run
should take 25 to 35 minutes.

## Stop conditions

The run stops itself, and the operator stops it, on any of these:

- the credit cap, which exits with 3.
- any 401 or 402 from the service. These point at the key or the account, not the
  pages.
- more than ten percent of requests ending in transport errors or timeouts over
  any 30 consecutive requests.
- fewer than half of the first 40 baseline responses carrying a resource map when
  the event tracker was asked for. Blacklist candidates would then have no input.
  The run can continue without them, but the operator decides.
- a median shingle Jaccard under 0.8 on the first five repeat pairs. The content
  label is then not behaving, and nothing after it can be interpreted.

## Command

```bash
cd optimize-collect
SPIDER_SERVICE_REVISION=<from the deployment serving the URL> \
timeout 45m cargo run --release -p optimize-collect -- \
  --urls ../training/data/pilot-1/urls.txt \
  --candidates ../training/data/pilot-1/candidates.json \
  --out ../training/data/pilot-1 \
  --salt-file ../training/data/pilot-1/salt.bin \
  --repeat-share 0.05 \
  --service-revision "$SPIDER_SERVICE_REVISION" \
  --spend --max-credits 2500
```

The key is read from the usual places and never printed. Afterwards:

```bash
cd training
uv run spider-optimize-train validate data/pilot-1
uv run spider-optimize-train train data/pilot-1 --out data/pilot-1-run --seed 1
uv run spider-optimize-train thresholds data/pilot-1 --run data/pilot-1-run
uv run spider-optimize-train evaluate data/pilot-1 --run data/pilot-1-run
uv run spider-optimize-train gates data/pilot-1 --run data/pilot-1-run
```

If any code has only one outcome, `validate` will name the per code rule. That
is a finding, not a failure of the pilot. `gates` will report insufficient. The
manifest records the real spend, the collector, client and service revisions, and
the chosen `tau`.

## The decision rule

Proceed to the 5,000 pair collection when all of these hold:

- repeat pairs have a median Jaccard of at least 0.9, and no repeat pair is under
  0.5.
- at least 60 percent of baseline rows carry a resource map.
- every edit code has at least ten candidate rows with both outcomes.
- transport errors and timeouts stay under five percent.
- the measured mean pair price times 5,000 fits the budget the next
  authorisation names.

Otherwise the pilot's findings go back into the collector or the label
definitions. A second pilot of the same size then runs before anything larger.
Either way, the pilot leads to no learned edit in production. After a passing
run, the first step is shadow mode with the pilot's own model.
