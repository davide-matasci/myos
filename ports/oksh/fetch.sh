#!/usr/bin/env bash
# Fetch a pinned portable OpenBSD ksh (oksh) release for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/oksh/versions.env
source "$HERE/versions.env"
OKSH_SRC="$ROOT/target/oksh-src"
OKSH_REV="${OKSH_REV:-15f69e430e27eebee4dd9eb5c1fd570804ab15bd}"
OKSH_TAG="${OKSH_TAG:-oksh-7.9}"

if [[ -d "$OKSH_SRC/.git" && -f "$OKSH_SRC/main.c" ]]; then
  got="$(git -C "$OKSH_SRC" rev-parse HEAD)"
  if [[ "$got" == "$OKSH_REV" ]]; then
    echo "oksh already present at $OKSH_SRC ($OKSH_REV)"
    exit 0
  fi
  echo "oksh at $got, want $OKSH_REV; refetching" >&2
  rm -rf "$OKSH_SRC"
fi

if [[ -e "$OKSH_SRC" ]]; then
  echo "removing incomplete oksh tree at $OKSH_SRC" >&2
  rm -rf "$OKSH_SRC"
fi

echo "==> fetch oksh ($OKSH_TAG / $OKSH_REV)"
"$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$OKSH_TAG" https://github.com/ibara/oksh.git "$OKSH_SRC"
got="$(git -C "$OKSH_SRC" rev-parse HEAD)"
if [[ "$got" != "$OKSH_REV" ]]; then
  echo "error: oksh HEAD $got does not match pin $OKSH_REV" >&2
  exit 1
fi
