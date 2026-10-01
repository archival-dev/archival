#!/usr/bin/env bash
#
# Describes a directory the way the archival skills need to see it, as
# key=value lines in a fixed order:
#
#   site-info.sh [dir]        defaults to the current directory
#
# Never prints a credential: logged_in says only whether ~/.archivalrc holds a
# token, and remote_url has its userinfo removed. Every probe is guarded, so one
# failing tool (no git, no archival) still yields a complete report.
set -u

dir="${1:-.}"
if [ ! -d "$dir" ]; then
  echo "error=no such directory: $dir" >&2
  exit 2
fi
cd "$dir" || exit 2
echo "cwd=$(pwd -P)"

if [ -z "$(ls -A1 2>/dev/null | grep -vxF -e .git -e .claude -e .DS_Store | head -1)" ]; then
  echo "empty_dir=yes"
else
  echo "empty_dir=no"
fi

if [ -f archival_objects.toml ]; then
  site=yes
  objects_file=archival_objects.toml
elif [ -f objects.toml ]; then
  site=legacy
  objects_file=objects.toml
else
  site=no
  objects_file=none
fi
if [ -f archival.toml ]; then
  manifest=archival.toml
elif [ -f manifest.toml ]; then
  manifest=manifest.toml
else
  manifest=none
fi
echo "site=$site"
echo "manifest=$manifest"
echo "objects_file=$objects_file"

manifest_value() {
  [ "$manifest" = none ] && return 0
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$manifest" | head -1
}
site_url="$(manifest_value site_url)"
echo "site_url=${site_url:-none}"
if [ -n "$(manifest_value upload_prefix)" ]; then
  echo "upload_prefix=set"
else
  echo "upload_prefix=none"
fi
if [ -f .github/workflows/archival.yml ]; then
  echo "workflow=yes"
else
  echo "workflow=no"
fi

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "git=yes"
  branch="$(git symbolic-ref --short -q HEAD 2>/dev/null || true)"
  echo "branch=${branch:-none}"
  if [ -n "$(git status --porcelain 2>/dev/null | head -1)" ]; then
    echo "dirty=yes"
  else
    echo "dirty=no"
  fi
  remote_url="$(git remote get-url origin 2>/dev/null || true)"
  if [ -z "$remote_url" ]; then
    first="$(git remote 2>/dev/null | head -1)"
    [ -n "$first" ] && remote_url="$(git remote get-url "$first" 2>/dev/null || true)"
  fi
  if [ -z "$remote_url" ]; then
    echo "remote=none"
    echo "remote_url=none"
  else
    case "$remote_url" in
      *git.archival.dev*|*git.archival-staging.dev*) kind=archival ;;
      *github.com*) kind=github ;;
      *) kind=other ;;
    esac
    echo "remote=$kind"
    echo "remote_url=$(printf '%s' "$remote_url" | sed -E 's#//[^/@]+@#//#')"
  fi
else
  echo "git=no"
  echo "branch=none"
  echo "dirty=n/a"
  echo "remote=none"
  echo "remote_url=none"
fi

bin=""
if command -v archival >/dev/null 2>&1; then
  bin="$(command -v archival)"
elif [ -x "$HOME/.archival/bin/archival" ]; then
  bin="$HOME/.archival/bin/archival"
elif [ -x ./.archival-bin/archival ]; then
  bin="$(pwd -P)/.archival-bin/archival"
fi
if [ -n "$bin" ]; then
  version="$("$bin" --version 2>/dev/null | awk '{print $NF}')"
  echo "archival=$bin ${version:-unknown}"
else
  echo "archival=missing"
fi

if [ -f "$HOME/.archivalrc" ] &&
  grep -q '^[[:space:]]*access_token[[:space:]]*=[[:space:]]*"[^"][^"]*"' "$HOME/.archivalrc" 2>/dev/null; then
  echo "logged_in=yes"
else
  echo "logged_in=no"
fi
