#!/bin/bash

set -e

cd $(dirname "$0")

# Verify every place the version is written down agrees with Cargo.toml
./check-versions.sh
./plugins/archival/test.sh

cargo fmt -- --check --color always
cargo clippy --all-features --all-targets -- --no-deps -D warnings
./test.sh

./validate-actions.sh
./validate-schemas.sh
