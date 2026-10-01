#!/usr/bin/env bash
#
# Checks the plugin's shell scripts: syntax, and site-info.sh's report on each
# fixture directory. Runs with a HOME of its own holding a fake ~/.archivalrc,
# so the test also proves the token never reaches the output.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

for script in bin/install-archival.sh bin/site-info.sh hooks/session-start.sh test.sh; do
  bash -n "$script"
done

FAKE_HOME="$(mktemp -d)"
EMPTY_DIR="$(mktemp -d)"
trap 'rm -rf "$FAKE_HOME" "$EMPTY_DIR"' EXIT
TOKEN="archival-cli-0123456789abcdef-SECRETSECRETSECRET"
printf 'access_token = "%s"\n' "$TOKEN" >"$FAKE_HOME/.archivalrc"

FAILED=0
expect() {
  local dir="$1" key="$2" want="$3"
  local got
  got="$(HOME="$FAKE_HOME" bash bin/site-info.sh "$dir" | sed -n "s/^$key=//p")"
  if [ "$got" != "$want" ]; then
    echo "site-info.sh $dir: $key=$got, expected $want" >&2
    FAILED=1
  fi
}

expect test/fixtures/site site yes
expect test/fixtures/site manifest archival.toml
expect test/fixtures/site objects_file archival_objects.toml
expect test/fixtures/site upload_prefix none
expect test/fixtures/site empty_dir no
expect test/fixtures/site logged_in yes

expect test/fixtures/legacy site legacy
expect test/fixtures/legacy manifest manifest.toml
expect test/fixtures/legacy objects_file objects.toml
expect test/fixtures/legacy site_url https://legacy.example.com
expect test/fixtures/legacy upload_prefix set

expect test/fixtures/not-a-site site no
expect test/fixtures/not-a-site manifest none
expect test/fixtures/not-a-site empty_dir no

expect "$EMPTY_DIR" site no
expect "$EMPTY_DIR" empty_dir yes
expect "$EMPTY_DIR" git no
expect "$EMPTY_DIR" logged_in yes

if HOME="$EMPTY_DIR" bash bin/site-info.sh test/fixtures/site | grep -q '^logged_in=yes$'; then
  echo "site-info.sh reports a login with no ~/.archivalrc" >&2
  FAILED=1
fi

for dir in test/fixtures/site test/fixtures/legacy "$EMPTY_DIR"; do
  if HOME="$FAKE_HOME" bash bin/site-info.sh "$dir" | grep -q 'SECRET'; then
    echo "site-info.sh $dir printed the access token" >&2
    FAILED=1
  fi
done

if ! bash bin/site-info.sh "$EMPTY_DIR/does-not-exist" >/dev/null 2>&1; then
  :
else
  echo "site-info.sh exited 0 for a missing directory" >&2
  FAILED=1
fi

hook_out="$(HOME="$FAKE_HOME" CLAUDE_PROJECT_DIR="$PWD/test/fixtures/site" bash hooks/session-start.sh)"
case "$hook_out" in
  '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"This folder is an Archival site'*'archival:site'*) ;;
  *)
    echo "session-start.sh did not announce the site fixture: $hook_out" >&2
    FAILED=1
    ;;
esac
if [ -n "$(CLAUDE_PROJECT_DIR="$PWD/test/fixtures/not-a-site" bash hooks/session-start.sh)" ]; then
  echo "session-start.sh spoke up for a directory that is not a site" >&2
  FAILED=1
fi

if [ "$FAILED" -ne 0 ]; then
  exit 1
fi
echo "plugin ok"
