#!/usr/bin/env bash
# Cross-build the image libraries against mlibc for the three arches:
# libpng (over ports/zlib's mlibc build), libjpeg-turbo (no SIMD: its
# assembly needs nasm on x86), libwebp and pixman, static, into one prefix
# per arch, target/image-libs-mlibc-<arch> (lib/ with the archives and their
# pkg-config files, include/), what cairo, gdk-pixbuf and GTK (issue #616)
# link. Then the package's smoke (smoke.c) over them and x11-libs' mlibc
# build.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/image-libs/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/image-libs/versions.env
source "$HERE/versions.env"

if myos_image_libs_is_current; then
  echo "image libraries up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/ports/zlib/build.sh"
"$ROOT/packages/x11-libs/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's tool wrappers, <triple>-ar, -ranlib and -strip
# (zlib's build makes them), over LLVM's: the host's strip does not know
# aarch64 and riscv64, and the CI image has LLVM's under versioned names
# only.
export PATH="$ROOT/target/newlib-bin:$PATH"
for tool in meson ninja cmake pkg-config; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

T="$ROOT/target"
SRC="$T/image-libs-src"
jobs="$(nproc 2>/dev/null || echo 4)"

# configure_make NAME DIR [ARG...]: an autoconf build of target/image-libs-src/
# NAME in DIR with the arch's cross cc, installed into $out.
configure_make() {
  local name="$1" dir="$2"
  shift 2
  mkdir -p "$dir"
  (cd "$dir" && CC="$cc" AR="$ar" RANLIB="$ranlib" CPPFLAGS="-I$out/include" LDFLAGS="-L$out/lib" \
    "$SRC/$name/configure" --host="${arch}-unknown-none" --build="$(sh "$SRC/$name/config.guess")" \
      --prefix="$out" --enable-static --disable-shared "$@" >configure.log 2>&1) \
    || { tail -30 "$dir/configure.log" >&2; exit 1; }
  make -C "$dir" -j"$jobs" >"$dir/make.log" 2>&1 \
    || { grep -n "error:" "$dir/make.log" | head -20 >&2; tail -20 "$dir/make.log" >&2; exit 1; }
  make -C "$dir" install >"$dir/install.log" 2>&1 || { tail -20 "$dir/install.log" >&2; exit 1; }
  echo "$name ($arch) -> $out"
}

build_arch() {
  local arch="$1"
  local build="$T/image-libs-build-$arch"
  local out="$T/image-libs-mlibc-${arch}"
  local x11="$T/x11-libs-mlibc-${arch}/lib/x11"
  local cc="$build/cc" ar="${arch}-unknown-myos-ar" ranlib="${arch}-unknown-myos-ranlib"
  local cpu="$arch"
  echo "==> image libraries ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$build" "$out/lib/pkgconfig" "$out/include"
  myos_write_cross_cc "$arch" "$cc" -O2
  case "$arch" in aarch64) cpu=armv8-a ;; riscv64) cpu=rv64gc ;; esac

  # zlib (ports/zlib's mlibc build), for libpng.
  cp "$T/zlib-mlibc-$arch/lib/libz.a" "$out/lib/"
  cp "$T/zlib-mlibc-$arch/include/zlib.h" "$T/zlib-mlibc-$arch/include/zconf.h" "$out/include/"
  sed "s|^prefix=.*|prefix=$out|" "$T/zlib-mlibc-$arch/lib/pkgconfig/zlib.pc" >"$out/lib/pkgconfig/zlib.pc"

  # libpng, with its NEON code on aarch64 (always there); no RISC-V vector
  # code (rv64gc has none).
  configure_make "libpng-$LIBPNG_VERSION" "$build/libpng" --enable-riscv-rvv=no \
    --without-binconfigs

  # libjpeg-turbo: CMake, the 8-bit library alone (no TurboJPEG API, no
  # tools).
  cmake -S "$SRC/libjpeg-turbo-$LIBJPEG_TURBO_VERSION" -B "$build/libjpeg-turbo" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$out" -DCMAKE_INSTALL_LIBDIR=lib \
    -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR="$arch" \
    -DCMAKE_C_COMPILER="$cc" -DCMAKE_AR="$(command -v "$ar")" -DCMAKE_RANLIB="$(command -v "$ranlib")" \
    -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
    -DENABLE_SHARED=OFF -DENABLE_STATIC=ON -DWITH_SIMD=OFF -DWITH_TURBOJPEG=OFF \
    -DWITH_TOOLS=OFF -DWITH_TESTS=OFF \
    >"$build/jpeg-cmake.log" 2>&1 || { tail -30 "$build/jpeg-cmake.log" >&2; exit 1; }
  ninja -C "$build/libjpeg-turbo" install >"$build/jpeg-ninja.log" 2>&1 \
    || { grep -n "error" "$build/jpeg-ninja.log" | head -20 >&2; tail -20 "$build/jpeg-ninja.log" >&2; exit 1; }
  echo "libjpeg-turbo ($arch) -> $out"

  # libwebp: the libraries alone (no tools, so none of their image
  # libraries), threads on mlibc's pthreads.
  configure_make "libwebp-$LIBWEBP_VERSION" "$build/libwebp" --disable-gl --disable-sdl \
    --disable-png --disable-jpeg --disable-tiff --disable-gif --disable-wic \
    --enable-libwebpmux --enable-libwebpdemux --enable-libwebpdecoder

  # pixman: meson, cross, with the cross cc (as packages/glib); SIMD that
  # every CPU of the arch has (SSE2 and SSSE3 checked at run time, NEON),
  # no RISC-V vector code.
  cat >"$build/cross.txt" <<EOC
