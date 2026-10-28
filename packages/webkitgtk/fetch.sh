#!/usr/bin/env bash
# Fetch the pinned sources into target/<name>-src: ICU, libgpg-error,
# libgcrypt, libtasn1, WebKitGTK with its patches (webkitgtk-*.myos.patch)
# applied, and HarfBuzz again (packages/cairo-pango's pin), for its ICU
# library.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/webkitgtk/versions.env
source "$HERE/versions.env"
# shellcheck source=packages/cairo-pango/versions.env
source "$ROOT/packages/cairo-pango/versions.env"

# fetch NAME VERSION URL SHA256 TAR-FLAG DIR [PATCH...]: target/NAME-src
# from the tarball's DIR, with the PATCHes applied.
fetch() {
  local name="$1" version="$2" url="$3" sha="$4" flag="$5" dir="$6"
  shift 6
  local src="$ROOT/target/$name-src" stamp="$ROOT/target/.$name-src-version"
  local want="$version" p
  for p in "$@"; do want+=" $(sha256sum <"$p" | cut -c1-16)"; done
  if [[ -f "$stamp" && "$(cat "$stamp")" == "$want" && -d "$src" ]]; then
    echo "$name sources up to date ($version)"
    return
  fi
  rm -rf "$src"
  mkdir -p "$ROOT/target"
  local tmp
  tmp="$(mktemp -d "$ROOT/target/.fetch-$name.XXXXXX")"
  echo "fetch $name $version"
  curl -fsSL --retry 5 --retry-delay 2 "$url" -o "$tmp/src.tar"
  echo "$sha  $tmp/src.tar" | sha256sum -c -
  mkdir -p "$tmp/extract"
  tar -x"$flag"f "$tmp/src.tar" -C "$tmp/extract"
  mv "$tmp/extract/$dir" "$src"
  rm -rf "$tmp"
  for p in "$@"; do
    patch -d "$src" -p1 -s <"$p"
  done
  echo "$want" >"$stamp"
}

fetch icu "$ICU_VERSION" "$ICU_URL" "$ICU_SHA256" z icu
fetch libgpg-error "$LIBGPG_ERROR_VERSION" "$LIBGPG_ERROR_URL" "$LIBGPG_ERROR_SHA256" j \
  "libgpg-error-$LIBGPG_ERROR_VERSION"
fetch libgcrypt "$LIBGCRYPT_VERSION" "$LIBGCRYPT_URL" "$LIBGCRYPT_SHA256" j "libgcrypt-$LIBGCRYPT_VERSION"
fetch libtasn1 "$LIBTASN1_VERSION" "$LIBTASN1_URL" "$LIBTASN1_SHA256" z "libtasn1-$LIBTASN1_VERSION"
fetch harfbuzz "$HARFBUZZ_VERSION" "$HARFBUZZ_URL" "$HARFBUZZ_SHA256" J "harfbuzz-$HARFBUZZ_VERSION"
shopt -s nullglob
fetch webkitgtk "$WEBKITGTK_VERSION" "$WEBKITGTK_URL" "$WEBKITGTK_SHA256" J \
  "webkitgtk-$WEBKITGTK_VERSION" "$HERE"/webkitgtk-*.myos.patch
echo "webkitgtk fetch ok ($WEBKITGTK_VERSION)"
