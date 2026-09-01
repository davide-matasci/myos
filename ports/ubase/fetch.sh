#!/usr/bin/env bash
# Fetch a pinned ubase revision for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/ubase/versions.env
source "$HERE/versions.env"
UBASE_SRC="$ROOT/target/ubase-src"
UBASE_REV="${UBASE_REV:-f152e7fc3bd1675060818ac224f96541a2d9d6e7}"

if [[ -d "$UBASE_SRC/.git" && -f "$UBASE_SRC/getty.c" ]]; then
  got="$(git -C "$UBASE_SRC" rev-parse HEAD)"
  if [[ "$got" == "$UBASE_REV" ]]; then
    echo "ubase already present at $UBASE_SRC ($UBASE_REV)"
    exit 0
  fi
  echo "ubase at $got, want $UBASE_REV; refetching" >&2
  rm -rf "$UBASE_SRC"
fi

if [[ -e "$UBASE_SRC" ]]; then
  echo "removing incomplete ubase tree at $UBASE_SRC" >&2
  rm -rf "$UBASE_SRC"
fi

echo "==> fetch ubase ($UBASE_REV)"
"$ROOT/scripts/git-retry.sh" clone --depth 1 https://github.com/michaelforney/ubase.git "$UBASE_SRC"
"$ROOT/scripts/git-retry.sh" -C "$UBASE_SRC" fetch --depth 1 origin "$UBASE_REV"
git -C "$UBASE_SRC" checkout "$UBASE_REV"
got="$(git -C "$UBASE_SRC" rev-parse HEAD)"
if [[ "$got" != "$UBASE_REV" ]]; then
  echo "error: ubase HEAD $got does not match pin $UBASE_REV" >&2
  exit 1
fi
