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

# fetch NAME VERSION SHA256 EXT MIRROR...: NAME-VERSION.tar.EXT from the
# first of x.org and the mirrors that answers, checked against the pin,
# unpacked into SRC.
fetch() {
  local name="$1" version="$2" sha="$3" ext="$4"
  shift 4
  local tarball="$CACHE/$name-$version.tar.$ext"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    local url base
    for base in "$X11_LIB_URL" "$@"; do
      url="$base/$name-$version.tar.$ext"
      # The X.Org server resets CI's connections for hours at a time, which
      # --retry alone does not get past; the next URL is a mirror.
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
  if [[ "$ext" == xz ]] && ! command -v xz >/dev/null 2>&1; then
    # The CI image has python3 but no xz.
    python3 -c 'import lzma, sys; sys.stdout.buffer.write(lzma.decompress(sys.stdin.buffer.read()))' \
      < "$tarball" | tar -xf - -C "$SRC"
  else
    tar -xf "$tarball" -C "$SRC"
  fi
}

# shellcheck disable=SC2086 # the mirror lists are space-separated
fetch libfontenc "$LIBFONTENC_VERSION" "$LIBFONTENC_SHA256" xz $X11_MIRROR_URLS
fetch libXfont "$LIBXFONT_VERSION" "$LIBXFONT_SHA256" gz "$LIBXFONT_MIRROR_URL"
# shellcheck disable=SC2086
fetch libXdmcp "$LIBXDMCP_VERSION" "$LIBXDMCP_SHA256" xz $X11_MIRROR_URLS
echo "tinyx sources -> $SRC"
