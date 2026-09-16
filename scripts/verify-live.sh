#!/usr/bin/env bash
# A release check that spends credits. Keep its record with the release evidence.
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$#" -eq 1 ] && [ "$1" = --release ] || {
  printf 'usage: scripts/verify-live.sh --release\n' >&2
  exit 1
}
exec python3 scripts/verify-live.py
