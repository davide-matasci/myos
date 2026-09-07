#!/usr/bin/env bash
# Fetch pinned Lynx tarball into target/lynx-src (idempotent; not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/lynx/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/lynx-src"
CACHE="$ROOT/target/crate-fetch-lynx"
TARBALL="$CACHE/$LYNX_TARBALL"
MARKER="$SRC/.myos-lynx-version"

mkdir -p "$CACHE"

if [[ -f "$MARKER" && "$(cat "$MARKER")" == "$LYNX_VERSION" && -f "$SRC/src/LYMain.c" ]]; then
  echo "lynx already present at $SRC ($LYNX_VERSION)"
  exit 0
fi

if [[ ! -f "$TARBALL" ]]; then
  echo "==> fetch lynx $LYNX_VERSION"
  curl -L --fail --retry 5 --retry-delay 2 -o "$TARBALL.partial" "$LYNX_URL"
  mv "$TARBALL.partial" "$TARBALL"
fi

got="$(sha256sum "$TARBALL" | awk '{print $1}')"
if [[ "$got" != "$LYNX_SHA256" ]]; then
  echo "error: lynx tarball sha256 $got != pin $LYNX_SHA256" >&2
  rm -f "$TARBALL"
  exit 1
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tar -xzf "$TARBALL" -C "$ROOT/target"
# Official tarball extracts to lynx2.9.3/
if [[ -d "$ROOT/target/lynx${LYNX_VERSION}" ]]; then
  mv "$ROOT/target/lynx${LYNX_VERSION}" "$SRC"
elif [[ -d "$ROOT/target/lynx-${LYNX_VERSION}" ]]; then
  mv "$ROOT/target/lynx-${LYNX_VERSION}" "$SRC"
else
  found="$(find "$ROOT/target" -maxdepth 1 -type d -name 'lynx*' ! -name 'lynx-src' ! -name 'lynx-myos-build' ! -name 'lynx-obj-*' | head -1)"
  [[ -n "$found" ]] || { echo "error: lynx extract failed"; exit 1; }
  mv "$found" "$SRC"
fi
echo "$LYNX_VERSION" >"$MARKER"
echo "lynx -> $SRC ($LYNX_VERSION)"
