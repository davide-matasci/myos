#!/usr/bin/env bash
# Fetch the pinned dmenu tarball into target/dmenu-src/dmenu-<version> (idempotent;
# not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/dmenu/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/dmenu-src"
CACHE="$ROOT/target/crate-fetch-dmenu"
tarball="$CACHE/dmenu-$DMENU_VERSION.tar.gz"
mkdir -p "$SRC" "$CACHE"

if [[ -f "$SRC/dmenu-$DMENU_VERSION/dmenu.c" ]]; then
  exit 0
fi
if [[ ! -f "$tarball" ]]; then
  echo "==> fetch dmenu $DMENU_VERSION"
  curl -L --fail --retry 5 --retry-delay 2 -o "$tarball.partial" "$DMENU_URL"
  mv "$tarball.partial" "$tarball"
fi
got="$(sha256sum "$tarball" | awk '{print $1}')"
if [[ "$got" != "$DMENU_SHA256" ]]; then
  echo "error: dmenu tarball sha256 $got != pin $DMENU_SHA256" >&2
  rm -f "$tarball"
  exit 1
fi
rm -rf "$SRC/dmenu-$DMENU_VERSION"
tar -xzf "$tarball" -C "$SRC"
echo "dmenu sources -> $SRC/dmenu-$DMENU_VERSION"
