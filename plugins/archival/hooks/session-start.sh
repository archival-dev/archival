#!/usr/bin/env bash
#
# SessionStart hook. When the project directory holds an Archival site, tells
# Claude so and names the skill for working on it. Prints nothing otherwise, and
# never fails the session.
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dir="${CLAUDE_PROJECT_DIR:-$PWD}"
info="$(bash "$HERE/../bin/site-info.sh" "$dir" 2>/dev/null)" || exit 0

value() {
  printf '%s\n' "$info" | sed -n "s/^$1=//p" | head -1
}
case "$(value site)" in
  yes | legacy) ;;
  *) exit 0 ;;
esac

json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

context="This folder is an Archival site (manifest: $(value manifest); objects: $(value objects_file); git remote: $(value remote); archival CLI: $(value archival); archival login: $(value logged_in)). For any change to it, use the archival:site skill, which begins by running its bin/site-info.sh."
printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' \
  "$(json_escape "$context")"
