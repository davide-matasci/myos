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

# fetch NAME VERSION SHA256 DIR: DIR/NAME-VERSION.tar.xz from x.org or the
# first mirror that answers, checked against the pin, unpacked into SRC.
fetch() {
  local name="$1" version="$2" sha="$3" dir="$4"
  local tarball="$CACHE/$name-$version.tar.xz"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    local url mirror urls=("$X11_APPS_BASE_URL/$dir/$name-$version.tar.xz")
    for mirror in $X11_APPS_MIRROR_URLS; do
      urls+=("$mirror/$name-$version.tar.xz")
    done
    for url in "${urls[@]}"; do
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
  if command -v xz >/dev/null 2>&1; then
    tar -xJf "$tarball" -C "$SRC"
  else
    # The CI image has python3 but no xz.
    python3 -c 'import lzma, sys; sys.stdout.buffer.write(lzma.decompress(sys.stdin.buffer.read()))' \
      < "$tarball" | tar -xf - -C "$SRC"
  fi
}

fetch libXext "$LIBXEXT_VERSION" "$LIBXEXT_SHA256" lib
fetch libXrandr "$LIBXRANDR_VERSION" "$LIBXRANDR_SHA256" lib
fetch xev "$XEV_VERSION" "$XEV_SHA256" app
echo "x11-apps sources -> $SRC"
