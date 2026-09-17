#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
trap 'status=$?; echo $status > "$PWD/media/out/streaming-crawl.exit"; exit $status' EXIT
export SPIDER_AGENT_NO_UPDATE=1 NO_COLOR=1
export PATH="$PWD/target/release:$PATH"
printf '%s\n' '{"tool":"crawl","limit_per_seed":2,"budget":2,"format":"ndjson"}'
spider-agent crawl https://example.com https://httpbin.org/links/3/0 --limit 2 --budget 2 \
  --wall 20 --ndjson --no-router --quiet | python3 -u media/demo/consume.py --stdin
printf '%s\n' '{"exit":0}'
