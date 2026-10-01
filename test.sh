#!/bin/bash

set -e
cd $(dirname $0)

FLAMEGRAPH=0
PROFILE=0
EXTRA_ARGS=""
for var in "$@"
do
    if [[ "$var" == "--flamegraph" ]]; then
      FLAMEGRAPH=1
    elif [[ "$var" == "--profile" ]]; then
      PROFILE=1
    else
      EXTRA_ARGS="$EXTRA_ARGS $var"
    fi
done

if [[ $PROFILE == 1 ]]; then
    echo "---- profiling binary"
    RUST_LOG=debug cargo test --features=binary --features=gen-traces -- binary_tests
elif [[ $FLAMEGRAPH == 1 ]]; then
    rm -rf flamegraph.svg
    echo "---- generating unit test flamegraph"
    cargo flamegraph --unit-test --unit-test-kind lib --features=binary --dev $EXTRA_ARGS
    mv flamegraph.svg unit-flamegraph.svg
    echo "---- generating binary integration flamegraph"
    cargo flamegraph --test binary --features=binary --dev$EXTRA_ARGS
    mv flamegraph.svg integration-flamegraph.svg
else
    echo "---- features: binary"
    RUST_LOG=debug cargo test --features=binary $EXTRA_ARGS
    echo "---- features: [no default], typescript"
    RUST_LOG=debug cargo test --no-default-features --features=typescript $EXTRA_ARGS
    echo "---- features: [no default], uniffi"
    RUST_LOG=debug cargo test --no-default-features --features=uniffi $EXTRA_ARGS
    echo "---- features: carriers"
    RUST_LOG=debug cargo test --features=carriers $EXTRA_ARGS
    if command -v node >/dev/null 2>&1; then
        echo "---- carrier harness"
        node --test src/binary/carriers/harness.test.mjs
    else
        echo "---- carrier harness: skipped (no node)"
    fi
fi

rm -rf target/file-system-tests
rm -rf target/binary-tests
