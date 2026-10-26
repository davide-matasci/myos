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

# fetch DIR NAME VERSION SHA256: DIR is proto/ or lib/ on the X.Org server;
# the tarball from the first of it and the mirrors that answers.
fetch() {
  local dir="$1" name="$2" version="$3" sha="$4"
  local tarball="$CACHE/$name-$version.tar.xz"
  if [[ -f "$SRC/$name-$version/configure" ]]; then
    return
  fi
  if [[ ! -f "$tarball" ]]; then
    echo "==> fetch $name $version"
    local url mirror urls=("$X11_BASE_URL/$dir/$name-$version.tar.xz")
    for mirror in $X11_MIRROR_URLS; do
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

fetch proto xorgproto "$XORGPROTO_VERSION" "$XORGPROTO_SHA256"
fetch lib xtrans "$XTRANS_VERSION" "$XTRANS_SHA256"
fetch lib libXau "$LIBXAU_VERSION" "$LIBXAU_SHA256"
fetch proto xcb-proto "$XCB_PROTO_VERSION" "$XCB_PROTO_SHA256"
fetch lib libxcb "$LIBXCB_VERSION" "$LIBXCB_SHA256"
fetch lib libX11 "$LIBX11_VERSION" "$LIBX11_SHA256"
fetch lib libXext "$LIBXEXT_VERSION" "$LIBXEXT_SHA256"
fetch lib libXrender "$LIBXRENDER_VERSION" "$LIBXRENDER_SHA256"
fetch lib libXfixes "$LIBXFIXES_VERSION" "$LIBXFIXES_SHA256"
fetch lib libXi "$LIBXI_VERSION" "$LIBXI_SHA256"
fetch lib libXrandr "$LIBXRANDR_VERSION" "$LIBXRANDR_SHA256"
fetch lib libXdamage "$LIBXDAMAGE_VERSION" "$LIBXDAMAGE_SHA256"
fetch lib libXcomposite "$LIBXCOMPOSITE_VERSION" "$LIBXCOMPOSITE_SHA256"
fetch lib libXcursor "$LIBXCURSOR_VERSION" "$LIBXCURSOR_SHA256"
fetch lib libXinerama "$LIBXINERAMA_VERSION" "$LIBXINERAMA_SHA256"
echo "x11 libraries -> $SRC"
