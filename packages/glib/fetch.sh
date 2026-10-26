#!/usr/bin/env bash
# Fetch the pinned GLib, libffi, GNU libiconv and proxy-libintl sources
# into target/{glib,libffi,libiconv,proxy-libintl}-src, and zlib's and
# PCRE2's through their own fetch scripts.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/glib/versions.env
source "$HERE/versions.env"

# fetch NAME VERSION URL SHA256 TAR-FLAG [PATCH...]: target/NAME-src from
# the tarball, with the PATCHes applied.
fetch() {
  local name="$1" version="$2" url="$3" sha="$4" flag="$5"
  shift 5
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
  tmp="$(mktemp -d)"
  echo "fetch $name $version"
  curl -fsSL --retry 5 --retry-delay 2 "$url" -o "$tmp/src.tar"
  echo "$sha  $tmp/src.tar" | sha256sum -c -
  mkdir -p "$tmp/extract"
  if [[ "$flag" == J ]] && ! command -v xz >/dev/null 2>&1; then
    python3 -c 'import lzma, sys; sys.stdout.buffer.write(lzma.decompress(sys.stdin.buffer.read()))' \
      <"$tmp/src.tar" | tar -x -C "$tmp/extract"
  else
    tar -x"$flag"f "$tmp/src.tar" -C "$tmp/extract"
  fi
  mv "$tmp/extract/$name-$version" "$src"
  rm -rf "$tmp"
  for p in "$@"; do
    patch -d "$src" -p1 -s <"$p"
  done
  echo "$want" >"$stamp"
}

fetch glib "$GLIB_VERSION" "$GLIB_URL" "$GLIB_SHA256" J "$HERE"/glib-*.myos.patch
fetch libffi "$LIBFFI_VERSION" "$LIBFFI_URL" "$LIBFFI_SHA256" z
fetch libiconv "$LIBICONV_VERSION" "$LIBICONV_URL" "$LIBICONV_SHA256" z
SRC="$ROOT/target/proxy-libintl-src"
if [[ ! -d "$SRC/.git" || "$(git -C "$SRC" rev-parse HEAD)" != "$PROXY_LIBINTL_REV" ]]; then
  rm -rf "$SRC"
  echo "fetch proxy-libintl $PROXY_LIBINTL_VERSION"
  "$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$PROXY_LIBINTL_VERSION" \
    https://github.com/frida/proxy-libintl "$SRC"
  got="$(git -C "$SRC" rev-parse HEAD)"
  if [[ "$got" != "$PROXY_LIBINTL_REV" ]]; then
    echo "proxy-libintl $PROXY_LIBINTL_VERSION is $got, not the pinned $PROXY_LIBINTL_REV" >&2
    exit 1
  fi
fi
"$ROOT/ports/zlib/fetch.sh"
"$ROOT/packages/pcre2/fetch.sh"
echo "glib fetch ok ($GLIB_VERSION)"
