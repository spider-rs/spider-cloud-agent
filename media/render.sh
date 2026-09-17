#!/usr/bin/env bash
# Records every scene under media/tapes and keeps the fastest clean take of each.
#
# The live scenes talk to the real service, so one run tells you little: the
# first call of the day pays for DNS, TLS and a cold path on the other side.
# Each scene is recorded TAKES times, a take that did not serve what the scene
# expects is thrown away, and the quickest survivor wins.
#
#   bash media/render.sh              three takes per scene
#   TAKES=5 bash media/render.sh      five
#   SCENES="agent-calls-agent" bash media/render.sh   one scene only
#
# The two scenes that drive a coding agent CLI are the exception. Every take of
# those spends the caller's own model tokens on top of the credits, so they run
# once by default however many takes the other scenes get. Set AGENT_TAKES to
# ask for more, or TAKES to set every scene at once.
#
# vhs writes PNG frames here rather than video. Its own encoder calls ffmpeg
# with flags that ffmpeg 9 no longer accepts, and it fails without printing
# anything, so media/demo/encode.py does the encoding.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$PWD/target/release:/opt/homebrew/bin:$PATH"
export SPIDER_AGENT_NO_UPDATE=1 NO_COLOR=1
export SHELL=/bin/bash
export PS1='$ '
export HISTFILE=/dev/null
export BASH_SILENCE_DEPRECATION_WARNING=1
export BASH_ENV="$PWD/media/demo/shell-env.sh"

takes="${TAKES:-3}"
agent_takes="${TAKES:-${AGENT_TAKES:-1}}"
scenes="${SCENES:-}"

# A scene that drives a coding agent CLI needs that CLI installed and signed in.
# Without one there is nothing to record, which is not a failure: the scene is
# passed over and whatever is already in out/ stays where it is.
ready() {
  case "$1" in
    claude) command -v claude > /dev/null &&
            claude auth status 2> /dev/null | grep -q '"loggedIn": *true' ;;
    codex)  command -v codex > /dev/null && codex login status > /dev/null 2>&1 ;;
    *)      true ;;
  esac
}
mkdir -p media/out/frames media/out/takes
[[ "$(./target/release/spider-agent --version)" == 'spider-agent 0.7.0' ]]

cargo test -p spider-cloud-agent --test thrift_budget measured_numbers -- --ignored --nocapture \
  > media/out/measured-numbers.txt 2>&1
vhs validate media/tapes/*.tape

# One throwaway call so the first recorded take is not the one paying for a
# cold DNS answer and a new TLS session.
printf 'warming the path to the service\n'
spider-agent scrape https://example.com --json --budget 2 --wall 20 --no-router --quiet \
  > /dev/null || printf 'warmup call failed, recording anyway\n' >&2

if [[ -n "$scenes" ]]; then
  tapes=()
  for scene in $scenes; do tapes+=("media/tapes/$scene.tape"); done
else
  tapes=(media/tapes/*.tape)
fi

for tape in "${tapes[@]}"; do
  name="$(basename "$tape" .tape)"
  frames="media/out/frames/$name"
  scene_takes="$takes"
  case "$name" in
    claude-calls-agent) driver=claude; scene_takes="$agent_takes" ;;
    codex-calls-agent)  driver=codex;  scene_takes="$agent_takes" ;;
    *)                  driver= ;;
  esac
  if [[ -n "$driver" ]] && ! ready "$driver"; then
    printf '\n== %s, passed over: %s is not on PATH or not signed in\n' "$name" "$driver"
    continue
  fi

  # Only this run's takes compete for the scene. One recorded under an older
  # tape is not a candidate.
  rm -rf "media/out/takes/${name:?}"
  mkdir -p "media/out/takes/$name"

  for take in $(seq 1 "$scene_takes"); do
    printf '\n== %s, take %s of %s\n' "$name" "$take" "$scene_takes"
    rm -rf "${frames:?}"
    printf 'pending\n' > "media/out/$name.exit"
    rm -f "media/out/$name.summary.json"
    vhs "$tape" > /dev/null

    if [[ "$(cat "media/out/$name.exit")" != 0 ]]; then
      printf 'discarded: the scene exited %s\n' "$(cat "media/out/$name.exit")"
      continue
    fi
    if ! python3 media/demo/judge.py "$name" check; then
      continue
    fi

    dir="media/out/takes/$name/$take"
    mkdir -p "$dir"
    python3 media/demo/encode.py "$name" "$frames" "$dir" | tee "$dir/encode.json"
    if [[ -f "media/out/$name.summary.json" ]]; then
      cp "media/out/$name.summary.json" "$dir/summary.json"
    fi
  done

  # The winner only replaces what is already committed if it beat it.
  python3 media/demo/judge.py "$name" keep "media/out/takes/$name"
  rm -rf "${frames:?}"
done

python3 - <<'PY'
from pathlib import Path

for path in sorted(Path('media/out').glob('*')):
    if path.suffix in {'.gif', '.mp4', '.png'}:
        size = path.stat().st_size
        print(f'{path.name}: {size:,} bytes')
        if path.suffix == '.gif' and size >= 2_000_000:
            raise SystemExit(f'{path} exceeds 2 MB')
PY
