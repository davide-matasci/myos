#!/usr/bin/env bash
# Fetch a pinned Git release for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/git/versions.env
source "$HERE/versions.env"
GIT_SRC="$ROOT/target/git-src"
GIT_REV="${GIT_REV:-e9019fcafe0040228b8631c30f97ae1adb61bcdc}"
GIT_TAG="${GIT_TAG:-v2.55.0}"

if [[ -d "$GIT_SRC/.git" && -f "$GIT_SRC/git.c" ]]; then
  got="$(git -C "$GIT_SRC" rev-parse HEAD)"
  if [[ "$got" == "$GIT_REV" ]]; then
    echo "git already present at $GIT_SRC ($GIT_REV)"
    exit 0
  fi
  echo "git at $got, want $GIT_REV; refetching" >&2
  rm -rf "$GIT_SRC"
fi

if [[ -e "$GIT_SRC" ]]; then
  echo "removing incomplete git tree at $GIT_SRC" >&2
  rm -rf "$GIT_SRC"
fi

echo "==> fetch git ($GIT_TAG / $GIT_REV)"
"$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$GIT_TAG" https://github.com/git/git.git "$GIT_SRC"
got="$(git -C "$GIT_SRC" rev-parse HEAD)"
if [[ "$got" != "$GIT_REV" ]]; then
  echo "error: git HEAD $got does not match pin $GIT_REV" >&2
  exit 1
fi
