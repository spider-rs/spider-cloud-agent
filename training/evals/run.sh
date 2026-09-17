#!/usr/bin/env bash
# The reproducibility eval for the spider-optimize trainer. FIXTURE-ONLY throughout:
# every number it prints comes from the synthetic seed 7 corpus, and a green run says
# the pipeline still recovers what synth.py planted and still writes the committed
# fixture bytes. It says nothing about real requests.
#
#   training/evals/run.sh          the 4000 pair corpus, floors from synth-seed7-floors.json,
#                                  then an export compared byte for byte with fixtures/golden
#   training/evals/run.sh --quick  the 1500 pair corpus and synth-quick-floors.json; exports
#                                  but does not compare, since the fixtures are the full corpus
#
# Both modes then run the two regression scenarios on 20000 pairs each: `stable`, whose
# gate must pass with edits applied (synth-stable-floors.json), and `reversal`, whose
# residential edit turns harmful inside the test window and whose gate must fail
# (synth-reversal-floors.json). Each ends with a `tradeoff` table under its run directory.
#
# EVAL_SCRATCH names the scratch directory (default training/data/eval-scratch, which git
# ignores). EVAL_ALLOW_DRIFT=1 turns an export digest mismatch into a warning with both
# digests and a zero exit, for a machine whose LightGBM or numpy build moves the last bits
# of a weight; evals/README.md says when that is acceptable. The last line is always
# `eval: ok` or `eval: FAILED <what>`.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
training=$(cd "$here/.." && pwd)

quick=0
for arg in "$@"; do
  case "$arg" in
    --quick) quick=1 ;;
    *) echo "usage: $0 [--quick]" >&2; exit 2 ;;
  esac
done

if [ "$quick" = 1 ]; then
  pairs=1500
  floors=$here/synth-quick-floors.json
  mode=quick
else
  pairs=4000
  floors=$here/synth-seed7-floors.json
  mode=full
fi

scratch=${EVAL_SCRATCH:-$training/data/eval-scratch}
scratch=$(mkdir -p "$scratch" && cd "$scratch" && pwd)
corpus=$scratch/$mode/corpus
run=$scratch/$mode/run
golden_run=$scratch/$mode/run-golden
out=$scratch/$mode/out
mkdir -p "$corpus" "$run" "$golden_run" "$out"

fail() {
  echo "eval: FAILED $*"
  exit 1
}

step() {
  echo "== $*"
}

cd "$training"
train() {
  uv run spider-optimize-train "$@"
}

step "synth: seed 7, $pairs pairs, $mode mode"
train synth --out "$corpus" --seed 7 --pairs "$pairs" || fail "synth"

step "validate"
train validate "$corpus" || fail "validate"

step "train: both kinds, seed 7"
train train "$corpus" --out "$run" --seed 7 || fail "train"

step "thresholds: default sweep settings, every floor is expected to abstain"
train thresholds "$corpus" --run "$run" --seed 7 || fail "thresholds"

step "evaluate"
train evaluate "$corpus" --run "$run" --quantize int8 || fail "evaluate"

step "gates: expected to exit 1 with status insufficient, nothing applied"
set +e
gates_out=$(train gates "$corpus" --run "$run" --seed 7 2>&1)
gates_code=$?
set -e
printf '%s\n' "$gates_out"
[ "$gates_code" = 1 ] || fail "gates exited $gates_code, expected 1"
for kind in mlp gbdt; do
  # A here-string, not a pipe: under pipefail, grep -q exiting on its first match
  # sends SIGPIPE to the writer and turns a match into a failure.
  grep -q "^FIXTURE-ONLY: $kind: gate insufficient on .* applied 0 " <<<"$gates_out" \
    || fail "gates: $kind did not report insufficient with 0 applied"
done
# The 4000 pair corpus lands on "applied no edit"; the 1500 pair one has fewer than 300
# test pairs and stops there first. Either way the report must name its reason.
grep -q "^Insufficient: .*, and the gate fails\.$" "$run/regression-report.md" \
  || fail "gates: the report does not say why it is insufficient"

