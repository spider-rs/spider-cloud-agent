#!/usr/bin/env python3
"""Conservative source gate, not a Rust type checker.

Strip comments and literals, then only cfg(test) inline modules (balanced braces).
Other cfg branches remain checked. Reject unknown imports and macros rather than
silently assuming a new crate is pure. Aliases are checked at their import site.
"""
import re
from pathlib import Path


def production(text):
    # Rust raw strings, ordinary strings, chars, and nested block comments.
    token = re.compile(r'//[^\n]*|/\*|(?:b|c)?r(\#*)"|(?:b|c)?"(?:\\.|[^"\\])*"|b?\'(?:\\u\{[0-9a-fA-F_]+\}|\\x[0-9a-fA-F]{2}|\\.|[^\'\\\n])\'', re.S)
    out, pos = [], 0
    for_match = token.search
    while (m := for_match(text, pos)):
        out.append(text[pos:m.start()])
        end = m.end()
        if m[0] == '/*':
            depth = 1
            while depth:
                n = re.search(r'/\*|\*/', text[end:])
                if not n:
                    raise ValueError('unclosed block comment')
                depth += 1 if n[0] == '/*' else -1
                end += n.end()
        elif m[1] is not None:
            close = '"' + m[1]
            index = text.find(close, end)
            if index < 0:
                raise ValueError('unclosed raw string')
            end = index + len(close)
        out.append(' ' + '\n' * text[m.start():end].count('\n'))
        pos = end
    out.append(text[pos:])
    code = ''.join(out)
    test = re.compile(r'#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{')
    while (m := test.search(code)):
        depth, end = 1, m.end()
        while depth and end < len(code):
            depth += (code[end] == '{') - (code[end] == '}')
            end += 1
        if depth:
            raise ValueError('unclosed test module')
        code = code[:m.start()] + '\n' * code[m.start():end].count('\n') + code[end:]
    return code


def scan(directory, policy=False):
    files = sorted(Path(directory).rglob('*.rs'))
    errors = []
    if len(files) < (6 if policy else 7):
        errors.append('source file count fell below the floor')
    # Duration is a value, not a clock. Formatting into memory is also pure.
    allowed_std = {'time', 'Duration', 'fmt', 'Formatter', 'Result', 'Display', 'mem', 'discriminant'} if policy else {
        'fmt', 'Formatter', 'Result', 'error', 'Error', 'sync', 'Arc',
        'str', 'FromStr', 'ops', 'Deref', 'convert', 'TryFrom', 'cmp', 'Ordering',
        'f32', 'consts', 'TAU',
    }
    allowed_roots = {'crate', 'self', 'super', 'url', 'std'}
    if policy:
        allowed_roots |= {'backoff', 'budget', 'engine', 'ladder', 'rule'}
    if not policy:
        allowed_roots |= {'serde', 'action', 'decision', 'domain', 'features', 'heuristic', 'router'}
    allowed_macros = {'vec', 'matches', 'format', 'write', 'writeln', 'debug_assert', 'debug_assert_eq'}
    if not policy:
        allowed_macros.add('assert')  # Compile-time feature dimension assertion.
    forbidden = {'fs', 'net', 'process', 'tokio'}
    if policy:
        forbidden |= {'io', 'env', 'thread', 'Instant', 'SystemTime', 'log', 'tracing', 'reqwest', 'async_std', 'println', 'eprintln', 'print', 'eprint', 'dbg'}
    for path in files:
        try:
            code = production(path.read_text())
        except ValueError as error:
            errors.append(f'{path}: {error}')
            continue
        path_roots = allowed_roots | {'str', 'u8', 'u32', 'u64', 'usize', 'f32', 'f64', 'fmt'}
        path_roots |= set(re.findall(r'\bas\s+([a-z_]\w*)', code))
        for root in re.findall(r'(?<![\w:])([a-z_]\w*)\s*::', code):
            if root not in path_roots:
                errors.append(f'{path}: path root outside allowlist: {root}')
        if policy:
            # Local imports are limited to value types and the policy itself.
            # In particular, a renamed Transport cannot enter through crate::.
            modules = {'credits', 'error', 'params', 'policy', 'response', 'status'}
            for module in re.findall(r'\bcrate\s*::\s*(\w+)', code):
                if module not in modules:
                    errors.append(f'{path}: crate module outside allowlist: {module}')
        # Identifier matching catches grouped imports and renamed imports too.
        for name in sorted(set(re.findall(r'\b\w+\b', code)) & forbidden):
            errors.append(f'{path}: forbidden identifier {name}')
        for m in re.finditer(r'\buse\s+(.*?);', code, re.S):
            imp = m[1].strip().lstrip(':')
            root = re.match(r'\w+', imp)
            if not root or root[0] not in allowed_roots:
                errors.append(f'{path}: import outside allowlist: {imp}')
            if root and root[0] == 'std':
                # Remove rename targets; the original path must be allowed.
                words = set(re.findall(r'\w+', re.sub(r'\bas\s+\w+', '', imp))) - {'std', 'self'}
                if words - allowed_std or '*' in imp:
                    errors.append(f'{path}: std import outside allowlist: {imp}')
        for m in re.finditer(r'\b([a-zA-Z_]\w*)\s*!\s*[({\[]', code):
            if m[1] not in allowed_macros:
                errors.append(f'{path}: macro outside allowlist: {m[1]}!')
        # A clock alias still has to originate in a rejected import. Direct
        # qualified paths must meet the same std allowlist.
        for m in re.finditer(r'\bstd\s*::\s*((?:\w+\s*::\s*)*\w+)', code):
            words = set(re.findall(r'\w+', m[1]))
            if words - allowed_std:
                errors.append(f'{path}: std path outside allowlist: {m[0]}')
    return len(files), errors


