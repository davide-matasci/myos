#!/usr/bin/env bash
# Fetch the pinned X library tarballs into target/x11-libs-src/<name>-<version>
# (idempotent; not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/x11-libs/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/x11-libs-src"
CACHE="$ROOT/target/crate-fetch-x11"
mkdir -p "$SRC" "$CACHE"

# fetch DIR NAME VERSION SHA256: DIR is proto/ or lib/ on the X.Org server.
fetch() {
  local dir="$1" name="$2" version="$3" sha="$4"
  local tarball="$CACHE/$name-$version.tar.gz"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    curl -L --fail --retry 5 --retry-delay 2 -o "$tarball.partial" \
      "$X11_BASE_URL/$dir/$name-$version.tar.gz"
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

fetch proto xorgproto "$XORGPROTO_VERSION" "$XORGPROTO_SHA256"
fetch lib xtrans "$XTRANS_VERSION" "$XTRANS_SHA256"
fetch lib libXau "$LIBXAU_VERSION" "$LIBXAU_SHA256"
fetch proto xcb-proto "$XCB_PROTO_VERSION" "$XCB_PROTO_SHA256"
fetch lib libxcb "$LIBXCB_VERSION" "$LIBXCB_SHA256"
fetch lib libX11 "$LIBX11_VERSION" "$LIBX11_SHA256"
fetch lib libXext "$LIBXEXT_VERSION" "$LIBXEXT_SHA256"
echo "x11 libraries -> $SRC"
