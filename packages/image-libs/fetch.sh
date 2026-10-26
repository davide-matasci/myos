#!/usr/bin/env bash
# Fetch the pinned image library tarballs into target/image-libs-src/
# <name>-<version> (idempotent; not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/image-libs/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/image-libs-src"
CACHE="$ROOT/target/crate-fetch-image-libs"
mkdir -p "$SRC" "$CACHE"

# fetch NAME VERSION SHA256 URL: the tarball, checked against the pin,
# unpacked into SRC.
fetch() {
  local name="$1" version="$2" sha="$3" url="$4"
  local tarball="$CACHE/$name-$version.tar.${url##*.tar.}"
  if [[ -d "$SRC/$name-$version" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    curl -L --fail --connect-timeout 20 --retry 5 --retry-delay 10 --retry-all-errors \
      -o "$tarball.partial" "$url"
    mv "$tarball.partial" "$tarball"
  fi
  local got
  got="$(sha256sum "$tarball" | awk '{print $1}')"
  if [[ "$got" != "$sha" ]]; then
    echo "error: $name tarball sha256 $got != pin $sha" >&2
    rm -f "$tarball"
    exit 1
  fi
  if [[ "$tarball" == *.xz ]] && ! command -v xz >/dev/null 2>&1; then
    # The CI image may lack xz; python3 has lzma.
    python3 -c 'import lzma, sys; sys.stdout.buffer.write(lzma.decompress(sys.stdin.buffer.read()))' \
      < "$tarball" | tar -xf - -C "$SRC"
  else
    tar -xf "$tarball" -C "$SRC"
  fi
}

fetch libpng "$LIBPNG_VERSION" "$LIBPNG_SHA256" "$LIBPNG_URL"
fetch libjpeg-turbo "$LIBJPEG_TURBO_VERSION" "$LIBJPEG_TURBO_SHA256" "$LIBJPEG_TURBO_URL"
fetch libwebp "$LIBWEBP_VERSION" "$LIBWEBP_SHA256" "$LIBWEBP_URL"
fetch pixman "$PIXMAN_VERSION" "$PIXMAN_SHA256" "$PIXMAN_URL"
echo "image libraries -> $SRC"