step "eval: floors from $floors"
train eval "$corpus" --run "$run" --floors "$floors" --seed 7 || fail "eval: a floor was missed"

step "export: both kinds from the golden fixtures' relaxed sweep settings"
cp -R "$run"/. "$golden_run"/
train thresholds "$corpus" --run "$golden_run" --seed 7 \
  --min-covered 30 --r-max 0.05 --min-sites 20 || fail "thresholds for the export"
for kind in mlp gbdt; do
  train export "$corpus" --run "$golden_run" --kind "$kind" --seed 7 \
    --out "$out/synth-$kind.bin" --golden "$out/golden-$kind.json" || fail "export $kind"
done

scenario_pairs=20000

scenario() {
  local name=$1 expect_gates=$2 corpus run code
  corpus=$scratch/$mode/$name/corpus
  run=$scratch/$mode/$name/run
  mkdir -p "$corpus" "$run"
  step "synth: $name scenario, seed 7, $scenario_pairs pairs"
  train synth --out "$corpus" --seed 7 --pairs "$scenario_pairs" --scenario "$name" \
    || fail "synth $name"
  train validate "$corpus" || fail "validate $name"
  step "train and thresholds: $name, default sweep settings"
  train train "$corpus" --out "$run" --seed 7 || fail "train $name"
  train thresholds "$corpus" --run "$run" --seed 7 || fail "thresholds $name"
  step "gates: $name, expected to exit $expect_gates"
  set +e
  train gates "$corpus" --run "$run" --seed 7
  code=$?
  set -e
  [ "$code" = "$expect_gates" ] || fail "gates on $name exited $code, expected $expect_gates"
  grep -q "^## Policy against the heuristic baseline" "$run/regression-report.md" \
    || fail "gates on $name: the report has no comparison with the baseline"
  step "eval: $name against synth-$name-floors.json"
  train eval "$corpus" --run "$run" --floors "$here/synth-$name-floors.json" --seed 7 \
    || fail "eval on $name: a floor was missed"
  step "tradeoff: $name"
  train tradeoff "$corpus" --run "$run" --seed 7 || fail "tradeoff on $name"
  grep -q "^> \*\*FIXTURE-ONLY\.\*\*" "$run/tradeoff.md" \
    || fail "tradeoff on $name: the table has no banner"
}

scenario stable 0
scenario reversal 1

if [ "$quick" = 1 ]; then
  echo "quick mode: the export is not compared with fixtures/golden (they are the 4000 pair corpus)"
  echo "eval: ok"
  exit 0
fi

step "compare: sha256 of each export against the committed sidecar and golden file"
drift=0
for kind in mlp gbdt; do
  want=$(uv run python -c "import json,sys; print(json.load(open(sys.argv[1]))['sha256'])" \
    "$training/fixtures/golden/synth-$kind.sidecar.json")
  got=$(uv run python -c "import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())" \
    "$out/synth-$kind.bin")
  if [ "$want" = "$got" ]; then
    echo "synth-$kind.bin sha256 $got matches the committed sidecar"
  else
    echo "synth-$kind.bin sha256 $got, the committed sidecar says $want"
    drift=1
  fi
  if cmp -s "$out/golden-$kind.json" "$training/fixtures/golden/golden-$kind.json"; then
    echo "golden-$kind.json matches the committed file"
  else
    echo "golden-$kind.json differs from the committed file"
    drift=1
  fi
done
if [ "$drift" = 1 ]; then
  if [ "${EVAL_ALLOW_DRIFT:-0}" = 1 ]; then
    echo "warning: the export is not byte for byte the committed fixture on this machine (EVAL_ALLOW_DRIFT=1)"
  else
    fail "export drift; see the digests above, or set EVAL_ALLOW_DRIFT=1 on a machine with a different LightGBM or numpy build"
  fi
fi

echo "eval: ok"
