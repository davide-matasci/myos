#!/usr/bin/env bash
# Fetch the pinned tarballs into target/x11-apps-src/<name>-<version>
# (idempotent; not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/x11-apps/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/x11-apps-src"
CACHE="$ROOT/target/crate-fetch-x11-apps"
mkdir -p "$SRC" "$CACHE"

# fetch NAME VERSION SHA256 DIR: DIR/NAME-VERSION.tar.gz from x.org, checked
# against the pin, unpacked into SRC.
fetch() {
  local name="$1" version="$2" sha="$3" dir="$4"
  local tarball="$CACHE/$name-$version.tar.gz"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    curl -L --fail --retry 5 --retry-delay 2 -o "$tarball.partial" \
      "$X11_APPS_BASE_URL/$dir/$name-$version.tar.gz"
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

fetch libXext "$LIBXEXT_VERSION" "$LIBXEXT_SHA256" lib
fetch libXrandr "$LIBXRANDR_VERSION" "$LIBXRANDR_SHA256" lib
fetch xev "$XEV_VERSION" "$XEV_SHA256" app
echo "x11-apps sources -> $SRC"
