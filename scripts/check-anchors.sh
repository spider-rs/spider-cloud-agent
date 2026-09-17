#!/usr/bin/env bash
# Citations are `path:line` (`symbol or phrase`). Allow drift of at most two lines.
# A separate broad parse catches citations that lost their symbol annotation.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 - <<'PY'
from pathlib import Path
import re
import sys

WINDOW = 2
FLOOR = 72
document = Path('PRINCIPLES.md').read_text()
citations = list(re.finditer(r'(?<![\w/.-])([\w./-]+):(\d+)\b', document))
errors = []
checked = 0
for match in citations:
    path, number = match[1], int(match[2])
    annotation = re.match(r'`\s*\(`([^`\n]+)`\)', document[match.end():])
    if not annotation:
        errors.append(f'{path}:{number}: missing symbol or phrase annotation')
        continue
    target = Path(path)
    if target.is_absolute() or '..' in target.parts or not target.is_file():
        errors.append(f'{path}:{number}: file does not exist inside the repo')
        continue
    lines = target.read_text().splitlines()
    checked += 1
    if not 1 <= number <= len(lines):
        errors.append(f'{path}:{number}: line does not exist')
    elif not any(annotation[1] in line for line in lines[max(0, number-1-WINDOW):number+WINDOW]):
        errors.append(f'{path}:{number}: expected {annotation[1]!r} within {WINDOW} lines')
if checked < FLOOR:
    errors.append(f'expected at least {FLOOR} anchors, checked {checked}')
for error in errors:
    print(error)
print(f'principle anchors: checked {checked} anchors ({len(citations)} citations)')
sys.exit(bool(errors))
PY
