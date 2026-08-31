#!/usr/bin/env bash
# Fetch a pinned sbase release for the myos port.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SBASE_SRC="$ROOT/target/sbase-src"
SBASE_REV="${SBASE_REV:-c546c3a5724c81cee9a11d816a38ccdf17472129}"

if [[ -d "$SBASE_SRC/.git" && -f "$SBASE_SRC/echo.c" ]]; then
  echo "sbase already present at $SBASE_SRC"
  exit 0
fi

if [[ -e "$SBASE_SRC" ]]; then
  echo "removing incomplete sbase tree at $SBASE_SRC" >&2
  rm -rf "$SBASE_SRC"
fi

echo "==> fetch sbase ($SBASE_REV)"
git clone --depth 1 https://git.suckless.org/sbase "$SBASE_SRC"
git -C "$SBASE_SRC" fetch --depth 1 origin "$SBASE_REV"
git -C "$SBASE_SRC" checkout "$SBASE_REV"
