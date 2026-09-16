#!/usr/bin/env bash
# Policy source gate: six files minimum; only value operations are allowed.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 - <<'PY'
import sys
sys.dont_write_bytecode = True
sys.path.insert(0, 'scripts')
import importlib
boundary = importlib.import_module('check-rust-boundary')
count, errors = boundary.scan('spider-cloud-agent/src/policy', policy=True)
for error in errors:
    print(error)
print(f'policy purity: checked {count} files')
sys.exit(bool(errors))
PY
