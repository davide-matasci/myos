#!/usr/bin/env bash
# Fetch TinyX at its pinned commit and the pinned library tarballs into
# target/tinyx-src (idempotent; not vendored). The myos patches are applied
# by build.sh, on a fresh copy.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/tinyx/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/tinyx-src"
CACHE="$ROOT/target/crate-fetch-tinyx"
mkdir -p "$SRC" "$CACHE"

if [[ ! -d "$SRC/tinyx/.git" ]] || [[ "$(git -C "$SRC/tinyx" rev-parse HEAD 2>/dev/null)" != "$TINYX_REV" ]]; then
  rm -rf "$SRC/tinyx"
  echo "==> fetch tinyx ($TINYX_REV)"
  "$ROOT/scripts/git-retry.sh" clone --depth 1 "$TINYX_URL" "$SRC/tinyx"
  "$ROOT/scripts/git-retry.sh" -C "$SRC/tinyx" fetch --depth 1 origin "$TINYX_REV"
  git -C "$SRC/tinyx" checkout -q "$TINYX_REV"
fi

# fetch NAME VERSION SHA256
fetch() {
  local name="$1" version="$2" sha="$3"
  local tarball="$CACHE/$name-$version.tar.gz"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    curl -L --fail --retry 5 --retry-delay 2 -o "$tarball.partial" \
      "$X11_LIB_URL/$name-$version.tar.gz"
    mv "$tarball.partial" "$tarball"
  fi
  local got
  got="$(sha256sum "$tarball" | awk '{print $1}')"
  if [[ "$got" != "$sha" ]]; then
    echo "error: $name tarball sha256 $got != pin $sha" >&2
    rm -f "$tarball"
    exit 1
  fi
  rm -rf "$SRC/$name-$version"
  tar -xzf "$tarball" -C "$SRC"
}

fetch libfontenc "$LIBFONTENC_VERSION" "$LIBFONTENC_SHA256"
fetch libXfont "$LIBXFONT_VERSION" "$LIBXFONT_SHA256"
fetch libXdmcp "$LIBXDMCP_VERSION" "$LIBXDMCP_SHA256"
echo "tinyx sources -> $SRC"
