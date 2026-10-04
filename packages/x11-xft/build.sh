#!/usr/bin/env bash
# Cross-build the client-side font stack for the three arches: expat,
# FreeType, fontconfig, libXrender and libXft, static, each with its own
# autoconf configure in cross mode, on top of a copy of packages/x11-libs'
# stage: target/x11-xft-<arch> holds both, as if at /lib/x11, for the X
# packages drawing text with Xft (dwm). fontconfig reads
# /lib/x11/etc/fonts/fonts.conf (fonts.conf here: the fonts in /lib/fonts,
# the x11-fonts package; the cache in /tmp). Then fc-match, fc-list and the
# boot test xft_smoke.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/x11-xft/versions.env
source "$HERE/versions.env"

if myos_x11_xft_is_current; then
  echo "x11-xft up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/packages/x11-libs/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/x11-xft-src"
WORK="$ROOT/target/x11-xft-build"
X11="$ROOT/packages/x11-libs"
PREFIX=/lib/x11
jobs="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"
build_triple="$(sh "$SRC/libXft-$LIBXFT_VERSION/config.guess")"

# build_one ARCH NAME-VERSION CONFIGURE-ARGS...: configure in a copy of the
# source (FreeType builds in its tree), make, install into the arch's stage.
build_one() {
  local arch="$1" pkg="$2"
  shift 2
  local dir="$WORK/$arch/$pkg"
  echo "==> $pkg ($arch)"
  rm -rf "$dir"
  mkdir -p "$WORK/$arch"
  cp -a "$SRC/$pkg" "$dir"
  # This package's patches for it: <name>-*.myos.patch.
  local p
  for p in "$HERE/${pkg%-*}"-*.myos.patch; do
    [[ -f "$p" ]] && patch -d "$dir" -p1 --forward --batch -s < "$p"
  done
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
  STAGE="$ROOT/target/x11-xft-${arch}"
  local cc="$WORK/$arch-cc"
  rm -rf "$STAGE" "$WORK/$arch"
  mkdir -p "$WORK/$arch"
  cp -a "$ROOT/target/x11-libs-${arch}" "$STAGE"
  myos_write_cross_cc "$arch" "$cc" -I"$X11/include" -include "$X11/myos_compat.h" \
    -include "$HERE/myos_compat.h"

  export CC="$cc" CC_FOR_BUILD=gcc CC_BUILD=gcc CFLAGS="-O2"
  export AR="$(command -v llvm-ar 2>/dev/null || echo ar)"
  export RANLIB="$(command -v llvm-ranlib 2>/dev/null || echo ranlib)"
  export NM="$(command -v llvm-nm 2>/dev/null || echo nm)"
  export PKG_CONFIG="pkg-config --static"
  export PKG_CONFIG_SYSROOT_DIR="$STAGE"
  export PKG_CONFIG_LIBDIR="$STAGE$PREFIX/lib/pkgconfig:$STAGE$PREFIX/share/pkgconfig"
  export CPPFLAGS="-I$STAGE$PREFIX/include" LDFLAGS="-L$STAGE$PREFIX/lib"
  unset PKG_CONFIG_PATH

  # Hash salts from /dev/urandom (myos has no getrandom/arc4random).
  CPPFLAGS="$CPPFLAGS -DXML_DEV_URANDOM" build_one "$arch" "expat-$EXPAT_VERSION" \
    --without-docbook --without-xmlwf \
    --without-examples --without-tests
  # TrueType and OpenType only: no compressed fonts, PNG glyphs or shaping.
  build_one "$arch" "freetype-$FREETYPE_VERSION" --with-zlib=no --with-bzip2=no \
    --with-png=no --with-harfbuzz=no --with-brotli=no
  # gperf replaced (gperf-lite.py); newlib declares mkostemp for _GNU_SOURCE;
  # newlib's random() has no initstate/setstate, so fontconfig takes rand_r.
  GPERF="python3 $HERE/gperf-lite.py" CPPFLAGS="$CPPFLAGS -D_GNU_SOURCE" ac_cv_func_random=no \
    build_one "$arch" "fontconfig-$FONTCONFIG_VERSION" \
    --disable-docs --disable-nls --disable-cache-build \
    --sysconfdir="$PREFIX/etc" --localstatedir=/tmp \
    --with-default-fonts=/lib/fonts --with-cache-dir=/tmp/fontconfig
  build_one "$arch" "libXrender-$LIBXRENDER_VERSION"
  build_one "$arch" "libXft-$LIBXFT_VERSION"

  local fcdir="$WORK/$arch/fontconfig-$FONTCONFIG_VERSION"
  cp "$fcdir/fc-match/fc-match" "$ROOT/target/fc-match-${arch}-unknown-none"
  cp "$fcdir/fc-list/fc-list" "$ROOT/target/fc-list-${arch}-unknown-none"
  "$cc" -O2 -Wall -Wextra -I"$STAGE$PREFIX/include" -I"$STAGE$PREFIX/include/freetype2" \
    "$HERE/xft_smoke.c" -o "$ROOT/target/xft-smoke-${arch}-unknown-none" \
    -L"$STAGE$PREFIX/lib" -lXft -lXrender -lfontconfig -lexpat -lfreetype -lX11 -lxcb -lXau -lm
  echo "font stack -> $STAGE"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

myos_x11_xft_version_hash >"$MYOS_X11_XFT_VERSION"
