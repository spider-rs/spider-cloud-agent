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
#     spider-cloud-agent/optimize (compiled in, never called, so the linker may
#     drop it), and of two spider-optimize examples that make one scoring call,
#     one through the bundled artifact and one through NoModel, each build in
#     its own CARGO_TARGET_DIR so no build reuses another's artifacts,
#   - peak RSS and wall time of those two examples, 20 runs each, min and median,
#   - the criterion means from the spider-optimize bench.
#
# The only model in assets is the synthetic fixture, and every table says so.
# Works on macOS (BSD stat, /usr/bin/time -l) and Linux (GNU stat, /usr/bin/time -v).
set -euo pipefail

cd "$(dirname "$0")/.."

RUNS=20
FEATURES="spider-cloud-agent/optimize"
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
  /usr/bin/time "$TIME_FLAG" "$binary" \
    >/dev/null 2>"$log" || { cat "$log" >&2; rm -f "$log"; fail "$binary failed"; }
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
  printf '== building in %s: %s\n' "$dir" "$*" >&2
  CARGO_TARGET_DIR="$dir" cargo build --locked --release "$@" >&2 \
    || fail "release build in $dir"
}

# The command line tool, with and without the client feature. The tool never
# constructs an optimizer, so this measures what compiling the feature in adds
# before the linker drops what nothing calls, and can come out at zero.
BASE_DIR="$OUT_DIR/build-base"
OPT_DIR="$OUT_DIR/build-optimize"
build "$BASE_DIR" -p spider-agent-cli
build "$OPT_DIR" -p spider-agent-cli --features "$FEATURES"
BASE_BIN="$BASE_DIR/release/spider-agent"
OPT_BIN="$OPT_DIR/release/spider-agent"

# Two programs that make the same one scoring call, one through the bundled
# artifact and the reader, one through NoModel. Their difference is what the
# reader and the artifact cost to carry, run and start.
EX_DIR="$OUT_DIR/build-examples"
build "$EX_DIR" -p spider-optimize --example keep_no_model
build "$EX_DIR" -p spider-optimize --features embedded-model --example score_embedded
KEEP_BIN="$EX_DIR/release/examples/keep_no_model"
SCORE_BIN="$EX_DIR/release/examples/score_embedded"

printf '== running the two scoring examples, %s runs each\n' "$RUNS" >&2
measure "$KEEP_BIN" "$OUT_DIR/run-keep.txt"
measure "$SCORE_BIN" "$OUT_DIR/run-score.txt"
read -r BASE_RSS_MIN BASE_RSS_MED BASE_WALL_MIN BASE_WALL_MED <"$OUT_DIR/run-keep.txt"
read -r OPT_RSS_MIN OPT_RSS_MED OPT_WALL_MIN OPT_WALL_MED <"$OUT_DIR/run-score.txt"

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

  printf '\n## Stripped release binaries, %s\n\n' "$HEADING"
  printf '| Binary | Bytes |\n|---|---:|\n'
  BASE_SIZE="$(size_of "$BASE_BIN")"
  OPT_SIZE="$(size_of "$OPT_BIN")"
  KEEP_SIZE="$(size_of "$KEEP_BIN")"
  SCORE_SIZE="$(size_of "$SCORE_BIN")"
  printf '| spider-agent, without the feature | %s |\n' "$BASE_SIZE"
  printf '| spider-agent, with `%s` (compiled, not called) | %s |\n' "$FEATURES" "$OPT_SIZE"
  printf '| spider-agent difference | %s |\n' "$((OPT_SIZE - BASE_SIZE))"
  printf '| `keep_no_model` example | %s |\n' "$KEEP_SIZE"
  printf '| `score_embedded` example, reader and artifact linked | %s |\n' "$SCORE_SIZE"
  printf '| example difference | %s |\n' "$((SCORE_SIZE - KEEP_SIZE))"

  printf '\n## One scoring call from a cold start, %s runs each, %s\n\n' "$RUNS" "$HEADING"
  printf '| Program | Peak RSS min (bytes) | Peak RSS median (bytes) | Wall min (s) | Wall median (s) |\n'
  printf '|---|---:|---:|---:|---:|\n'
  printf '| `keep_no_model` | %s | %s | %s | %s |\n' \
    "$BASE_RSS_MIN" "$BASE_RSS_MED" "$BASE_WALL_MIN" "$BASE_WALL_MED"
  printf '| `score_embedded` | %s | %s | %s | %s |\n' \
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
