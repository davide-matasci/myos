#!/usr/bin/env bash
# Cross-build static libpcre2-8 for myos (newlib headers + freestanding CC).
# Sources: target/pcre2-src (fetched). Headers: ports/ripgrep/pcre2-headers
# (configure-generated config.h/pcre2.h with SUPPORT_JIT left undefined).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

"$ROOT/ports/ripgrep/fetch-pcre2.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/pcre2-src"
HDR="$ROOT/ports/ripgrep/pcre2-headers"

# Same file list as pcre2-sys (minus JIT + chartables generated from .dist).
PCRE2_SRCS=(
  pcre2_auto_possess.c pcre2_chkdint.c pcre2_compile.c pcre2_compile_class.c
  pcre2_config.c pcre2_context.c pcre2_convert.c pcre2_dfa_match.c
  pcre2_error.c pcre2_extuni.c pcre2_find_bracket.c pcre2_maketables.c
  pcre2_match.c pcre2_match_data.c pcre2_newline.c pcre2_ord2utf.c
  pcre2_pattern_info.c pcre2_script_run.c pcre2_serialize.c
  pcre2_string_utils.c pcre2_study.c pcre2_substitute.c pcre2_substring.c
  pcre2_tables.c pcre2_ucd.c pcre2_valid_utf.c pcre2_xclass.c
  pcre2_jit_compile.c
)

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/pcre2-${arch}"
  local stamp="$out/.myos-pcre2-stamp"
  local want
  want="$(
    {
      sha256sum "$ROOT/ports/ripgrep/build-pcre2.sh"
      sha256sum "$ROOT/ports/ripgrep/fetch-pcre2.sh"
      sha256sum "$ROOT/ports/ripgrep/versions.env"
      sha256sum "$HDR/config.h" "$HDR/pcre2.h"
      myos_newlib_version_hash
    } | sha256sum | awk '{print $1}'
  )"
  if [[ -f "$stamp" && "$(cat "$stamp")" == "$want" && -f "$out/lib/libpcre2-8.a" ]]; then
    echo "pcre2 ${arch} up to date"
    return 0
  fi

  local prefix="$ROOT/target/newlib-${arch}/${triple}"
  local inc="$prefix/include"
  local cc="${triple}-cc"
  local objdir="$out/obj"
  local src base obj
  local objs=()

  rm -rf "$out"
  mkdir -p "$objdir" "$out/include" "$out/src" "$out/lib/pkgconfig"
  cp "$HDR/config.h" "$out/include/config.h"
  cp "$HDR/pcre2.h" "$out/include/pcre2.h"
  cp "$SRC/src/pcre2_chartables.c.dist" "$out/src/pcre2_chartables.c"

  echo "==> pcre2 ($triple)"
  local cflags=(
    -ffreestanding -fPIC -O2 -fno-builtin
    -DPCRE2_CODE_UNIT_WIDTH=8 -DPCRE2_STATIC -DHAVE_CONFIG_H
    -I"$out/include" -I"$SRC/src" -isystem "$inc"
  )
  for src in "${PCRE2_SRCS[@]}"; do
    base="${src%.c}"
    obj="$objdir/${base}.o"
    "$cc" "${cflags[@]}" -c "$SRC/src/$src" -o "$obj"
    objs+=("$obj")
  done
  "$cc" "${cflags[@]}" -c "$out/src/pcre2_chartables.c" -o "$objdir/pcre2_chartables.o"
  objs+=("$objdir/pcre2_chartables.o")
  ar rcs "$out/lib/libpcre2-8.a" "${objs[@]}"
  cat >"$out/lib/pkgconfig/libpcre2-8.pc" <<PC
prefix=$out
libdir=\${prefix}/lib
includedir=\${prefix}/include
Name: libpcre2-8
Description: PCRE2 8-bit (myos cross build, no JIT)
Version: 10.45
Libs: -L\${libdir} -lpcre2-8
Cflags: -I\${includedir} -DPCRE2_STATIC
PC
  echo "$want" >"$stamp"
  echo "pcre2 -> $out/lib/libpcre2-8.a ($(du -h "$out/lib/libpcre2-8.a" | awk '{print $1}'))"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done
