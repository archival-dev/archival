#!/bin/bash

# Sets the archival version everywhere it is written down, then commits the
# result and tags it v<version>. Run this instead of editing versions by hand.

set -e

cd $(dirname "$0")

VERSION="$1"

if [ -z "$VERSION" ]; then
    echo "Usage: ./bump-version.sh <version>   (e.g. ./bump-version.sh 0.18.0)"
    exit 1
fi

if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "Error: '$VERSION' is not a semver version (major.minor.patch)"
    exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
    echo "Error: working tree is dirty; commit or stash your changes first"
    git status --short
    exit 1
fi

if git rev-parse -q --verify "refs/tags/v$VERSION" > /dev/null; then
    echo "Error: tag v$VERSION already exists"
    exit 1
fi

# BSD and GNU sed disagree about in-place editing, so rewrite via a temp file.
# Usage: rewrite <file> <command...>, where the file is passed to the command.
rewrite() {
    file="$1"
    shift
    "$@" "$file" > "$file.bump.tmp"
    mv "$file.bump.tmp" "$file"
}

# The [package] version is the first bare `version = ` in Cargo.toml; the rest
# belong to dependencies.
rewrite Cargo.toml awk -v v="$VERSION" '
    !done && /^version = "/ { sub(/"[^"]*"/, "\"" v "\""); done = 1 }
    { print }
'

# In the lockfile, only the archival package entry is ours.
rewrite Cargo.lock awk -v v="$VERSION" '
    /^name = "archival"$/ { found = 1 }
    found && /^version = "/ { sub(/"[^"]*"/, "\"" v "\""); found = 0 }
    { print }
'

rewrite package.json awk -v v="$VERSION" '
    !done && /^  "version": "/ { sub(/: "[^"]*"/, ": \"" v "\""); done = 1 }
    { print }
'

# The action installs this version from cargo when no binary is provided.
rewrite action.yml awk -v v="$VERSION" '
    /^  archival-version:/ { input = 1 }
    input && /^    default: "/ { sub(/"[^"]*"/, "\"" v "\""); input = 0 }
    { print }
'

./check-versions.sh

git commit --quiet --all --message "v$VERSION"
git tag "v$VERSION"

echo "committed and tagged v$VERSION"