def selftest():
    """Prove the gate rejects real code, including code after test modules."""
    import tempfile
    checked = 0
    errors = []
    with tempfile.TemporaryDirectory(prefix='boundary-check-') as directory:
        root = Path(directory)
        for index in range(7):
            (root / f'module{index}.rs').write_text('pub fn pure() {}\n')
        target = root / 'module0.rs'
        safe = '''
// std::fs::read("comment")
/* outer /* nested std::net */ comment */
const TEXT: &str = r##"std::process and { braces }"##;
#[cfg(test)]
mod tests {
    mod nested { fn test() { std::fs::read("test"); } }
}
fn after_tests() {}
'''
        cases = [
            (safe, False),
            (safe + 'use std::fs as files;', True),
            (safe + 'use std::{net as sockets};', True),
            (safe + 'use std::process::Command as Run;', True),
            (safe + 'fn io() { tokio::spawn(async {}); }', True),
            (safe + 'use std as system; fn io() { system::fs::read("x"); }', True),
        ]
        policy_cases = [
            ('use std::time::Duration; fn pure() {}', False),
            (safe + 'use std::{env as vars};', True),
            (safe + 'use std::time::Instant as Clock;', True),
            (safe + 'use std::thread as worker;', True),
            (safe + 'fn emit() { tracing::info!("message"); }', True),
            (safe + 'use log::warn as report; fn emit() { report!("message"); }', True),
            (safe + 'fn emit() { println!("message"); }', True),
            (safe + 'fn wait() { smol::Timer::after(span); }', True),
            (safe + 'use crate::transport::Transport as Http;', True),
            (safe + 'fn config() { option_env!("SETTING"); }', True),
        ]
        for policy, entries in [(False, cases), (True, policy_cases)]:
            for code, should_fail in entries:
                target.write_text(code)
                _, findings = scan(root, policy)
                checked += 1
                if bool(findings) != should_fail:
                    errors.append(f'case {checked}: unexpected findings: {findings}')
        nested = root / 'nested'
        nested.mkdir()
        (nested / 'module.rs').write_text('use std::net as sockets;')
        target.write_text('fn pure() {}')
        count, findings = scan(root)
        checked += 1
        if count != 8 or not findings:
            errors.append('nested source escaped the scan')
        empty = root / 'empty'
        empty.mkdir()
        _, findings = scan(empty)
        checked += 1
        if not findings:
            errors.append('empty source tree passed')
    for error in errors:
        print(error)
    print(f'boundary scanner: checked {checked} cases')
    return bool(errors)


if __name__ == '__main__':
    raise SystemExit(selftest())
