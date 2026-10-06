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

# fetch NAME VERSION SHA256 URL...: the tarball from the first URL that
# answers (a mirror of the same bytes after the upstream one), checked
# against the pin, unpacked into SRC.
fetch() {
  local name="$1" version="$2" sha="$3"
  shift 3
  local tarball="$CACHE/$name-$version.tar.${1##*.tar.}"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    local url
    for url in "$@"; do
      # freedesktop.org refuses a burst of CI fetches (HTTP 418) for a while,
      # which --retry alone does not retry; the next URL is a mirror.
      if curl -L --fail --connect-timeout 20 --retry 3 --retry-delay 10 --retry-all-errors -o "$tarball.partial" "$url"; then
        break
      fi
      echo "fetch $url failed; trying the next mirror" >&2
      rm -f "$tarball.partial"
    done
    [[ -f "$tarball.partial" ]] || { echo "error: cannot fetch $name $version" >&2; exit 1; }
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
  if [[ "$tarball" == *.xz ]] && ! command -v xz >/dev/null 2>&1; then
    # The CI image has python3 but no xz.
    python3 -c 'import lzma, sys; sys.stdout.buffer.write(lzma.decompress(sys.stdin.buffer.read()))' \
      < "$tarball" | tar -xf - -C "$SRC"
  else
    tar -xf "$tarball" -C "$SRC"
  fi
}

fetch expat "$EXPAT_VERSION" "$EXPAT_SHA256" "$EXPAT_URL"
fetch freetype "$FREETYPE_VERSION" "$FREETYPE_SHA256" "$FREETYPE_URL" "$FREETYPE_MIRROR_URL"
fetch fontconfig "$FONTCONFIG_VERSION" "$FONTCONFIG_SHA256" "$FONTCONFIG_URL" "$FONTCONFIG_MIRROR_URL"
fetch libXrender "$LIBXRENDER_VERSION" "$LIBXRENDER_SHA256" "$LIBXRENDER_URL"
fetch libXft "$LIBXFT_VERSION" "$LIBXFT_SHA256" "$LIBXFT_URL"
echo "font stack sources -> $SRC"
