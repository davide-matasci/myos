#!/usr/bin/env bash
# Fetch the pinned text and drawing library tarballs into
# target/cairo-pango-src/<name>-<version> (idempotent; not vendored).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/cairo-pango/versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/cairo-pango-src"
CACHE="$ROOT/target/crate-fetch-cairo-pango"
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

fetch harfbuzz "$HARFBUZZ_VERSION" "$HARFBUZZ_SHA256" "$HARFBUZZ_URL"
fetch fribidi "$FRIBIDI_VERSION" "$FRIBIDI_SHA256" "$FRIBIDI_URL"
fetch cairo "$CAIRO_VERSION" "$CAIRO_SHA256" "$CAIRO_URL"
fetch pango "$PANGO_VERSION" "$PANGO_SHA256" "$PANGO_URL"
fetch gdk-pixbuf "$GDK_PIXBUF_VERSION" "$GDK_PIXBUF_SHA256" "$GDK_PIXBUF_URL"
fetch libepoxy "$LIBEPOXY_VERSION" "$LIBEPOXY_SHA256" "$LIBEPOXY_URL"

# The EGL headers, file by file, into SRC/egl-registry-<commit>/{EGL,KHR}.
egl="$SRC/egl-registry-$EGL_REGISTRY_COMMIT"
if [[ ! -d "$egl" ]]; then
  rm -rf "$egl.partial"
  for h in $EGL_HEADERS; do
    mkdir -p "$egl.partial/$(dirname "${h%%:*}")"
    curl -L --fail --connect-timeout 20 --retry 5 --retry-delay 10 --retry-all-errors \
      -o "$egl.partial/${h%%:*}" "$EGL_REGISTRY_URL/${h%%:*}"
    echo "${h##*:}  $egl.partial/${h%%:*}" | sha256sum -c --quiet -
  done
  mv "$egl.partial" "$egl"
fi
echo "text and drawing libraries -> $SRC"
