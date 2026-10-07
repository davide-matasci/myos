#!/usr/bin/env bash
# Cross-build xev for the three arches: libXext and libXrandr (xev asks the
# server for RandR events), static, then upstream xev, unpatched, each with
# its own autoconf configure in cross mode, on top of a copy of the font
# stack's stage (packages/x11-xft: libX11, xcb and libXrender, which
# libXrandr builds on) in target/x11-apps-<arch>.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/x11-apps/versions.env
source "$HERE/versions.env"

if myos_x11_apps_is_current; then
  echo "x11-apps up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/packages/x11-xft/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/x11-apps-src"
WORK="$ROOT/target/x11-apps-build"
X11="$ROOT/packages/x11-libs"
PREFIX=/lib/x11
jobs="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"
build_triple="$(sh "$SRC/xev-$XEV_VERSION/config.guess")"

# build_one ARCH NAME-VERSION CONFIGURE-ARGS...: configure in a copy of the
# source, make, install into the arch's stage.
build_one() {
  local arch="$1" pkg="$2"
  shift 2
  local dir="$WORK/$arch/$pkg"
  echo "==> $pkg ($arch)"
  rm -rf "$dir"
  mkdir -p "$WORK/$arch"
  cp -a "$SRC/$pkg" "$dir"
  (
    cd "$dir"
    ./configure --build="$build_triple" --host="${arch}-unknown-elf" \
      --prefix="$PREFIX" --disable-shared --enable-static "$@" >configure.log 2>&1 \
      || { tail -40 configure.log >&2; exit 1; }
    make -j"$jobs" >make.log 2>&1 || { grep -E "error" make.log | head -40 >&2; exit 1; }
    make install DESTDIR="$STAGE" >install.log 2>&1 || { tail -20 install.log >&2; exit 1; }
  )
  # libtool archives name the install paths, not the stage.
  find "$STAGE" -name '*.la' -delete
}

build_arch() {
  local arch="$1"
  STAGE="$ROOT/target/x11-apps-${arch}"
  local cc="$WORK/$arch-cc"
  rm -rf "$STAGE" "$WORK/$arch"
  mkdir -p "$WORK/$arch"
  cp -a "$ROOT/target/x11-xft-${arch}" "$STAGE"
  myos_write_cross_cc "$arch" "$cc" -I"$X11/include" -include "$X11/myos_compat.h"

  export CC="$cc" CC_FOR_BUILD=gcc CC_BUILD=gcc CFLAGS="-O2"
  export AR="$(command -v llvm-ar 2>/dev/null || echo ar)"
  export RANLIB="$(command -v llvm-ranlib 2>/dev/null || echo ranlib)"
  export NM="$(command -v llvm-nm 2>/dev/null || echo nm)"
  export PKG_CONFIG="pkg-config --static"
  export PKG_CONFIG_SYSROOT_DIR="$STAGE"
  export PKG_CONFIG_LIBDIR="$STAGE$PREFIX/lib/pkgconfig:$STAGE$PREFIX/share/pkgconfig"
  export CPPFLAGS="-I$STAGE$PREFIX/include" LDFLAGS="-L$STAGE$PREFIX/lib"
  unset PKG_CONFIG_PATH

  # newlib's malloc(0) is not NULL (as for libX11, packages/x11-libs).
  build_one "$arch" "libXext-$LIBXEXT_VERSION" --disable-specs --disable-malloc0returnsnull \
    --without-xmlto --without-fop --without-xsltproc
  build_one "$arch" "libXrandr-$LIBXRANDR_VERSION" --disable-malloc0returnsnull
  build_one "$arch" "xev-$XEV_VERSION"
  cp "$WORK/$arch/xev-$XEV_VERSION/xev" "$ROOT/target/xev-${arch}-unknown-none"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

myos_x11_apps_version_hash >"$MYOS_X11_APPS_VERSION"
echo "x11-apps -> target/xev-<arch>-unknown-none"
