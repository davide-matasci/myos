#!/usr/bin/env bash
# Cross-build the X client libraries for the three arches: xorgproto, xtrans,
# libXau, xcb-proto, libxcb (core only, no extension libraries) and libX11
# (thread-safe with XInitThreads, no XKB), static, each with its own autoconf configure in cross
# mode. Installed under target/x11-libs-<arch> as if at /lib/x11 (libX11's
# data at /lib/X11), the way the X packages find them:
#
#   PKG_CONFIG_SYSROOT_DIR=target/x11-libs-<arch>
#   PKG_CONFIG_LIBDIR=<that>/lib/x11/lib/pkgconfig:<that>/lib/x11/share/pkgconfig
#
# Then links the boot test x11_smoke against them.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/x11-libs/versions.env
source "$HERE/versions.env"

if myos_x11_libs_is_current; then
  echo "x11 libraries up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/x11-libs-src"
WORK="$ROOT/target/x11-libs-build"
PREFIX=/lib/x11
jobs="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"
build_triple="$(sh "$SRC/xtrans-$XTRANS_VERSION/config.guess")"
AR_BIN="$(command -v llvm-ar 2>/dev/null || echo ar)"
RANLIB_BIN="$(command -v llvm-ranlib 2>/dev/null || echo ranlib)"
NM_BIN="$(command -v llvm-nm 2>/dev/null || echo nm)"

# build ARCH NAME-VERSION CONFIGURE-ARGS...: configure out of tree, make,
# install into the arch's stage.
build_one() {
  local arch="$1" pkg="$2"
  shift 2
  local dir="$WORK/$arch/$pkg"
  echo "==> $pkg ($arch)"
  rm -rf "$dir"
  mkdir -p "$dir"
  (
    cd "$dir"
    "$SRC/$pkg/configure" --build="$build_triple" --host="${arch}-unknown-elf" \
      --prefix="$PREFIX" --disable-shared --enable-static "$@" >configure.log 2>&1 \
      || { tail -40 configure.log >&2; exit 1; }
    make -j"$jobs" >make.log 2>&1 || { grep -E "error" make.log | head -40 >&2; exit 1; }
    make install DESTDIR="$STAGE" >install.log 2>&1 || { tail -20 install.log >&2; exit 1; }
  )
  # libtool archives name the install paths, not the stage: the next
  # library's link would look for /lib/x11/lib/libXau.la.
  find "$STAGE" -name '*.la' -delete
}

build_arch() {
  local arch="$1"
  STAGE="$ROOT/target/x11-libs-${arch}"
  local cc="$WORK/$arch-cc"
  rm -rf "$STAGE" "$WORK/$arch"
  mkdir -p "$STAGE" "$WORK/$arch"
  # The compiler (scripts/myos-c-userspace-lib.sh), with the headers the
  # X code expects from libc and libgloss lacks.
  myos_write_cross_cc "$arch" "$cc" -I"$HERE/include" -include "$HERE/myos_compat.h"

  export CC="$cc" CC_FOR_BUILD=gcc CFLAGS="-O2" AR="$AR_BIN" RANLIB="$RANLIB_BIN" NM="$NM_BIN"
  export PYTHON=python3
  export PKG_CONFIG_SYSROOT_DIR="$STAGE"
  export PKG_CONFIG_LIBDIR="$STAGE$PREFIX/lib/pkgconfig:$STAGE$PREFIX/share/pkgconfig"
  unset PKG_CONFIG_PATH

  build_one "$arch" "xorgproto-$XORGPROTO_VERSION"
  build_one "$arch" "xtrans-$XTRANS_VERSION"
  build_one "$arch" "libXau-$LIBXAU_VERSION"
  build_one "$arch" "xcb-proto-$XCB_PROTO_VERSION"
  # libxcb asks for pthread-stubs, which on a libc with the pthread
  # functions (libgloss pthread.c) is an empty .pc: write it rather than
  # fetch a package of nothing.
  printf 'Name: pthread stubs\nDescription: pthreads are in libc\nVersion: 0.5\nCflags:\nLibs:\n' \
    > "$STAGE$PREFIX/lib/pkgconfig/pthread-stubs.pc"
  local ext no_ext=()
  for ext in composite damage dbe dpms dri2 dri3 ge glx present randr record render \
             resource screensaver shape shm sync xevie xfixes xfree86-dri xinerama \
             xinput xkb xprint selinux xtest xv xvmc; do
    no_ext+=("--disable-$ext")
  done
  build_one "$arch" "libxcb-$LIBXCB_VERSION" --disable-devel-docs --without-doxygen "${no_ext[@]}"
  # newlib's malloc(0) returns a pointer; IPv6 is left to a later need.
  # Xlib's locks are on libgloss's pthreads (XInitThreads); its constructor
  # that would call XInitThreads by itself never runs: myos's crt0 runs no
  # .init_array (issue #343).
  build_one "$arch" "libX11-$LIBX11_VERSION" --datadir=/lib \
    --disable-specs --enable-xthreads --disable-thread-safety-constructor \
    --disable-xkb --disable-xf86bigfont --disable-loadable-i18n \
    --disable-loadable-xcursor --disable-composecache --disable-ipv6 \
    --disable-malloc0returnsnull --without-xmlto --without-fop --without-xsltproc

  "$cc" -O2 -Wall -Wextra -I"$STAGE$PREFIX/include" "$HERE/x11_smoke.c" \
    -o "$ROOT/target/x11-smoke-${arch}-unknown-none" \
    -L"$STAGE$PREFIX/lib" -lX11 -lxcb -lXau
  echo "x11 libraries -> $STAGE"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

myos_x11_libs_version_hash >"$MYOS_X11_LIBS_VERSION"
