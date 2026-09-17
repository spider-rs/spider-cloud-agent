# Pilot proposal: the first paired collection

This is a proposal. Nothing in it has run, and nothing runs until someone with
authority over the account approves the hard cap below and executes the command
themselves. Every number marked "assumed" is a planning figure, not a price.

## What the pilot decides

One question: is the collection and labelling machinery sound enough on real
pages to justify the 5,000 pair collection? The pilot answers it through five
measurements, and the decision rule is at the end.

1. The pair relative labels behave. On repeated baseline pairs the shingle
   Jaccard should sit near one; its fifth percentile sets `tau`.
2. The event tracker returns resource maps often enough that blacklist
   candidates exist for a useful share of sites.
3. The real credit price of each arm, so the multiplier table in `rollout.md`
   can be replaced by measured prices.
4. Every learnable edit code gets rows with both outcomes, so the validator's
   per code rule can be met at scale.
5. Retry and error rates stay low enough that a longer run would not burn the
   budget on failures.

The pilot cannot train a usable model. Two hundred pairs is below the gate's
300 pair minimum and far below the 200 covered rows an edit code needs for a
floor, so every code will abstain, and that is expected.

## Requests

| Item | Count |
| --- | --- |
| Distinct URLs | 120, on at least 60 registrable domains |
| Baseline runs | 120 |
| Candidate runs | 200, round robin over the configurations below |
| Baseline repeats for `tau` | 10 (about five percent) |
| Requests in total | 330 |
| Pairs written | 200, plus 10 repeat pairs |

URL selection is the operator's, from public pages that need no sign in,
stratified as: markdown 60, text 20, fields 20 (with a selector file naming two
to four fields per page), links 20; extension classes none, markup and other
in roughly equal thirds; and at least a fifth of the domains known to serve a
block or an empty page to plain HTTP, so the residential and browser arms have
something to fix. No URL from the fleet's own sites. The list is committed to
`training/data/pilot-1/urls.txt`, which is gitignored, and its sha256 goes into
the manifest.

## Candidate configurations

The seven entries of `training/data/pilot-1/candidates.json`, each a single edit
so its effect is attributable:

| Code | Edit | Why it is in the pilot |
| --- | --- | --- |
| request | `request = http` | The cheapest arm; measures the base price |
| request | `request = browser` | The heaviest common arm |
| wait_for | idle network 5,000 ms | The wait effect the fixture plants |
| proxy | `proxy = residential` | The residential effect, and the reversal risk |
| block_stylesheets | `false` | The content changing switch |
| full_resources | `true` | The dearest resource setting |
| network_blacklist | append the top observed third party | Only where a baseline observation exists, with `disable_hints` on both arms |

The extraction and trimming edits stay out of the pilot: they change content
and the label pipeline for them is not yet exercised on real pages.

## Credits

The assumed base unit is the credits one HTTP page costs. Each candidate is
priced by the multiplier table in `rollout.md`; the baseline is the router's
own choice, assumed 1.5 on average.

| Arm mix | Assumed base units |
| --- | --- |
| 120 baselines at 1.5 | 180 |
| 200 candidates at an average of 3.4 | 680 |
| 10 repeats at 1.5 | 15 |
| Event tracker maps on every baseline, assumed 0.2 | 24 |
| Retries and escalation, ten percent | 90 |
| Estimated total | about 990 base units |

The service prices in credits, and the base unit is not known until the first
responses arrive. The run therefore starts with a preflight: the collector's
first ten baseline pages are read for `Costs.total_cost`, and the operator
converts the estimate above before letting the run continue. As a hard number
to authorise before that reading exists, the cap is 25,000 credits, which is
2.50 USD at the client's 10,000 credits per dollar, and it is passed as
`--max-credits 25000`. The collector mirrors it into every request's budget and
stops with exit 3 the moment the running total reaches it, so the cap holds
even if the estimate is wrong by an order of magnitude. If the preflight shows
the base unit above 10 credits, stop and re-plan; the estimate would then exceed
a third of the cap.

## Concurrency and timing

Concurrency one: the collector sends one request at a time, and a URL's arms go
out back to back so a site sees its baseline and candidate close together. That
is deliberate for a pilot, where the ordering of arms matters more than speed.
Each request keeps the client's own timeouts and its 15 minute wall per walk;
the whole run is bounded from the shell with `timeout 45m`. Expected duration
25 to 35 minutes at about five seconds a request.

## Stop conditions

The run stops itself, and the operator stops it, on any of:

- the credit cap (exit 3);
- any 401 or 402 from the service (the key or the account, not the pages);
- more than ten percent of requests ending in transport errors or timeouts
  over any 30 consecutive requests;
- fewer than half of the first 40 baseline responses carrying a resource map
  when the event tracker was asked for (blacklist candidates would have no
  input; the run continues without them but the operator decides);
- the median shingle Jaccard of the first five repeat pairs under 0.8 (the
  content label is not behaving and nothing after it is interpretable).

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
  --spend --max-credits 25000
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

`validate` will name the per code rule if any code has one outcome; that is a
finding, not a failure of the pilot. `gates` will report insufficient. The
manifest records the real spend, the collector, client and service revisions,
and the chosen `tau`.

## The decision rule

Proceed to the 5,000 pair collection when all of these hold:

- repeat pairs have a median Jaccard of at least 0.9 and no repeat pair under
  0.5;
- at least 60 percent of baseline rows carry a resource map;
- every edit code has at least ten candidate rows with both outcomes;
- transport errors and timeouts under five percent;
- the measured mean pair price times 5,000 fits the budget the next
  authorisation names.

Otherwise the pilot's findings go back into the collector or the label
definitions, and a second pilot of the same size runs before anything larger.
Either way no learned edit is applied in production as a result of the pilot;
shadow mode with the pilot's own model is the first step after a passing run.
