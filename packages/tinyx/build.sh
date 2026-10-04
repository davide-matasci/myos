#!/usr/bin/env bash
# Cross-build TinyX's Xfbdev for the three arches, with the myos patches
# (*.myos.patch: the kdrive OS layer on /dev/fb and /dev/console/kbd, no
# MIT-SHM or XTEST, current proto headers) and libfontenc and libXfont 1.x,
# against the X libraries of packages/x11-libs. Static, like everything; the
# fonts are libXfont's built-in `fixed` and `cursor`. Then the boot test
# tinyx_smoke, an X client.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/tinyx/versions.env
source "$HERE/versions.env"

if myos_tinyx_is_current; then
  echo "tinyx up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/packages/x11-libs/build.sh"
"$ROOT/ports/zlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/tinyx-src"
WORK="$ROOT/target/tinyx-build"
X11="$ROOT/packages/x11-libs"
PREFIX=/lib/x11
jobs="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"
build_triple="$(sh "$SRC/libXfont-$LIBXFONT_VERSION/config.guess")"

# configure_make DIR CONFIGURE-ARGS...: configure in DIR (in tree), make,
# install into the stage.
configure_make() {
  local dir="$1"
  shift
  (
    cd "$dir"
    ./configure --build="$build_triple" --host="${arch}-unknown-elf" \
      --prefix="$PREFIX" --disable-shared --enable-static "$@" >configure.log 2>&1 \
      || { tail -40 configure.log >&2; exit 1; }
    make -j"$jobs" >make.log 2>&1 || { grep -E "error" make.log | head -40 >&2; exit 1; }
  )
}

build_arch() {
  local arch="$1"
  local dir="$WORK/$arch" stage="$WORK/$arch/stage" cc="$WORK/$arch-cc"
  echo "==> tinyx ($arch)"
  rm -rf "$dir"
  mkdir -p "$dir"
  # The X libraries' stage, plus zlib (ports/zlib has no .pc) and what is
  # built here.
  cp -a "$ROOT/target/x11-libs-${arch}" "$stage"
  cp "$ROOT/target/zlib-${arch}"/include/*.h "$stage$PREFIX/include/"
  cp "$ROOT/target/zlib-${arch}/lib/libz.a" "$stage$PREFIX/lib/"
  printf 'prefix=%s\nName: zlib\nDescription: zlib (ports/zlib)\nVersion: 1\nLibs: -L${prefix}/lib -lz\nCflags: -I${prefix}/include\n' \
    "$PREFIX" > "$stage$PREFIX/lib/pkgconfig/zlib.pc"

  # The compiler, with the headers the X code and TinyX expect from libc.
  myos_write_cross_cc "$arch" "$cc" -I"$X11/include" -include "$X11/myos_compat.h" \
    -I"$HERE/include" -include "$HERE/myos_compat.h" -D_GNU_SOURCE
  export CC="$cc" CC_FOR_BUILD=gcc CFLAGS="-O2"
  export AR="$(command -v llvm-ar 2>/dev/null || echo ar)"
  export RANLIB="$(command -v llvm-ranlib 2>/dev/null || echo ranlib)"
  export NM="$(command -v llvm-nm 2>/dev/null || echo nm)"
  # Static links need the private dependencies too (libXfont's zlib).
  export PKG_CONFIG="pkg-config --static"
  export PKG_CONFIG_SYSROOT_DIR="$stage"
  export PKG_CONFIG_LIBDIR="$stage$PREFIX/lib/pkgconfig:$stage$PREFIX/share/pkgconfig"
  export CPPFLAGS="-I$stage$PREFIX/include" LDFLAGS="-L$stage$PREFIX/lib"
  unset PKG_CONFIG_PATH

  cp -a "$SRC/libfontenc-$LIBFONTENC_VERSION" "$dir/libfontenc"
  configure_make "$dir/libfontenc" --with-fontrootdir=/lib/X11/fonts
  make -C "$dir/libfontenc" install DESTDIR="$stage" >/dev/null
  cp -a "$SRC/libXfont-$LIBXFONT_VERSION" "$dir/libXfont"
  configure_make "$dir/libXfont" --disable-freetype --disable-fc --disable-devel-docs \
    --disable-ipv6 --without-xmlto --without-fop
  make -C "$dir/libXfont" install DESTDIR="$stage" >/dev/null
  cp "$SRC/libXdmcp-$LIBXDMCP_VERSION/include/X11/Xdmcp.h" "$stage$PREFIX/include/X11/"
  # libtool archives name the install paths, not the stage.
  find "$stage" -name '*.la' -delete

  # TinyX: git has no configure; autoreconf after the patches (one adds
  # kdrive/myos to configure.ac). Its Xext passes INITARGS, which only
  # miinitext.c defines, as K&R parameter lists clang refuses.
  cp -a "$SRC/tinyx" "$dir/tinyx"
  rm -rf "$dir/tinyx/.git"
  local p
  for p in "$HERE"/*.myos.patch; do
    patch -d "$dir/tinyx" -p1 --forward --batch -s < "$p"
  done
  # xtrans.m4 comes from the x11-libs stage, not the host.
  (cd "$dir/tinyx" && ACLOCAL_PATH="$stage$PREFIX/share/aclocal" \
    autoreconf -fi >autoreconf.log 2>&1) \
    || { tail -20 "$dir/tinyx/autoreconf.log" >&2; exit 1; }
  CPPFLAGS="$CPPFLAGS -DINITARGS=void" configure_make "$dir/tinyx" --with-kdrive-os=myos \
    --disable-xvesa --enable-xfbdev --disable-xdmcp --disable-xdm-auth-1 --disable-dpms \
    --disable-xres --disable-screensaver --disable-dbe --disable-xf86bigfont --disable-ipv6 \
    --with-fontdir=/lib/X11/fonts --with-default-font-path=built-ins
  cp "$dir/tinyx/kdrive/fbdev/Xfbdev" "$ROOT/target/xfbdev-${arch}-unknown-none"

  "$cc" -O2 -Wall -Wextra -I"$stage$PREFIX/include" "$HERE/tinyx_smoke.c" \
    -o "$ROOT/target/tinyx-smoke-${arch}-unknown-none" \
    -L"$stage$PREFIX/lib" -lX11 -lxcb -lXau
  echo "tinyx -> target/xfbdev-${arch}-unknown-none"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

myos_tinyx_version_hash >"$MYOS_TINYX_VERSION"
