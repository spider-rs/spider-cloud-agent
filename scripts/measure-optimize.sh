#!/usr/bin/env bash
# What the optimizer costs to carry, measured on this machine.
#
#   scripts/measure-optimize.sh
#
# A developer script, not a gate: it asserts nothing and exits non zero only when a
# build or a run fails. It writes target/measure-optimize/<stamp>.md with
#
#   - the bytes of every model in spider-optimize/assets,
#   - the stripped release size of spider-agent without and with
#     spider-cloud-agent/optimize and spider-optimize/embedded-model, each built in
#     its own CARGO_TARGET_DIR so neither build reuses the other's artifacts,
#   - peak RSS and wall time of `spider-agent route https://example.com` for both
#     binaries, 20 runs each, min and median,
#   - the criterion means from the spider-optimize bench.
#
# The only model in assets is the synthetic fixture, and every table says so.
# Works on macOS (BSD stat, /usr/bin/time -l) and Linux (GNU stat, /usr/bin/time -v).
set -euo pipefail

cd "$(dirname "$0")/.."

RUNS=20
FEATURES="spider-cloud-agent/optimize,spider-optimize/embedded-model"
OUT_DIR=target/measure-optimize
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
REPORT="$OUT_DIR/$STAMP.md"
mkdir -p "$OUT_DIR"

fail() { printf 'measure-optimize: %s\n' "$1" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) size_of() { stat -f%z "$1"; }; TIME_FLAG=-l ;;
  Linux) size_of() { stat -c%s "$1"; }; TIME_FLAG=-v ;;
  *) fail "only macOS and Linux are supported" ;;
esac
[ -x /usr/bin/time ] || fail "/usr/bin/time is missing"
command -v python3 >/dev/null 2>&1 || fail "python3 is needed to read timings and criterion output"

HEADING="synthetic fixture model, not a trained one"

# One run of the binary under /usr/bin/time, printed as "<peak rss bytes> <wall seconds>".
measure_once() {
  local binary="$1" log
  log="$(mktemp)"
  SPIDER_AGENT_NO_UPDATE=1 /usr/bin/time "$TIME_FLAG" "$binary" route https://example.com \
    >/dev/null 2>"$log" || { cat "$log" >&2; rm -f "$log"; fail "$binary route failed"; }
  python3 - "$log" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
# macOS: "0.01 real ..." and "12345678  maximum resident set size" in bytes.
# Linux: "Elapsed (wall clock) time (h:mm:ss or m:ss): 0:00.01" and
# "Maximum resident set size (kbytes): 12345".
real = re.search(r"([\d.]+)\s+real", text)
if real:
    wall = float(real.group(1))
    rss = int(re.search(r"(\d+)\s+maximum resident set size", text).group(1))
else:
    clock = re.search(r"Elapsed \(wall clock\) time.*?: ([\d:.]+)", text).group(1)
    wall = 0.0
    for part in clock.split(":"):
        wall = wall * 60 + float(part)
    rss = int(re.search(r"Maximum resident set size \(kbytes\): (\d+)", text).group(1)) * 1024
print(rss, wall)
PY
  rm -f "$log"
}

# RUNS runs of one binary, summarized into a file as
# "<rss min> <rss median> <wall min> <wall median>". Called directly rather than in
# a command substitution, so a failed run stops the script.
measure() {
  local binary="$1" into="$2" samples i
  samples="$(mktemp)"
  for i in $(seq 1 "$RUNS"); do
    measure_once "$binary" >>"$samples"
  done
  python3 -c '
import statistics, sys
rows = [line.split() for line in sys.stdin if line.strip()]
rss = [int(r[0]) for r in rows]
wall = [float(r[1]) for r in rows]
print(min(rss), int(statistics.median(rss)), min(wall), statistics.median(wall))
' <"$samples" >"$into"
  rm -f "$samples"
}

build() {
  local dir="$1"; shift
  printf '== building spider-agent in %s %s\n' "$dir" "$*" >&2
  CARGO_TARGET_DIR="$dir" cargo build --locked --release -p spider-agent-cli "$@" >&2 \
    || fail "release build in $dir"
}

