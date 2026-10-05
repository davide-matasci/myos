#!/usr/bin/env bash
# Fetch the pinned font stack tarballs into target/x11-xft-src/<name>-<version>
# (idempotent; not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/x11-xft/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/x11-xft-src"
CACHE="$ROOT/target/crate-fetch-x11-xft"
mkdir -p "$SRC" "$CACHE"

# fetch NAME VERSION URL SHA256
fetch() {
  local name="$1" version="$2" url="$3" sha="$4"
  local tarball="$CACHE/$name-$version.tar.gz"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    # freedesktop.org sometimes refuses a burst of CI fetches (HTTP 418),
    # which --retry alone does not retry.
    curl -L --fail --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball.partial" "$url"
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

fetch expat "$EXPAT_VERSION" "$EXPAT_URL" "$EXPAT_SHA256"
fetch freetype "$FREETYPE_VERSION" "$FREETYPE_URL" "$FREETYPE_SHA256"
fetch fontconfig "$FONTCONFIG_VERSION" "$FONTCONFIG_URL" "$FONTCONFIG_SHA256"
fetch libXrender "$LIBXRENDER_VERSION" "$LIBXRENDER_URL" "$LIBXRENDER_SHA256"
fetch libXft "$LIBXFT_VERSION" "$LIBXFT_URL" "$LIBXFT_SHA256"
echo "font stack sources -> $SRC"
