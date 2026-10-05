#!/usr/bin/env bash
# Fetch the pinned st tarball into target/st-src/st-<version> (idempotent;
# not vendored). The myos patch is applied by build.sh, on a fresh copy.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/st/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/st-src"
CACHE="$ROOT/target/crate-fetch-st"
tarball="$CACHE/st-$ST_VERSION.tar.gz"
mkdir -p "$SRC" "$CACHE"

if [[ -f "$SRC/st-$ST_VERSION/st.c" ]]; then
  exit 0
fi
if [[ ! -f "$tarball" ]]; then
  echo "==> fetch st $ST_VERSION"
  curl -L --fail --retry 5 --retry-delay 2 -o "$tarball.partial" "$ST_URL"
  mv "$tarball.partial" "$tarball"
fi
got="$(sha256sum "$tarball" | awk '{print $1}')"
if [[ "$got" != "$ST_SHA256" ]]; then
  echo "error: st tarball sha256 $got != pin $ST_SHA256" >&2
  rm -f "$tarball"
  exit 1
fi
rm -rf "$SRC/st-$ST_VERSION"
tar -xzf "$tarball" -C "$SRC"
echo "st sources -> $SRC/st-$ST_VERSION"
