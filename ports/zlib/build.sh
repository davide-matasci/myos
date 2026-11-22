#!/bin/sh
# Cross-build static libz.a for myos arches (like LibreSSL), against newlib
# (target/zlib-<arch>) and against mlibc (target/zlib-mlibc-<arch>, with a
# zlib.pc for the mlibc ports' pkg-config).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
. "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

echo "zlib: version=$ZLIB_VERSION"

if myos_zlib_is_current; then
  echo "zlib libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/toolchain/mlibc/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/zlib-src"

# Core + gzip API (git uses deflate/inflate; gz* kept for completeness).
ZLIB_SRCS="
  adler32.c compress.c crc32.c deflate.c
  gzclose.c gzlib.c gzread.c gzwrite.c
  inflate.c infback.c inftrees.c inffast.c
  trees.c uncompr.c zutil.c
"

# build_arch ARCH LIBC: target/zlib-<arch> for newlib, zlib-mlibc-<arch>
# for mlibc.
build_arch() (
  arch="$1" libc="$2"
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  cc="${triple}-cc"
  out="$ROOT/target/zlib-${arch}"
  clang_res="$(clang -print-resource-dir)/include"
  # The compiler's flags are the positional parameters: the libc's first.
  set -- -ffreestanding -fPIC -nostdinc -isystem "$clang_res" -isystem "$inc"
  if [ "$libc" = mlibc ]; then
    cc=clang out="$ROOT/target/zlib-mlibc-${arch}"
    # shellcheck disable=SC2046
    set -- $(myos_mlibc_cflags "$arch") -DHAVE_UNISTD_H -DHAVE_STDARG_H
  fi
  obj="$out/obj"
  lib="$out/lib"
  set -- \
    "$@" -Os -g0 -std=gnu99 \
    -I"$SRC" \
    -D_LARGEFILE64_SOURCE=1 \
    -D_DEFAULT_SOURCE
  objs="" nobjs=0

  echo "zlib: building $arch ($libc)"
  command -v "$cc" >/dev/null || { echo "missing compiler $cc"; exit 1; }
  rm -rf "$out"
  mkdir -p "$obj" "$lib" "$out/include"

  for name in $ZLIB_SRCS; do
    src="$SRC/$name"
    [ -f "$src" ] || { echo "missing $src"; exit 1; }
    o="$obj/${name%.c}.o"
    echo "  CC $arch $name"
    "$cc" "$@" -c "$src" -o "$o"
    objs="$objs $o" nobjs=$((nobjs + 1))
  done
  # shellcheck disable=SC2086
  "${triple}-ar" rcs "$lib/libz.a" $objs
  cp "$SRC/zlib.h" "$SRC/zconf.h" "$out/include/"
  if [ "$libc" = mlibc ]; then
    mkdir -p "$lib/pkgconfig"
    cat >"$lib/pkgconfig/zlib.pc" <<PC
prefix=$out
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: zlib
Description: zlib compression library (myos, mlibc)
Version: $ZLIB_VERSION
Libs: -L\${libdir} -lz
Cflags: -I\${includedir}
PC
  else
    cp "$lib/libz.a" "$ROOT/target/zlib-libz-${triple}.a"
  fi
  echo "zlib $arch ($libc): $nobjs objs -> $lib/libz.a"
)

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch" newlib
  build_arch "$arch" mlibc
done
echo "$(myos_zlib_version_hash)" >"$MYOS_ZLIB_VERSION"
echo "zlib build ok"
