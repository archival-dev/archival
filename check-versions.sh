#!/bin/bash

# Verifies that every place archival's version is written down agrees with
# Cargo.toml. Pass a version to also require that they all match it (CI uses
# this to check the release tag). Run ./bump-version.sh to fix mismatches.

set -e

cd $(dirname "$0")

EXPECTED="$1"

# Runs an awk program that prints the version found in a file.
quoted_value() {
    awk "$1" "$2"
}

VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

FAILED=0
check() {
    if [ "$2" != "$VERSION" ]; then
        echo "Error: $1 version ($2) does not match Cargo.toml version ($VERSION)"
        FAILED=1
    fi
}

if [ -n "$EXPECTED" ] && [ "$VERSION" != "$EXPECTED" ]; then
    echo "Error: Cargo.toml version ($VERSION) does not match expected version ($EXPECTED)"
    FAILED=1
fi

check "Cargo.lock" "$(quoted_value '
    /^name = "archival"$/ { found = 1; next }
    found && /^version = "/ {
        match($0, /"[^"]*"/)
        print substr($0, RSTART + 1, RLENGTH - 2)
        exit
    }
' Cargo.lock)"

check "package.json" "$(quoted_value '
    /^  "version": "/ {
        match($0, /: "[^"]*"/)
        print substr($0, RSTART + 3, RLENGTH - 4)
        exit
    }
' package.json)"

check "action.yml archival-version" "$(quoted_value '
    /^  archival-version:/ { input = 1 }
    input && /^    default: "/ {
        match($0, /"[^"]*"/)
        print substr($0, RSTART + 1, RLENGTH - 2)
        exit
    }
' action.yml)"

# Schemas published to SchemaStore carry the version in their $id. Schemas with
# an unversioned $id (archival_template) are skipped.
for schema in *.schema.json; do
    id=$(grep '"\$id"' "$schema" | head -1)
    case "$id" in
        *jesseditson/archival/v*)
            check "$schema \$id" \
                "$(echo "$id" | sed -E 's|.*jesseditson/archival/v([0-9]+\.[0-9]+\.[0-9]+)/.*|\1|')"
            ;;
    esac
done

check "plugins/new/bin/install-archival.sh inferred" \
    "$(ARCHIVAL_VERSION= plugins/new/bin/install-archival.sh --print-version)"

if [ $FAILED -ne 0 ]; then
    exit 1
fi

echo "versions ok: $VERSION"
