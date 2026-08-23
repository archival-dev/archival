#!/usr/bin/env bash
#
# Installs an archival release binary into ./.archival-bin and prints its path.
# Safe to re-run: an already-installed matching version is a no-op.
#
#   install-archival.sh [version] [dest-dir]
#   install-archival.sh --print-version    # resolve only, install nothing
#
# The version is resolves, in order, from:
#
#   1. the argument      - what archival.dev pinned for this session; always wins
#   2. $ARCHIVAL_VERSION
#   3. this repo's Cargo.toml, which bump-version.sh owns and check-versions.sh
#      asserts against, so a release bump reaches this script with no edit
#   4. the latest GitHub release
#
# Only GitHub is contacted, which matters: it is on the default allowlist for
# Claude Code cloud sessions, so this works with no network configuration.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# bin/ -> archival/ -> plugins/ -> repo root. Present in a checkout and in the
# clone a marketplace install makes; absent if the plugin dir was copied out on
# its own, which is what the GitHub fallback below is for.
cargo_toml_version() {
  [ -f "$HERE/../../../Cargo.toml" ] || return 1
  awk '/^version = "/ { match($0, /"[^"]*"/); print substr($0, RSTART + 1, RLENGTH - 2); exit }' \
    "$HERE/../../../Cargo.toml"
}

latest_release_version() {
  curl -fsSL https://api.github.com/repos/jesseditson/archival/releases/latest |
    awk -F'"' '/"tag_name"/ { sub(/^v/, "", $4); print $4; exit }'
}

PRINT_ONLY=""
if [ "${1:-}" = "--print-version" ]; then
  PRINT_ONLY=1
  shift
fi

VERSION="${1:-${ARCHIVAL_VERSION:-}}"
[ -n "$VERSION" ] || VERSION="$(cargo_toml_version || true)"
# --print-version is what check-versions.sh compares against Cargo.toml, so it
# must not reach for the network: a release would otherwise "pass" the check by
# agreeing with GitHub rather than with this repo.
if [ -z "$VERSION" ] && [ -z "$PRINT_ONLY" ]; then
  VERSION="$(latest_release_version || true)"
fi
if [ -z "$VERSION" ]; then
  echo "could not determine an archival version to install" >&2
  exit 1
fi
VERSION="${VERSION#v}"

if [ -n "$PRINT_ONLY" ]; then
  echo "$VERSION"
  exit 0
fi
DEST="${2:-.archival-bin}"
BIN="$DEST/archival"

if [ -x "$BIN" ] && [ "$("$BIN" --version 2>/dev/null | awk '{print $NF}')" = "$VERSION" ]; then
  echo "$BIN"
  exit 0
fi

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-musl" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64)  arch="x86_64" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

# Linux on arm64 is published, but macOS is the only platform with both arches
# for every release; a missing asset surfaces as a 404 below rather than here.
target="$arch-$os"
name="archival-v$VERSION-$target"
base="https://github.com/jesseditson/archival/releases/download/v$VERSION"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/a.tar.gz" "$base/$name.tar.gz" ||
  { echo "no archival v$VERSION release asset for $target" >&2; exit 1; }

# The release publishes a .sha256 next to each asset. Verifying it is the only
# thing standing between a compromised download and arbitrary code execution, so
# a missing checksum is a failure rather than a skip.
curl -fsSL -o "$tmp/a.sha256" "$base/$name.tar.gz.sha256" ||
  { echo "no checksum published for $name.tar.gz" >&2; exit 1; }

expected="$(awk '{print $1}' <"$tmp/a.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/a.tar.gz" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/a.tar.gz" | awk '{print $1}')"
fi
if [ "$expected" != "$actual" ]; then
  echo "checksum mismatch for $name.tar.gz (expected $expected, got $actual)" >&2
  exit 1
fi

tar xzf "$tmp/a.tar.gz" -C "$tmp"
mkdir -p "$DEST"
# The binary sits one directory deep inside the tarball - see the repo's
# .github/scripts/package.sh, which the binstall metadata in Cargo.toml mirrors.
mv "$tmp/$name/archival" "$BIN"
chmod +x "$BIN"
echo "$BIN"