BASE_DIR="$OUT_DIR/build-base"
OPT_DIR="$OUT_DIR/build-optimize"
build "$BASE_DIR"
build "$OPT_DIR" --features "$FEATURES"
BASE_BIN="$BASE_DIR/release/spider-agent"
OPT_BIN="$OPT_DIR/release/spider-agent"

printf '== running spider-agent route, %s runs each\n' "$RUNS" >&2
measure "$BASE_BIN" "$OUT_DIR/route-base.txt"
measure "$OPT_BIN" "$OUT_DIR/route-optimize.txt"
read -r BASE_RSS_MIN BASE_RSS_MED BASE_WALL_MIN BASE_WALL_MED <"$OUT_DIR/route-base.txt"
read -r OPT_RSS_MIN OPT_RSS_MED OPT_WALL_MIN OPT_WALL_MED <"$OUT_DIR/route-optimize.txt"

printf '== cargo bench -p spider-optimize --bench optimize\n' >&2
cargo bench --locked -p spider-optimize --bench optimize >&2 || fail "optimize bench"
CRITERION="${CARGO_TARGET_DIR:-target}/criterion/optimize"

{
  printf '# spider-optimize measurements\n\n'
  printf 'Taken %s on %s %s, rustc %s, commit %s.\n\n' \
    "$STAMP" "$(uname -s)" "$(uname -m)" "$(rustc --version | cut -d' ' -f2)" \
    "$(git rev-parse --short=12 HEAD)"

  printf '## Model bytes, %s\n\n' "$HEADING"
  printf '| Asset | Bytes |\n|---|---:|\n'
  for asset in spider-optimize/assets/*.bin; do
    printf '| `%s` | %s |\n' "$(basename "$asset")" "$(size_of "$asset")"
  done

  printf '\n## Stripped release binary, %s\n\n' "$HEADING"
  printf '| spider-agent | Bytes |\n|---|---:|\n'
  BASE_SIZE="$(size_of "$BASE_BIN")"
  OPT_SIZE="$(size_of "$OPT_BIN")"
  printf '| without the optimizer | %s |\n' "$BASE_SIZE"
  printf '| with `%s` | %s |\n' "$FEATURES" "$OPT_SIZE"
  printf '| difference | %s |\n' "$((OPT_SIZE - BASE_SIZE))"

  printf '\n## `spider-agent route https://example.com`, %s runs each, %s\n\n' "$RUNS" "$HEADING"
  printf '| spider-agent | Peak RSS min (bytes) | Peak RSS median (bytes) | Wall min (s) | Wall median (s) |\n'
  printf '|---|---:|---:|---:|---:|\n'
  printf '| without the optimizer | %s | %s | %s | %s |\n' \
    "$BASE_RSS_MIN" "$BASE_RSS_MED" "$BASE_WALL_MIN" "$BASE_WALL_MED"
  printf '| with the optimizer | %s | %s | %s | %s |\n' \
    "$OPT_RSS_MIN" "$OPT_RSS_MED" "$OPT_WALL_MIN" "$OPT_WALL_MED"

  printf '\n## Criterion means, `cargo bench -p spider-optimize --bench optimize`, %s\n\n' "$HEADING"
  printf '| Benchmark | Mean |\n|---|---:|\n'
  for name in generate featurize_edit score_mlp score_gbdt choose_no_model; do
    estimates="$CRITERION/$name/new/estimates.json"
    [ -f "$estimates" ] || fail "no criterion estimate at $estimates"
    printf '| `%s` | %s |\n' "$name" "$(python3 -c '
import json, sys
ns = json.load(open(sys.argv[1]))["mean"]["point_estimate"]
for unit, scale in (("s", 1e9), ("ms", 1e6), ("µs", 1e3)):
    if ns >= scale:
        print(f"{ns / scale:.3f} {unit}")
        break
else:
    print(f"{ns:.1f} ns")
' "$estimates")"
  done
} >"$REPORT"

printf '%s\n' "$REPORT"
