#!/usr/bin/env bash
# Every gate this project ships behind, run from a developer machine.
#
# There is no hosted CI. The checks live here so they can be read, and so a
# green run means the same thing on any machine that can build the workspace.
#
#   scripts/verify.sh              the gates that need no secret
#   SPIDER_LEAKCHECK_WORDS=<path>  also check against the private denylist
#
# Exits non zero on the first failure, naming the gate that failed.
set -euo pipefail

cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$1"; }
fail() { printf '\nFAILED: %s\n' "$1" >&2; exit 1; }

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
cargo clippy --workspace --all-targets --all-features -- -D warnings \
  || fail "clippy"

step "tests, all features"
cargo test --workspace --all-features || fail "tests with all features"

step "tests, no default features"
# What a consumer gets who takes none of the optional surface. The library has
# to work with no model compiled in, and that path has to keep working.
cargo test --workspace --no-default-features || fail "tests with no default features"

step "docs"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features \
  || fail "docs, a warning here is a broken link on docs.rs"

step "leak check, packaged set"
cargo run -q -p xtask -- leakcheck || fail "leak check on what would publish"

step "leak check, working tree"
cargo run -q -p xtask -- leakcheck --tree || fail "leak check on what the repo shows"

if [ -n "${SPIDER_LEAKCHECK_WORDS:-}" ]; then
  printf '  checked against the private denylist at %s\n' "$SPIDER_LEAKCHECK_WORDS"
else
  printf '  note: the private denylist was not supplied, so this run checked the\n'
  printf '  short public list only. Set SPIDER_LEAKCHECK_WORDS before a release.\n'
fi

step "package"
cargo publish --dry-run -p spider-route -p spider-cloud-agent \
  || fail "packaging, the crates would not publish"

printf '\nall gates passed\n'
