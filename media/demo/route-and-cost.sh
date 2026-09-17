#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
trap 'status=$?; echo $status > "$PWD/media/out/route-and-cost.exit"; exit $status' EXIT
export SPIDER_AGENT_NO_UPDATE=1 NO_COLOR=1
export PATH="$PWD/target/release:$PATH"
started=$(python3 -c 'import time; print(int(time.time() * 1000))')
printf '%s\n' '{"tool":"route","network":false,"key_required":false,"cost_credits":0}'
env -u SPIDER_API_KEY -u SPIDER_CLOUD_API_KEY \
  spider-agent route https://docs.python.org/3/_sources/tutorial/index.rst.txt \
  https://app.notion.so --ndjson --quiet | jq -c '{url,mode,source}'
printf '%s\n' '{"tool":"scrape","url":"https://example.com","format":"json"}'
spider-agent scrape https://example.com --json --budget 2 --wall 10 \
  --no-router --quiet | jq -c '{report: (.report | {served,refused,cost_credits:(.cost_credits*1e6|round/1e6),stopped}), statuses: [.items[] | {status}]}'
printf '%s\n' '{"exit":0}'
python3 -c 'import json, sys, time
elapsed = int(time.time() * 1000) - int(sys.argv[1])
print(json.dumps({"scene": "route-and-cost", "elapsed_ms": elapsed}))' "$started" \
  > media/out/route-and-cost.summary.json
