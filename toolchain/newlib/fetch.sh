#!/usr/bin/env bash
# Fetch a pinned newlib release for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
NEWLIB_SRC="$ROOT/target/newlib-src"
NEWLIB_TAG="${NEWLIB_TAG:-newlib-4.4.0}"

if [[ -d "$NEWLIB_SRC/newlib" && -f "$NEWLIB_SRC/config.sub" ]]; then
  echo "newlib already present at $NEWLIB_SRC"
  exit 0
fi

if [[ -e "$NEWLIB_SRC" ]]; then
  echo "removing incomplete newlib tree at $NEWLIB_SRC" >&2
  rm -rf "$NEWLIB_SRC"
fi

echo "==> fetch newlib ($NEWLIB_TAG)"
git clone --depth 1 --branch "$NEWLIB_TAG" \
  https://sourceware.org/git/newlib-cygwin.git "$NEWLIB_SRC"