[binaries]
c = '$cc'
ar = '$ar'
strip = '${arch}-unknown-myos-strip'
pkg-config = 'pkg-config'

[built-in options]
c_args = ['-I$out/include', '-D_GNU_SOURCE']
c_link_args = ['-L$out/lib']

[properties]
needs_exe_wrapper = true
pkg_config_libdir = '$out/lib/pkgconfig'

[host_machine]
system = 'myos'
cpu_family = '$arch'
cpu = '$cpu'
endian = 'little'
EOC
  meson setup "$build/pixman" "$SRC/pixman-$PIXMAN_VERSION" --cross-file "$build/cross.txt" \
    --prefix "$out" --libdir lib --includedir include --buildtype release \
    -Ddefault_library=static -Dtests=disabled -Ddemos=disabled -Dgtk=disabled \
    -Dlibpng=disabled -Dopenmp=disabled -Dtimers=false -Drvv=disabled \
    >"$build/pixman-setup.log" 2>&1 || { tail -40 "$build/pixman-setup.log" >&2; exit 1; }
  ninja -C "$build/pixman" install >"$build/pixman-ninja.log" 2>&1 \
    || { grep -n "error" "$build/pixman-ninja.log" | head -20 >&2; tail -20 "$build/pixman-ninja.log" >&2; exit 1; }
  echo "pixman ($arch) -> $out"

  # The smoke: the four libraries, and X through x11-libs' mlibc build.
  # shellcheck disable=SC2046
  "$cc" -Wall -Werror "$HERE/smoke.c" -o "$T/image-smoke-mlibc-${arch}-unknown-none" \
    -I"$x11/include" $(PKG_CONFIG_LIBDIR="$out/lib/pkgconfig" pkg-config --cflags --libs --static \
      libpng libjpeg libwebp pixman-1) \
    -L"$x11/lib" -lXcursor -lXrender -lXfixes -lXrandr -lXi -lXinerama -lXcomposite -lXdamage \
    -lXext -lX11 -lxcb -lXau -lm
  "${arch}-unknown-myos-strip" "$T/image-smoke-mlibc-${arch}-unknown-none"
  rm -rf "$build"
  echo "image libraries ($arch) -> $out"
}

for arch in ${MYOS_IMAGE_LIBS_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

myos_image_libs_version_hash >"$MYOS_IMAGE_LIBS_VERSION"
echo "image libraries build ok"
