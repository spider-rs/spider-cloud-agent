#!/bin/sh
# Installs the spider-agent release build for this machine.
#
#   curl -fsSL https://spider.cloud/install/spider-agent.sh | sh
#
# SPIDER_AGENT_VERSION pins a release, v0.4.0 or later.
# SPIDER_AGENT_INSTALL_DIR picks the directory, ~/.local/bin by default.
#
# Everything runs inside main, called on the last line, so a download cut off
# partway through defines a function and runs nothing.
set -eu

REPO="spider-rs/spider-cloud-agent"
BIN="spider-agent"
FALLBACK="Use: cargo install spider-agent-cli"

fail() { printf 'spider-agent install: %s\n' "$1" >&2; exit 1; }

main() {
  command -v curl >/dev/null 2>&1 || fail "curl is not installed. $FALLBACK"

  case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux)  os="unknown-linux-gnu" ;;
    *) fail "unsupported OS $(uname -s). $FALLBACK" ;;
  esac
  case "$(uname -m)" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64)  arch="x86_64" ;;
    *) fail "unsupported architecture $(uname -m). $FALLBACK" ;;
  esac
  target="$arch-$os"

  if [ -n "${SPIDER_AGENT_VERSION:-}" ]; then
    version="${SPIDER_AGENT_VERSION#v}"
  else
    # Without pipefail a failed curl leaves this empty rather than stopping the
    # script, and the empty value fails the check below.
    location=$(curl -fsSI "https://github.com/$REPO/releases/latest" | tr -d '\r' \
      | sed -n 's/^[Ll][Oo][Cc][Aa][Tt][Ii][Oo][Nn]:[[:space:]]*//p' | tail -n 1)
    version="${location##*/tag/v}"
    [ -n "$version" ] && [ "$version" != "$location" ] || fail "could not resolve the latest release"
  fi
  case "$version" in
    ""|*[!0-9A-Za-z.+-]*) fail "not a release version: $version" ;;
  esac
  asset="$BIN-$version-$target.tar.gz"
  base="https://github.com/$REPO/releases/download/v$version"

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 1' INT TERM
  curl -fsSL "$base/$asset" -o "$tmp/$asset" || fail "no v$version build for $target. $FALLBACK"
  curl -fsSL "$base/SHA256SUMS.txt" -o "$tmp/SHA256SUMS.txt" || fail "could not download SHA256SUMS.txt for v$version"
  expected=$(awk -v a="$asset" '{ sub(/^\*/, "", $2) } $2 == a { print $1; exit }' "$tmp/SHA256SUMS.txt")
  [ -n "$expected" ] || fail "$asset is not listed in SHA256SUMS.txt"
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
  else
    fail "neither sha256sum nor shasum is installed, so the download cannot be verified"
  fi
  [ "$actual" = "$expected" ] || fail "checksum mismatch for $asset. Nothing was installed"

  # GNU tar warns about the macOS extended headers in these tarballs. Its
  # output is shown only when the extraction fails.
  if ! tar -xzf "$tmp/$asset" -C "$tmp" "$BIN" 2>"$tmp/tar.err"; then
    cat "$tmp/tar.err" >&2
    fail "could not extract $BIN from $asset"
  fi
  dir="${SPIDER_AGENT_INSTALL_DIR:-$HOME/.local/bin}"
  mkdir -p "$dir" || fail "could not create $dir"
  # A failure inside an && list does not trip set -e, so it needs its own exit.
  { cp "$tmp/$BIN" "$dir/$BIN.tmp" && chmod 0755 "$dir/$BIN.tmp" && mv -f "$dir/$BIN.tmp" "$dir/$BIN"; } \
    || fail "could not write $dir/$BIN"
  printf 'installed spider-agent %s to %s\n' "$version" "$dir/$BIN"
  # shellcheck disable=SC2016 # $PATH is printed for the user's shell to expand.
  case ":$PATH:" in
    *":$dir:"*) ;;
    *) printf 'add it to your PATH: export PATH="%s:$PATH"\n' "$dir" ;;
  esac
}

main "$@"
