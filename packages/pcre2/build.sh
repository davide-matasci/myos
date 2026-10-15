#!/usr/bin/env bash
# Cross-build static libpcre2-8.a (Unicode, no JIT) for the three arches.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_pcre2_is_current; then
  echo "pcre2 libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/pcre2-src"

# COMMON_SOURCES of Makefile.am (the .c files) and the generated tables.
PCRE2_SRCS=(
  pcre2_auto_possess.c pcre2_chkdint.c pcre2_compile.c pcre2_compile_cgroup.c
  pcre2_compile_class.c pcre2_config.c pcre2_context.c pcre2_convert.c
  pcre2_dfa_match.c pcre2_error.c pcre2_extuni.c pcre2_find_bracket.c
  pcre2_jit_compile.c pcre2_maketables.c pcre2_match.c pcre2_match_data.c
  pcre2_match_next.c pcre2_newline.c pcre2_ord2utf.c pcre2_pattern_info.c
  pcre2_script_run.c pcre2_serialize.c pcre2_string_utils.c pcre2_study.c
  pcre2_substitute.c pcre2_substring.c pcre2_tables.c pcre2_ucd.c
  pcre2_valid_utf.c pcre2_xclass.c pcre2_chartables.c
)

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local cc="${triple}-cc"
  local out="$ROOT/target/pcre2-${arch}"
  local gen="$out/gen"
  local cflags=(
    -ffreestanding -fPIC -O2 -g0 -std=gnu99
    -nostdinc -isystem "$(clang -print-resource-dir)/include"
    -isystem "$ROOT/target/newlib-${arch}/${triple}/include"
    -I"$gen" -I"$SRC/src"
    -DHAVE_CONFIG_H -DPCRE2_CODE_UNIT_WIDTH=8 -DPCRE2_STATIC
    -DSUPPORT_PCRE2_8 -DSUPPORT_UNICODE
    -DHAVE_STDINT_H -DHAVE_INTTYPES_H -DHAVE_LIMITS_H -DHAVE_STRING_H
    -DHAVE_STDLIB_H -DHAVE_ASSERT_H
  )
  local name o objs=()

  echo "pcre2: building $arch with $cc"
  rm -rf "$out"
  mkdir -p "$gen" "$out/obj" "$out/lib" "$out/include"
  # What configure would generate: the generic config and header, the
  # default character tables.
  cp "$SRC/src/config.h.generic" "$gen/config.h"
  cp "$SRC/src/pcre2.h.generic" "$gen/pcre2.h"
  cp "$SRC/src/pcre2_chartables.c.dist" "$gen/pcre2_chartables.c"

  for name in "${PCRE2_SRCS[@]}"; do
    if [[ -f "$gen/$name" ]]; then src="$gen/$name"; else src="$SRC/src/$name"; fi
    o="$out/obj/${name%.c}.o"
    "$cc" "${cflags[@]}" -c "$src" -o "$o"
    objs+=("$o")
  done
  "${triple}-ar" rcs "$out/lib/libpcre2-8.a" "${objs[@]}"
  cp "$gen/pcre2.h" "$out/include/"
  echo "pcre2 $arch: ${#objs[@]} objs -> $out/lib/libpcre2-8.a"
}

build_arch x86_64
build_arch aarch64
build_arch riscv64
echo "$(myos_pcre2_version_hash)" >"$MYOS_PCRE2_VERSION"
echo "pcre2 build ok"
