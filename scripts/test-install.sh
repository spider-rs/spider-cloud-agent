#!/bin/sh
# Offline tests for scripts/install.sh.
#
# A fake curl serves a release built on the fly and a fake uname picks the
# platform, so no case touches the network or the real install directory.
#
#   sh scripts/test-install.sh
#   dash scripts/test-install.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
installer="$here/install.sh"
shell="${TEST_SHELL:-sh}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM

fake="$tmp/fake"
mkdir -p "$fake"

cat >"$fake/uname" <<'EOF'
#!/bin/sh
case "$1" in
  -s) printf '%s\n' "${FAKE_UNAME_S:-Darwin}" ;;
  -m) printf '%s\n' "${FAKE_UNAME_M:-arm64}" ;;
  *) exit 2 ;;
esac
EOF

# Answers a HEAD request for releases/latest with a redirect, and serves -o
# downloads from $FIXTURES by file name. Every call is logged to $CURL_LOG.
cat >"$fake/curl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$CURL_LOG"
head=false
out=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    http*) url="$1" ;;
    --*) ;;
    -*I*) head=true ;;
  esac
  shift
done
if "$head"; then
  case "$url" in
    */releases/latest)
      printf 'HTTP/2 302 \r\nlocation: https://github.com/spider-rs/spider-cloud-agent/releases/tag/v9.9.9\r\n\r\n'
      exit 0 ;;
  esac
  exit 22
fi
file="$FIXTURES/${url##*/}"
[ -n "$out" ] && [ -f "$file" ] || exit 22
cp "$file" "$out"
EOF
chmod +x "$fake/uname" "$fake/curl"

sum() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

asset="spider-agent-9.9.9-aarch64-apple-darwin.tar.gz"
good="$tmp/good"
bad="$tmp/bad"
mkdir -p "$good/stage" "$bad"
printf '#!/bin/sh\necho "spider-agent 9.9.9"\n' >"$good/stage/spider-agent"
chmod +x "$good/stage/spider-agent"
tar -czf "$good/$asset" -C "$good/stage" spider-agent
printf '%s  %s\n' "$(sum "$good/$asset")" "$asset" >"$good/SHA256SUMS.txt"

# The same release with one byte of the tarball flipped.
cp "$good/$asset" "$good/SHA256SUMS.txt" "$bad/"
size=$(wc -c <"$bad/$asset" | tr -d ' ')
offset=$((size / 2))
byte=$(dd if="$bad/$asset" bs=1 skip="$offset" count=1 2>/dev/null | od -An -tu1 | tr -d ' ')
printf '%b' "\\0$(printf '%03o' $(((byte + 1) % 256)))" \
  | dd of="$bad/$asset" bs=1 seek="$offset" count=1 conv=notrunc 2>/dev/null
[ "$(sum "$bad/$asset")" != "$(sum "$good/$asset")" ] || { echo "the corrupt fixture matches the good one" >&2; exit 1; }

failures=0
pass() { printf 'ok   %s\n' "$1"; }
flunk() { printf 'FAIL %s: %s\n' "$1" "$2"; failures=$((failures + 1)); }

# run <case dir> <fixtures> [VAR=value ...]: runs the installer, recording the
# exit code, stdout and stderr in the case dir.
run() {
  case_dir="$1"; fixtures="$2"; shift 2
  mkdir -p "$case_dir"
  : >"$case_dir/curl.log"
  set +e
  env PATH="$fake:$PATH" FIXTURES="$fixtures" CURL_LOG="$case_dir/curl.log" \
    SPIDER_AGENT_INSTALL_DIR="$case_dir/bin" "$@" \
    "$shell" "$installer" >"$case_dir/out" 2>"$case_dir/err"
  code=$?
  set -e
}

name="installs the latest release"
run "$tmp/c1" "$good"
if [ "$code" -ne 0 ]; then flunk "$name" "exit $code: $(cat "$tmp/c1/err")"
elif [ "$("$tmp/c1/bin/spider-agent" --version 2>&1)" != "spider-agent 9.9.9" ]; then flunk "$name" "installed binary did not print spider-agent 9.9.9"
elif ! grep -q 'add it to your PATH' "$tmp/c1/out"; then flunk "$name" "no PATH hint in: $(cat "$tmp/c1/out")"
else pass "$name"
fi

name="refuses a tarball whose checksum does not match"
run "$tmp/c2" "$bad"
if [ "$code" -ne 1 ]; then flunk "$name" "exit $code, wanted 1"
elif ! grep -q 'checksum mismatch' "$tmp/c2/err"; then flunk "$name" "stderr lacks checksum mismatch: $(cat "$tmp/c2/err")"
elif [ -e "$tmp/c2/bin/spider-agent" ]; then flunk "$name" "a binary was installed anyway"
else pass "$name"
fi

name="points an unsupported OS at cargo install"
run "$tmp/c3" "$good" FAKE_UNAME_S=FreeBSD
if [ "$code" -ne 1 ]; then flunk "$name" "exit $code, wanted 1"
elif ! grep -q 'unsupported OS FreeBSD' "$tmp/c3/err"; then flunk "$name" "stderr does not name the OS: $(cat "$tmp/c3/err")"
elif ! grep -q 'cargo install spider-agent-cli' "$tmp/c3/err"; then flunk "$name" "stderr lacks the cargo hint: $(cat "$tmp/c3/err")"
elif [ -s "$tmp/c3/curl.log" ]; then flunk "$name" "the installer downloaded before refusing the OS"
elif [ -e "$tmp/c3/bin/spider-agent" ]; then flunk "$name" "a binary was installed anyway"
else pass "$name"
fi

name="SPIDER_AGENT_VERSION pins a release without resolving latest"
run "$tmp/c4" "$good" SPIDER_AGENT_VERSION=v9.9.9
if [ "$code" -ne 0 ]; then flunk "$name" "exit $code: $(cat "$tmp/c4/err")"
elif grep -q 'releases/latest' "$tmp/c4/curl.log"; then flunk "$name" "the installer still asked for releases/latest"
elif ! grep -q 'releases/download/v9.9.9/' "$tmp/c4/curl.log"; then flunk "$name" "no download from v9.9.9 in: $(cat "$tmp/c4/curl.log")"
elif [ "$("$tmp/c4/bin/spider-agent" --version 2>&1)" != "spider-agent 9.9.9" ]; then flunk "$name" "installed binary did not print spider-agent 9.9.9"
else pass "$name"
fi

if [ "$failures" -ne 0 ]; then
  printf '%s install test(s) failed under %s\n' "$failures" "$shell" >&2
  exit 1
fi
printf 'install tests passed under %s\n' "$shell"
