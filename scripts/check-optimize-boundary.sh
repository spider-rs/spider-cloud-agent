#!/usr/bin/env bash
# Both feature configurations must keep the optimizer's normal dependency closure local.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 - <<'PY'
import importlib
import json
import subprocess
import sys
sys.dont_write_bytecode = True
sys.path.insert(0, 'scripts')
boundary = importlib.import_module('check-rust-boundary')
# The crate's own modules, and the router crate it is built on.
modules = {'candidates', 'edit', 'features', 'gate', 'labels', 'model', 'monitor',
           'observe', 'params', 'row', 'schema', 'validate'}
# The monitor's ring, head and latch. Atomics are the one kind of shared state
# this workspace allows; a lock is still refused by clippy.toml.
atomics = {'atomic', 'AtomicBool', 'AtomicU8', 'AtomicUsize'}
count, errors = boundary.scan('spider-optimize/src',
                              extra_roots=modules | {'spider_route'},
                              extra_macros={'include_bytes'},
                              extra_std=atomics)
# The route closure, reviewed, plus the router and this crate.
# Do not derive this allowlist from current metadata: a new dependency must fail.
allowed = set('''displaydoc form_urlencoded icu_collections icu_locale_core
icu_normalizer icu_normalizer_data icu_properties icu_properties_data icu_provider
idna idna_adapter litemap percent-encoding potential_utf proc-macro2 quote serde
serde_core serde_derive smallvec stable_deref_trait syn synstructure tinystr
unicode-ident url utf8_iter writeable yoke yoke-derive zerofrom zerofrom-derive
zerotrie zerovec zerovec-derive spider-route spider-optimize'''.split())
configs = dependencies = 0
for flags in ([], ['--all-features']):
    result = subprocess.run(['cargo', 'metadata', '--format-version', '1',
                             '--manifest-path', 'spider-optimize/Cargo.toml', *flags],
                            capture_output=True, text=True)
    if result.returncode:
        errors.append('cargo metadata failed: ' + result.stderr)
        continue
    data = json.loads(result.stdout)
    packages = {p['id']: p for p in data['packages']}
    nodes = {n['id']: n for n in data['resolve']['nodes']}
    roots = [p['id'] for p in data['packages'] if p['name'] == 'spider-optimize']
    if len(roots) != 1:
        errors.append('expected exactly one spider-optimize package')
        continue
    seen = set()
    pending = [roots[0]]
    while pending:
        current = pending.pop()
        for dep in nodes[current]['deps']:
            if not any(k['kind'] is None for k in dep['dep_kinds']):
                continue
            target = dep['pkg']
            if target not in seen:
                seen.add(target)
                pending.append(target)
    names = {packages[p]['name'] for p in seen}
    if not {'serde', 'url', 'spider-route'} <= names or len(seen) < 3:
        errors.append('normal dependency closure fell below its floor')
    if names - allowed:
        errors.append(f'normal dependencies outside allowlist: {sorted(names - allowed)}')
    configs += 1
    dependencies += len(seen)
if configs != 2:
    errors.append('both metadata configurations must be checked')
for error in errors:
    print(error)
print(f'optimize boundary: checked {count} files, {dependencies} dependencies across {configs} configurations')
sys.exit(bool(errors))
PY
