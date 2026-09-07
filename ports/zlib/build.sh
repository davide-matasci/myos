#!/usr/bin/env bash
# Cross-build static libz.a for myos arches (like mbedtls).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

echo "zlib: version=$ZLIB_VERSION"

if myos_zlib_is_current; then
  echo "zlib libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/zlib-src"

# Core + gzip API (git uses deflate/inflate; gz* kept for completeness).
ZLIB_SRCS=(
  adler32.c compress.c crc32.c deflate.c
  gzclose.c gzlib.c gzread.c gzwrite.c
  inflate.c infback.c inftrees.c inffast.c
  trees.c uncompr.c zutil.c
)

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local out="$ROOT/target/zlib-${arch}"
  local obj="$out/obj"
  local lib="$out/lib"
  local clang_res
  clang_res="$(clang -print-resource-dir)/include"
  local cflags=(
    -ffreestanding -fPIC -Os -g0 -std=gnu99
    -nostdinc
    -isystem "$clang_res"
    -isystem "$inc"
    -I"$SRC"
    -D_LARGEFILE64_SOURCE=1
    -D_DEFAULT_SOURCE
  )
  local name src o objs=()

  echo "zlib: building $arch with $cc"
  command -v "$cc" >/dev/null || { echo "missing compiler $cc"; exit 1; }
  rm -rf "$out"
  mkdir -p "$obj" "$lib" "$out/include"

  for name in "${ZLIB_SRCS[@]}"; do
    src="$SRC/$name"
    [[ -f "$src" ]] || { echo "missing $src"; exit 1; }
    o="$obj/${name%.c}.o"
    echo "  CC $arch $name"
    "$cc" "${cflags[@]}" -c "$src" -o "$o"
    objs+=("$o")
  done
  ar rcs "$lib/libz.a" "${objs[@]}"
  cp "$SRC/zlib.h" "$SRC/zconf.h" "$out/include/"
  cp "$lib/libz.a" "$ROOT/target/zlib-libz-${triple}.a"
  echo "zlib $arch: ${#objs[@]} objs -> $lib/libz.a"
}

build_arch x86_64
build_arch aarch64
build_arch riscv64
echo "$(myos_zlib_version_hash)" >"$MYOS_ZLIB_VERSION"
echo "zlib build ok"
