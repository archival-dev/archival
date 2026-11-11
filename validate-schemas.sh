#!/bin/bash

# Lints the JSON Schema files published to https://archival.dev/schemas/ and
# registered with SchemaStore.
#
# `cargo test schema_files` already checks these against the parsers and against
# the fixtures in tests/schemas. This script covers what the jsonschema crate
# cannot: Ajv's strict mode, which is what SchemaStore's own CI gates on. Strict
# mode rejects things a permissive validator quietly ignores - draft-04 leftovers
# like a bare `id` keyword, `$ref` siblings that get dropped, unreachable
# subschemas - and a schema that fails it is a schema editors may silently
# mis-handle.

set -e

cd $(dirname "$0")

SCHEMAS=(
    archival.schema.json
    archival_objects.schema.json
    archival_editor.schema.json
    archival_template.schema.json
    archival_social.schema.json
)

echo "---- ajv strict mode"
for schema in "${SCHEMAS[@]}"; do
    npx --yes ajv-cli@5 compile --strict=true -s "$schema"
done

echo "---- prettier"
npx --yes prettier@3 --check "${SCHEMAS[@]}"
