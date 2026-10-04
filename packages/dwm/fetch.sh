#!/usr/bin/env bash
# Fetch the pinned dwm tarball into target/dwm-src/dwm-<version> (idempotent;
# not vendored). The myos patch is applied by build.sh, on a fresh copy.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/dwm/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/dwm-src"
CACHE="$ROOT/target/crate-fetch-dwm"
tarball="$CACHE/dwm-$DWM_VERSION.tar.gz"
mkdir -p "$SRC" "$CACHE"

if [[ -f "$SRC/dwm-$DWM_VERSION/dwm.c" ]]; then
  exit 0
fi
if [[ ! -f "$tarball" ]]; then
  echo "==> fetch dwm $DWM_VERSION"
  curl -L --fail --retry 5 --retry-delay 2 -o "$tarball.partial" "$DWM_URL"
  mv "$tarball.partial" "$tarball"
fi
got="$(sha256sum "$tarball" | awk '{print $1}')"
if [[ "$got" != "$DWM_SHA256" ]]; then
  echo "error: dwm tarball sha256 $got != pin $DWM_SHA256" >&2
  rm -f "$tarball"
  exit 1
fi
rm -rf "$SRC/dwm-$DWM_VERSION"
tar -xzf "$tarball" -C "$SRC"
echo "dwm sources -> $SRC/dwm-$DWM_VERSION"
