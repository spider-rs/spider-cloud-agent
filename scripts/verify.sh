#!/usr/bin/env bash
# Every gate this project ships behind, run from a developer machine.
#
# There is no hosted CI. The checks live here so they can be read, and so a
# green run means the same thing on any machine that can build the workspace.
#
#   scripts/verify.sh              the gates that need no secret
#   SPIDER_LEAKCHECK_WORDS=<path> scripts/verify.sh --release
#
# Exits non zero on the first failure, naming the gate that failed.
set -euo pipefail

cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$1"; }
fail() { printf '\nFAILED: %s\n' "$1" >&2; exit 1; }

release=false
private_args=""
case "${1:-}" in
  "") [ "$#" -eq 0 ] || fail "usage: scripts/verify.sh [--release]" ;;
  --release)
    [ "$#" -eq 1 ] || fail "usage: scripts/verify.sh [--release]"
    release=true
    private_args=--require-private
    step "release private denylist"
    cargo run --locked -q -p xtask -- leakcheck --tree --require-private \
      || fail "release requires a readable, non-empty private denylist"
    ;;
  *) fail "usage: scripts/verify.sh [--release]" ;;
esac

step "dependency audit"
if cargo deny --version >/dev/null 2>&1; then
  if "$release"; then
    cargo deny --locked check advisories licenses bans || fail "dependency audit"
  else
    # Use cached advisories and registry data. An ordinary run adds no audit network calls.
    cargo deny --locked --offline check advisories licenses bans || fail "dependency audit (refresh the cache with cargo deny fetch)"
  fi
elif "$release"; then
  fail "release requires cargo-deny; install it with cargo install cargo-deny --locked"
else
  printf '  skipped dependency audit: cargo-deny is not installed\n'
fi

step "principle anchors"
scripts/check-anchors.sh || fail "principle anchors"

step "route boundary"
python3 scripts/check-rust-boundary.py || fail "boundary scanner self-tests"
scripts/check-route-boundary.sh || fail "route boundary"

step "policy purity"
scripts/check-policy-purity.sh || fail "policy purity"

step "format"
cargo fmt --all --check || fail "format, run cargo fmt --all"

step "clippy, all features"
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings \
  || fail "clippy"

step "tests, all features"
cargo test --locked --workspace --all-features || fail "tests with all features"

step "tests, no default features"
# What a consumer gets who takes none of the optional surface. The library has
# to work with no model compiled in, and that path has to keep working.
cargo test --locked --workspace --no-default-features || fail "tests with no default features"

step "live gate self-tests"
python3 -B scripts/test_verify_live.py || fail "live gate self-tests"

step "allocation baselines"
cargo bench --locked -p spider-cloud-agent --bench allocations -- --test \
  || fail "client allocation baselines"
cargo bench --locked -p spider-route --bench route -- --test \
  || fail "route allocation baselines"

step "docs"
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features \
  || fail "docs, a warning here is a broken link on docs.rs"

step "leak check, packaged set"
cargo run --locked -q -p xtask -- leakcheck ${private_args:+"$private_args"} || fail "leak check on what would publish"

step "leak check, working tree"
cargo run --locked -q -p xtask -- leakcheck ${private_args:+"$private_args"} --tree || fail "leak check on what the repo shows"

if [ -n "${SPIDER_LEAKCHECK_WORDS:-}" ]; then
  printf '  checked against the private denylist at %s\n' "$SPIDER_LEAKCHECK_WORDS"
else
  printf '  note: the private denylist was not supplied, so this run checked the\n'
  printf '  short public list only. Set SPIDER_LEAKCHECK_WORDS before a release.\n'
fi

step "package"
# Publish dependencies first: the CLI depends on both libraries at the workspace
# version (spider-route through spider-cloud-agent).
cargo publish --locked --dry-run -p spider-route -p spider-cloud-agent -p spider-agent-cli \
  || fail "packaging, the crates would not publish"

printf '\nall gates passed\n'
