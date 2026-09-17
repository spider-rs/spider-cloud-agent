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
  printf '%s\n' "$gates_out" | grep -q "^FIXTURE-ONLY: $kind: gate insufficient on .* applied 0 " \
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
