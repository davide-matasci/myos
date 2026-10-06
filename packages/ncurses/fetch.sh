#!/usr/bin/env bash
# Fetch pinned ncurses tarball into target/ncurses-src (idempotent; not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/ncurses/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/ncurses-src"
CACHE="$ROOT/target/crate-fetch-ncurses"
TARBALL="$CACHE/$NCURSES_TARBALL"

mkdir -p "$CACHE"

if [[ -f "$SRC/configure" && -f "$SRC/ncurses/base/lib_freeall.c" ]]; then
  # Already extracted at expected layout.
  echo "ncurses already present at $SRC ($NCURSES_VERSION)"
  exit 0
fi

if [[ ! -f "$TARBALL" ]]; then
  echo "==> fetch ncurses $NCURSES_VERSION"
  for url in $NCURSES_URLS; do
    if curl -L --fail --connect-timeout 20 --retry 3 --retry-delay 2 -o "$TARBALL.partial" "$url"; then
      break
    fi
    echo "fetch $url failed; trying the next mirror" >&2
    rm -f "$TARBALL.partial"
  done
  [[ -f "$TARBALL.partial" ]] || { echo "error: cannot fetch ncurses $NCURSES_VERSION" >&2; exit 1; }
  mv "$TARBALL.partial" "$TARBALL"
fi

got="$(sha256sum "$TARBALL" | awk '{print $1}')"
if [[ "$got" != "$NCURSES_SHA256" ]]; then
  echo "error: ncurses tarball sha256 $got != pin $NCURSES_SHA256" >&2
  rm -f "$TARBALL"
  exit 1
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tar -xzf "$TARBALL" -C "$ROOT/target"
# GNU tarball extracts to ncurses-6.5/
if [[ -d "$ROOT/target/ncurses-$NCURSES_VERSION" ]]; then
  mv "$ROOT/target/ncurses-$NCURSES_VERSION" "$SRC"
else
  echo "error: expected ncurses-$NCURSES_VERSION after extract" >&2
  exit 1
fi
echo "ncurses -> $SRC ($NCURSES_VERSION)"
